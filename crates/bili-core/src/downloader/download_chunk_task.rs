use std::{fs::File, future::Future, sync::Arc, time::Duration};

use parking_lot::Mutex;
use tokio::{
    sync::{Semaphore, watch},
    time::sleep,
};
use tracing::instrument;

use crate::downloader::{
    chunk_file::write_chunk, download_task::DownloadTask, download_task_state::DownloadTaskState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadChunkResult {
    Downloaded(usize),
    Interrupted,
}

#[derive(Debug)]
pub struct DownloadInterrupted;

impl std::fmt::Display for DownloadInterrupted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("下载已中断")
    }
}

impl std::error::Error for DownloadInterrupted {}

#[derive(Clone)]
struct ChunkSignals {
    state: watch::Receiver<DownloadTaskState>,
    restart: watch::Receiver<()>,
    delete: watch::Receiver<()>,
    cancel: watch::Receiver<()>,
}

impl ChunkSignals {
    fn interrupted(&self) -> bool {
        !matches!(self.restart.has_changed(), Ok(false))
            || !matches!(self.delete.has_changed(), Ok(false))
            || !matches!(self.cancel.has_changed(), Ok(false))
    }

    async fn wait_until_downloading(&mut self) -> Result<(), DownloadInterrupted> {
        loop {
            if self.interrupted() {
                return Err(DownloadInterrupted);
            }
            let state = *self.state.borrow_and_update();
            match state {
                DownloadTaskState::Downloading => return Ok(()),
                DownloadTaskState::Paused | DownloadTaskState::Pending => {}
                _ => return Err(DownloadInterrupted),
            }
            tokio::select! {
                biased;
                _ = self.restart.changed() => return Err(DownloadInterrupted),
                _ = self.delete.changed() => return Err(DownloadInterrupted),
                _ = self.cancel.changed() => return Err(DownloadInterrupted),
                changed = self.state.changed() => {
                    if changed.is_err() {
                        return Err(DownloadInterrupted);
                    }
                },
            }
        }
    }
}

pub struct DownloadChunkTask {
    pub download_task: Arc<DownloadTask>,
    pub start: u64,
    pub end: u64,
    pub url: String,
    pub file: Arc<Mutex<File>>,
    pub chunk_index: usize,
}

impl DownloadChunkTask {
    #[instrument(
        level = "error",
        skip_all,
        fields(
            chunk_index = ?self.chunk_index,
            start = self.start,
            end = self.end,
        )
    )]
    pub async fn process(self) -> eyre::Result<DownloadChunkResult> {
        let signals = ChunkSignals {
            state: self.download_task.state_sender.subscribe(),
            restart: self.download_task.restart_sender.subscribe(),
            delete: self.download_task.delete_sender.subscribe(),
            cancel: self.download_task.cancel_sender.subscribe(),
        };
        let semaphore = self
            .download_task
            .app
            .get_download_manager()
            .media_chunk_sem
            .clone();
        run_chunk(self.download_chunk(signals.clone()), semaphore, signals).await
    }

    #[instrument(level = "error", skip_all)]
    async fn download_chunk(&self, mut signals: ChunkSignals) -> eyre::Result<DownloadChunkResult> {
        let bili_client = self.download_task.app.get_bili_client();
        let chunk_data = bili_client
            .get_media_chunk(&self.url, self.start, self.end)
            .await?;

        loop {
            signals.wait_until_downloading().await?;
            let mut file = self.file.lock();
            if signals.interrupted() {
                return Err(DownloadInterrupted.into());
            }
            // A pause may arrive while waiting for the file lock. Retain the downloaded body.
            if *signals.state.borrow() != DownloadTaskState::Downloading {
                continue;
            }
            write_chunk(&mut *file, self.start, self.end, &chunk_data)?;
            break;
        }

        let len = u64::try_from(chunk_data.len())?;
        self.download_task
            .app
            .get_download_manager()
            .byte_per_sec
            .fetch_add(len, std::sync::atomic::Ordering::Relaxed);

        let chunk_download_interval_sec = self
            .download_task
            .app
            .get_config()
            .read()
            .chunk_download_interval_sec;
        sleep(Duration::from_secs(chunk_download_interval_sec)).await;

        Ok(DownloadChunkResult::Downloaded(self.chunk_index))
    }
}

/// Keep the same future across pauses so an already received body is not lost or retried.
async fn run_chunk(
    download: impl Future<Output = eyre::Result<DownloadChunkResult>>,
    semaphore: Arc<Semaphore>,
    mut signals: ChunkSignals,
) -> eyre::Result<DownloadChunkResult> {
    tokio::pin!(download);
    let mut permit = None;
    loop {
        let state = *signals.state.borrow();
        if !matches!(
            state,
            DownloadTaskState::Downloading | DownloadTaskState::Paused | DownloadTaskState::Pending
        ) {
            return Ok(DownloadChunkResult::Interrupted);
        }
        let downloading = state == DownloadTaskState::Downloading;
        tokio::select! {
            biased;
            _ = signals.restart.changed() => return Ok(DownloadChunkResult::Interrupted),
            _ = signals.delete.changed() => return Ok(DownloadChunkResult::Interrupted),
            _ = signals.cancel.changed() => return Ok(DownloadChunkResult::Interrupted),
            changed = signals.state.changed() => {
                if changed.is_err() {
                    return Ok(DownloadChunkResult::Interrupted);
                }
                if *signals.state.borrow() != DownloadTaskState::Downloading {
                    permit = None;
                }
            },
            acquired = semaphore.clone().acquire_owned(), if downloading && permit.is_none() => {
                permit = Some(acquired?);
            },
            result = &mut download, if downloading && permit.is_some() => {
                drop(permit.take());
                if signals.wait_until_downloading().await.is_err() {
                    return Ok(DownloadChunkResult::Interrupted);
                }
                return match result {
                    Err(err) if err.is::<DownloadInterrupted>() => Ok(DownloadChunkResult::Interrupted),
                    result => result,
                };
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tokio::sync::oneshot;

    use super::*;

    struct Controls {
        state: watch::Sender<DownloadTaskState>,
        restart: watch::Sender<()>,
        delete: watch::Sender<()>,
        cancel: watch::Sender<()>,
    }

    fn controls(state: DownloadTaskState) -> (Controls, ChunkSignals) {
        let (state_tx, state) = watch::channel(state);
        let (restart_tx, restart) = watch::channel(());
        let (delete_tx, delete) = watch::channel(());
        let (cancel_tx, cancel) = watch::channel(());
        (
            Controls {
                state: state_tx,
                restart: restart_tx,
                delete: delete_tx,
                cancel: cancel_tx,
            },
            ChunkSignals {
                state,
                restart,
                delete,
                cancel,
            },
        )
    }

    #[tokio::test]
    async fn interruption_wins_over_a_ready_write_and_never_reports_completion() {
        for interrupt in 0..3 {
            let (controls, signals) = controls(DownloadTaskState::Downloading);
            let writer = Arc::new(Mutex::new(Cursor::new(vec![0; 4])));
            let output = writer.clone();
            let download = async move {
                write_chunk(&mut *output.lock(), 0, 3, b"data")?;
                Ok(DownloadChunkResult::Downloaded(0))
            };
            match interrupt {
                0 => controls.restart.send(()).unwrap(),
                1 => controls.delete.send(()).unwrap(),
                _ => controls.cancel.send(()).unwrap(),
            }
            let result = run_chunk(download, Arc::new(Semaphore::new(1)), signals)
                .await
                .unwrap();
            assert_eq!(result, DownloadChunkResult::Interrupted);
            assert_eq!(writer.lock().get_ref(), &[0; 4]);
        }
    }

    #[tokio::test]
    async fn pause_releases_permit_and_preserves_inflight_future_through_pending() {
        let (controls, signals) = controls(DownloadTaskState::Downloading);
        let semaphore = Arc::new(Semaphore::new(1));
        let writer = Arc::new(Mutex::new(Cursor::new(vec![0; 4])));
        let output = writer.clone();
        let (started_tx, started_rx) = oneshot::channel();
        let (body_tx, body_rx) = oneshot::channel::<&'static [u8]>();
        let download = async move {
            started_tx.send(()).unwrap();
            let data = body_rx.await?;
            write_chunk(&mut *output.lock(), 0, 3, data)?;
            Ok(DownloadChunkResult::Downloaded(0))
        };
        let worker_semaphore = semaphore.clone();
        let worker =
            tokio::spawn(async move { run_chunk(download, worker_semaphore, signals).await });
        started_rx.await.unwrap();
        controls.state.send(DownloadTaskState::Paused).unwrap();
        let released = tokio::time::timeout(Duration::from_secs(1), semaphore.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(released);
        body_tx.send(b"data").unwrap();
        tokio::task::yield_now().await;
        assert_eq!(writer.lock().get_ref(), &[0; 4]);
        assert!(!worker.is_finished());
        controls.state.send(DownloadTaskState::Pending).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(writer.lock().get_ref(), &[0; 4]);
        assert!(!worker.is_finished());
        controls.state.send(DownloadTaskState::Downloading).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result, DownloadChunkResult::Downloaded(0));
        assert_eq!(writer.lock().get_ref(), b"data");
    }

    #[tokio::test]
    async fn short_body_write_cannot_report_downloaded() {
        let (_controls, signals) = controls(DownloadTaskState::Downloading);
        let download = async {
            let mut writer = Cursor::new(vec![0; 4]);
            write_chunk(&mut writer, 0, 3, b"bad")?;
            Ok(DownloadChunkResult::Downloaded(0))
        };
        assert!(
            run_chunk(download, Arc::new(Semaphore::new(1)), signals)
                .await
                .is_err()
        );
    }
}
