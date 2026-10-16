use std::{
    fs::{File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::Path,
};

use fs4::fs_std::FileExt;

use super::media_chunk::MediaChunk;

/// The bitmap belongs to the temporary file. Recreating that file invalidates every bit.
pub(crate) fn open_chunk_file(
    path: &Path,
    content_length: u64,
    chunks: &mut [MediaChunk],
    persist_reset: impl FnOnce(&[MediaChunk]),
) -> io::Result<File> {
    let reusable = match path.metadata() {
        Ok(metadata) => metadata.len() == content_length,
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => return Err(err),
    };

    if reusable {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        return Ok(file);
    }

    for chunk in chunks.iter_mut() {
        chunk.completed = false;
    }
    // Save the invalidation before truncation, including when allocation subsequently fails.
    persist_reset(chunks);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.allocate(content_length)?;
    file.set_len(content_length)?;
    Ok(file)
}

/// A chunk is eligible for completion only after its exact byte range is written in full.
pub(crate) fn write_chunk(
    writer: &mut (impl Seek + Write),
    start: u64,
    end: u64,
    data: &[u8],
) -> io::Result<()> {
    let expected_length = end
        .checked_sub(start)
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "无效的分片范围"))?;
    if u64::try_from(data.len()).ok() != Some(expected_length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "分片内容长度与请求范围不一致",
        ));
    }
    writer.seek(SeekFrom::Start(start))?;
    writer.write_all(data)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("bili-chunk-{}", uuid::Uuid::new_v4())))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn chunks() -> Vec<MediaChunk> {
        vec![
            MediaChunk {
                start: 0,
                end: 3,
                completed: true,
            },
            MediaChunk {
                start: 4,
                end: 7,
                completed: false,
            },
        ]
    }

    #[test]
    fn existing_file_preserves_bytes_and_completed_chunks() {
        let temp = TempFile::new();
        std::fs::write(&temp.0, b"abcd\0\0\0\0").unwrap();
        let mut bitmap = chunks();
        let mut file = open_chunk_file(&temp.0, 8, &mut bitmap, |_| {
            panic!("an existing temporary file must retain its bitmap");
        })
        .unwrap();
        assert!(bitmap[0].completed);
        assert!(!bitmap[1].completed);
        write_chunk(&mut file, 4, 7, b"efgh").unwrap();
        assert_eq!(std::fs::read(&temp.0).unwrap(), b"abcdefgh");
    }

    #[test]
    fn missing_file_clears_old_bitmap() {
        let temp = TempFile::new();
        let mut bitmap = chunks();
        let mut reset_saved = false;
        let file = open_chunk_file(&temp.0, 8, &mut bitmap, |chunks| {
            assert!(!temp.0.exists());
            assert!(chunks.iter().all(|chunk| !chunk.completed));
            reset_saved = true;
        })
        .unwrap();
        assert!(reset_saved);
        assert_eq!(file.metadata().unwrap().len(), 8);
        assert!(bitmap.iter().all(|chunk| !chunk.completed));
    }

    #[test]
    fn wrong_size_clears_old_bitmap() {
        let temp = TempFile::new();
        std::fs::write(&temp.0, b"old").unwrap();
        let mut bitmap = chunks();
        let mut reset_saved = false;
        let file = open_chunk_file(&temp.0, 8, &mut bitmap, |chunks| {
            assert_eq!(std::fs::read(&temp.0).unwrap(), b"old");
            assert!(chunks.iter().all(|chunk| !chunk.completed));
            reset_saved = true;
        })
        .unwrap();
        assert!(reset_saved);
        assert_eq!(file.metadata().unwrap().len(), 8);
        assert!(bitmap.iter().all(|chunk| !chunk.completed));
    }

    #[test]
    fn invalid_body_is_rejected_before_any_write() {
        let mut writer = Cursor::new(b"abcdefgh".to_vec());
        assert!(write_chunk(&mut writer, 2, 5, b"xy").is_err());
        assert_eq!(writer.into_inner(), b"abcdefgh");
    }

    struct ShortWriter {
        inner: Cursor<Vec<u8>>,
        remaining: usize,
    }

    impl Seek for ShortWriter {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    impl Write for ShortWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("simulated disk write failure"));
            }
            let count = data.len().min(self.remaining);
            self.remaining -= count;
            self.inner.write(&data[..count])
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn incomplete_disk_write_is_an_error() {
        let mut writer = ShortWriter {
            inner: Cursor::new(vec![0; 8]),
            remaining: 2,
        };
        assert!(write_chunk(&mut writer, 4, 7, b"efgh").is_err());
        assert_eq!(&writer.inner.into_inner()[4..], b"ef\0\0");
    }

    #[tokio::test]
    async fn resumes_only_missing_range_after_body_disconnect_and_preserves_existing_bytes() {
        use std::time::Duration;

        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        use crate::media_transfer::{RetryPolicy, download_media_chunk_with_policy};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/media", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in [b"ef".as_slice(), b"efgh".as_slice()] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut buffer = [0; 512];
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 4-7/8\r\nConnection: close\r\n\r\n").await.unwrap();
                stream.write_all(body).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            requests
        });

        let temp = TempFile::new();
        std::fs::write(&temp.0, b"abcd\0\0\0\0").unwrap();
        let mut bitmap = chunks();
        let mut file = open_chunk_file(&temp.0, 8, &mut bitmap, |_| {
            panic!("the existing first range must be preserved");
        })
        .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for chunk in &mut bitmap {
            if chunk.completed {
                continue;
            }
            let data = download_media_chunk_with_policy(
                &client,
                &url,
                chunk.start,
                chunk.end,
                RetryPolicy {
                    max_attempts: 2,
                    attempt_timeout: Duration::from_secs(1),
                    initial_backoff: Duration::ZERO,
                    max_backoff: Duration::ZERO,
                    max_jitter: Duration::ZERO,
                },
            )
            .await
            .unwrap();
            write_chunk(&mut file, chunk.start, chunk.end, &data).unwrap();
            chunk.completed = true;
        }
        drop(file);
        assert_eq!(std::fs::read(&temp.0).unwrap(), b"abcdefgh");
        let requests = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert!(
                request
                    .to_ascii_lowercase()
                    .lines()
                    .any(|line| line == "range: bytes=4-7")
            );
        }
        let _reopened = open_chunk_file(&temp.0, 8, &mut bitmap, |_| {
            panic!("reopening the downloaded file must retain the bitmap");
        })
        .unwrap();
        assert!(bitmap.iter().all(|chunk| chunk.completed));
    }
}
