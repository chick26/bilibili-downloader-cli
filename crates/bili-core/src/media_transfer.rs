//! Bounded retries around an entire media range response, including its body.

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use eyre::{OptionExt, WrapErr, eyre};
use reqwest::{Client, StatusCode, header::CONTENT_RANGE};
use tokio::time::{sleep, timeout};

#[derive(Clone, Copy, Debug)]
pub(crate) struct RetryPolicy {
    /// Includes the initial request.
    pub(crate) max_attempts: usize,
    pub(crate) attempt_timeout: Duration,
    pub(crate) initial_backoff: Duration,
    pub(crate) max_backoff: Duration,
    pub(crate) max_jitter: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            attempt_timeout: Duration::from_secs(60),
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(4),
            max_jitter: Duration::from_millis(125),
        }
    }
}

struct TransferFailure {
    error: eyre::Report,
    retryable: bool,
}

impl TransferFailure {
    fn permanent(error: eyre::Report) -> Self {
        Self {
            error,
            retryable: false,
        }
    }

    fn transient(error: eyre::Report) -> Self {
        Self {
            error,
            retryable: true,
        }
    }

    fn transport(error: reqwest::Error) -> Self {
        let retryable = !error.is_builder();
        // Signed CDN URLs can contain credentials. Do not put them in errors.
        Self {
            error: eyre!(error.without_url()),
            retryable,
        }
    }
}

pub(crate) async fn download_media_chunk(
    client: &Client,
    url: &str,
    start: u64,
    end: u64,
) -> eyre::Result<Bytes> {
    download_media_chunk_with_policy(client, url, start, end, RetryPolicy::default()).await
}

pub(crate) async fn download_media_chunk_with_policy(
    client: &Client,
    url: &str,
    start: u64,
    end: u64,
    policy: RetryPolicy,
) -> eyre::Result<Bytes> {
    let expected_len = end
        .checked_sub(start)
        .and_then(|length| length.checked_add(1))
        .ok_or_eyre("无效的媒体分片范围")?;
    if policy.max_attempts == 0 {
        return Err(eyre!("媒体分片下载至少需要一次尝试"));
    }

    let mut backoff = policy.initial_backoff.min(policy.max_backoff);
    for attempt in 1..=policy.max_attempts {
        let result = timeout(
            policy.attempt_timeout,
            download_once(client, url, start, end, expected_len),
        )
        .await
        .unwrap_or_else(|_| {
            Err(TransferFailure::transient(eyre!(
                "媒体分片下载超时（{} 秒）",
                policy.attempt_timeout.as_secs_f64()
            )))
        });

        match result {
            Ok(bytes) => return Ok(bytes),
            Err(failure) => {
                if !failure.retryable || attempt == policy.max_attempts {
                    return Err(failure.error.wrap_err(format!(
                        "媒体分片 {start}-{end} 下载失败（已尝试 {attempt}/{} 次）",
                        policy.max_attempts
                    )));
                }

                tracing::warn!(
                    start, end, attempt, max_attempts = policy.max_attempts,
                    error = %failure.error, "媒体分片下载失败，将自动重试"
                );
                let jitter = policy.max_jitter.mul_f64(rand::random::<f64>());
                sleep(backoff.saturating_add(jitter).min(policy.max_backoff)).await;
                backoff = backoff.saturating_mul(2).min(policy.max_backoff);
            }
        }
    }

    unreachable!("a nonzero attempt budget always returns a result")
}

async fn download_once(
    client: &Client,
    url: &str,
    start: u64,
    end: u64,
    expected_len: u64,
) -> Result<Bytes, TransferFailure> {
    let mut response = client
        .get(url)
        .header("range", format!("bytes={start}-{end}"))
        .send()
        .await
        .map_err(TransferFailure::transport)?;

    let status = response.status();
    if status != StatusCode::PARTIAL_CONTENT {
        let error = eyre!("媒体分片响应状态码为 {status}，需要 206 Partial Content");
        return Err(
            if status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error()
            {
                TransferFailure::transient(error)
            } else {
                TransferFailure::permanent(error)
            },
        );
    }

    validate_content_range(response.headers(), start, end).map_err(TransferFailure::permanent)?;

    // Body read errors must be retried together with send(), not outside its retry boundary.
    // Refuse excess data before buffering it, even if a CDN returns a whole file for a range.
    let mut bytes = BytesMut::new();
    while let Some(chunk) = response.chunk().await.map_err(TransferFailure::transport)? {
        let actual_len = bytes.len() as u64;
        if chunk.len() as u64 > expected_len - actual_len {
            return Err(TransferFailure::permanent(eyre!(
                "媒体分片正文超过预期的 {expected_len} 字节"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    let actual_len = bytes.len() as u64;
    if actual_len != expected_len {
        return Err(TransferFailure::transient(eyre!(
            "媒体分片长度不符：预期 {expected_len} 字节，实际 {actual_len} 字节"
        )));
    }

    Ok(bytes.freeze())
}

fn validate_content_range(
    headers: &reqwest::header::HeaderMap,
    start: u64,
    end: u64,
) -> eyre::Result<()> {
    let header = headers
        .get(CONTENT_RANGE)
        .ok_or_eyre("媒体分片响应缺少 Content-Range")?
        .to_str()
        .wrap_err("媒体分片 Content-Range 不是有效文本")?;
    let invalid_range = || eyre!("媒体分片 Content-Range 无效或与请求范围不符");
    let value = header.strip_prefix("bytes ").ok_or_else(invalid_range)?;
    let (range, total) = value.split_once('/').ok_or_else(invalid_range)?;
    let (response_start, response_end) = range.split_once('-').ok_or_else(invalid_range)?;
    let response_start = response_start.parse::<u64>().map_err(|_| invalid_range())?;
    let response_end = response_end.parse::<u64>().map_err(|_| invalid_range())?;
    let total = total.parse::<u64>().map_err(|_| invalid_range())?;
    if response_start != start || response_end != end || total <= end {
        return Err(invalid_range());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::{JoinHandle, JoinSet},
    };

    use super::*;

    #[derive(Clone)]
    struct Reply {
        bytes: &'static [u8],
        stall_after_write: Duration,
    }

    impl Reply {
        fn raw(bytes: &'static [u8]) -> Self {
            Self {
                bytes,
                stall_after_write: Duration::ZERO,
            }
        }
    }

    struct MockServer {
        url: String,
        requests: Arc<AtomicUsize>,
        received_headers: Arc<Mutex<Vec<String>>>,
        task: JoinHandle<()>,
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl MockServer {
        async fn start(replies: Vec<Reply>) -> Self {
            assert!(!replies.is_empty());
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/media", listener.local_addr().unwrap());
            let requests = Arc::new(AtomicUsize::new(0));
            let count = requests.clone();
            let received_headers = Arc::new(Mutex::new(Vec::new()));
            let headers = received_headers.clone();
            let task = tokio::spawn(async move {
                let mut connections = JoinSet::new();
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let index = count.fetch_add(1, Ordering::SeqCst);
                    let reply = replies[index.min(replies.len() - 1)].clone();
                    let headers = headers.clone();
                    connections.spawn(async move {
                        let mut request = Vec::new();
                        let mut byte = [0];
                        while !request.ends_with(b"\r\n\r\n") {
                            if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                                return;
                            }
                            request.push(byte[0]);
                            assert!(request.len() < 16 * 1024);
                        }
                        let request = String::from_utf8(request).unwrap().to_lowercase();
                        headers.lock().unwrap().push(request);
                        let _ = stream.write_all(reply.bytes).await;
                        sleep(reply.stall_after_write).await;
                        let _ = stream.shutdown().await;
                    });
                }
            });
            Self {
                url,
                requests,
                received_headers,
                task,
            }
        }

        fn attempts(&self) -> usize {
            let count = self.requests.load(Ordering::SeqCst);
            let headers = self.received_headers.lock().unwrap();
            assert_eq!(headers.len(), count);
            assert!(
                headers
                    .iter()
                    .all(|request| request.contains("\r\nrange: bytes=4-7\r\n"))
            );
            count
        }

        async fn download(&self, max_attempts: usize) -> eyre::Result<Bytes> {
            let client = Client::builder().no_proxy().build().unwrap();
            download_media_chunk_with_policy(
                &client,
                &self.url,
                4,
                7,
                RetryPolicy {
                    max_attempts,
                    attempt_timeout: Duration::from_millis(150),
                    initial_backoff: Duration::ZERO,
                    max_backoff: Duration::ZERO,
                    max_jitter: Duration::ZERO,
                },
            )
            .await
        }
    }

    const VALID: &[u8] = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/12\r\nContent-Length: 4\r\nConnection: close\r\n\r\nwxyz";
    const TRUNCATED: &[u8] = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/12\r\nContent-Length: 4\r\nConnection: close\r\n\r\nwx";
    const SHORT: &[u8] = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/12\r\nConnection: close\r\n\r\nwx";
    const UNAVAILABLE: &[u8] =
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    #[tokio::test]
    async fn retries_body_disconnect_and_returns_only_complete_attempt() {
        let server = MockServer::start(vec![Reply::raw(TRUNCATED), Reply::raw(VALID)]).await;
        assert_eq!(server.download(4).await.unwrap().as_ref(), b"wxyz");
        assert_eq!(server.attempts(), 2);
    }

    #[tokio::test]
    async fn retries_stalled_body_after_attempt_deadline() {
        let server = MockServer::start(vec![
            Reply {
                bytes: TRUNCATED,
                stall_after_write: Duration::from_secs(1),
            },
            Reply::raw(VALID),
        ])
        .await;
        let started = tokio::time::Instant::now();
        assert_eq!(server.download(4).await.unwrap().as_ref(), b"wxyz");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(server.attempts(), 2);
    }

    #[tokio::test]
    async fn retries_stalled_response_headers_after_attempt_deadline() {
        let server = MockServer::start(vec![
            Reply {
                bytes: b"",
                stall_after_write: Duration::from_secs(1),
            },
            Reply::raw(VALID),
        ])
        .await;
        let started = tokio::time::Instant::now();
        assert_eq!(server.download(4).await.unwrap().as_ref(), b"wxyz");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(server.attempts(), 2);
    }

    #[tokio::test]
    async fn retries_clean_eof_with_short_body() {
        let server = MockServer::start(vec![Reply::raw(SHORT), Reply::raw(VALID)]).await;
        assert_eq!(server.download(4).await.unwrap().as_ref(), b"wxyz");
        assert_eq!(server.attempts(), 2);
    }

    #[tokio::test]
    async fn retries_temporary_http_statuses() {
        for status in [
            b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n".as_slice(),
            UNAVAILABLE,
        ] {
            let server = MockServer::start(vec![Reply::raw(status), Reply::raw(VALID)]).await;
            assert_eq!(server.download(4).await.unwrap().as_ref(), b"wxyz");
            assert_eq!(server.attempts(), 2);
        }
    }

    #[tokio::test]
    async fn permanent_http_errors_do_not_retry() {
        for response in [
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n".as_slice(),
        ] {
            let server = MockServer::start(vec![Reply::raw(response), Reply::raw(VALID)]).await;
            assert!(server.download(4).await.is_err());
            assert_eq!(server.attempts(), 1);
        }
    }

    #[tokio::test]
    async fn exhaustion_respects_total_attempt_budget() {
        let server = MockServer::start(vec![Reply::raw(UNAVAILABLE)]).await;
        let error = server.download(3).await.unwrap_err();
        assert!(error.to_string().contains("3/3"));
        assert_eq!(server.attempts(), 3);
    }

    #[tokio::test]
    async fn persistent_short_body_exhausts_attempt_budget() {
        let server = MockServer::start(vec![Reply::raw(SHORT)]).await;
        assert!(server.download(3).await.is_err());
        assert_eq!(server.attempts(), 3);
    }

    #[tokio::test]
    async fn rejects_non_partial_response_or_missing_content_range() {
        for response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nwxyz".as_slice(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\n\r\nwxyz".as_slice(),
        ] {
            let server = MockServer::start(vec![Reply::raw(response), Reply::raw(VALID)]).await;
            assert!(server.download(4).await.is_err());
            assert_eq!(server.attempts(), 1);
        }
    }

    #[tokio::test]
    async fn rejects_wrong_range_or_invalid_total_without_retry() {
        for response in [
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/12\r\nContent-Length: 4\r\n\r\nwxyz".as_slice(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-8/12\r\nContent-Length: 4\r\n\r\nwxyz".as_slice(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/7\r\nContent-Length: 4\r\n\r\nwxyz".as_slice(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/*\r\nContent-Length: 4\r\n\r\nwxyz".as_slice(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: invalid\r\nContent-Length: 4\r\n\r\nwxyz".as_slice(),
        ] {
            let server = MockServer::start(vec![Reply::raw(response), Reply::raw(VALID)]).await;
            assert!(server.download(4).await.is_err());
            assert_eq!(server.attempts(), 1);
        }
    }

    #[tokio::test]
    async fn rejects_oversized_body_without_retry() {
        let server = MockServer::start(vec![Reply::raw(
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/12\r\nContent-Length: 8\r\n\r\nwxyzABCD"
        )]).await;
        assert!(server.download(4).await.is_err());
        assert_eq!(server.attempts(), 1);
    }

    #[tokio::test]
    async fn rejects_invalid_request_range_before_network_io() {
        let client = Client::builder().no_proxy().build().unwrap();
        for (start, end) in [(7, 4), (0, u64::MAX)] {
            assert!(
                download_media_chunk_with_policy(
                    &client,
                    "http://127.0.0.1:1/media",
                    start,
                    end,
                    RetryPolicy::default()
                )
                .await
                .is_err()
            );
        }
    }
}
