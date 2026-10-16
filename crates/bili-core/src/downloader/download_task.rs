use std::{sync::Arc, time::Duration};

use crate::runtime::AppContext;
use eyre::WrapErr;
use parking_lot::{Mutex, RwLock};
use tokio::{
    sync::{OwnedSemaphorePermit, watch},
    time::sleep,
};
use tracing::instrument;

use crate::{
    downloader::episode_type::EpisodeType, events::DownloadEvent, extensions::EyreReportToMessage,
    types::create_download_task_params::CreateDownloadTaskParams,
};

use super::{
    download_chunk_task::DownloadInterrupted, download_progress::DownloadProgress,
    download_task_state::DownloadTaskState,
};

pub struct DownloadTask {
    pub app: AppContext,
    pub state_sender: watch::Sender<DownloadTaskState>,
    pub restart_sender: watch::Sender<()>,
    pub cancel_sender: watch::Sender<()>,
    pub delete_sender: watch::Sender<()>,
    pub task_id: String,
    pub trace_fields: DownloadTaskTraceFields,
    pub progress: RwLock<DownloadProgress>,
    pub last_error: RwLock<Option<String>>,
    process_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    chunk_count: watch::Sender<usize>,
}

impl DownloadTask {
    #[allow(clippy::too_many_lines)]
    #[instrument(level = "error", skip_all)]
    pub fn from_params(
        app: &AppContext,
        params: &CreateDownloadTaskParams,
    ) -> eyre::Result<Vec<Arc<Self>>> {
        use CreateDownloadTaskParams::{Bangumi, Cheese, Normal};

        let mut progresses = Vec::new();
        match params {
            Normal(params) => {
                for &(aid, cid) in &params.aid_cid_pairs {
                    let span = tracing::error_span!(
                        "from_params_normal",
                        aid = aid,
                        bvid = params.info.bvid,
                        cid = cid,
                        collection_title = params.info.title,
                        up_name = params.info.owner.name,
                        up_uid = params.info.owner.mid,
                    );
                    let _enter = span.enter();

                    let progress = DownloadProgress::from_normal(app, &params.info, aid, cid)
                        .wrap_err("创建普通视频的下载进度失败")?;

                    progresses.extend(progress);
                }
            }
            Bangumi(params) => {
                for ep_id in &params.ep_ids {
                    let span = tracing::error_span!(
                        "from_params_bangumi",
                        ep_id = ep_id,
                        collection_title = params.info.title,
                        up_name = params.info.up_info.as_ref().map(|up_info| &up_info.uname),
                        up_uid = params.info.up_info.as_ref().map(|up_info| up_info.mid),
                    );
                    let _enter = span.enter();

                    let progress = DownloadProgress::from_bangumi(app, &params.info, *ep_id)
                        .wrap_err("创建番剧的下载进度失败")?;

                    progresses.push(progress);
                }
            }
            Cheese(params) => {
                for ep_id in &params.ep_ids {
                    let span = tracing::error_span!(
                        "from_params_cheese",
                        ep_id = ep_id,
                        collection_title = params.info.title,
                        up_name = params.info.up_info.uname,
                        up_uid = params.info.up_info.mid,
                    );
                    let _enter = span.enter();

                    let progress = DownloadProgress::from_cheese(app, &params.info, *ep_id)
                        .wrap_err("创建课程的下载进度失败")?;

                    progresses.push(progress);
                }
            }
        }

        let mut tasks = Vec::new();
        for progress in progresses {
            let span = tracing::error_span!(
                "create_tasks",
                task_id = progress.task_id,
                episode_type = ?progress.episode_type,
                aid = progress.aid,
                bvid = progress.bvid,
                cid = progress.cid,
                ep_id = progress.ep_id,
                collection_title = progress.collection_title,
                episode_title = progress.episode_title,
                episode_order = progress.episode_order,
                part_title = progress.part_title,
                part_order = progress.part_order,
                up_name = progress.up_name,
                up_uid = progress.up_uid,
            );
            let _enter = span.enter();

            progress
                .save(app, true)
                .wrap_err("保存下载进度到文件失败")?;

            let auto_start = app.get_config().read().auto_start_download_task;
            let init_state = if auto_start {
                DownloadTaskState::Pending
            } else {
                DownloadTaskState::Paused
            };

            let (state_sender, _) = watch::channel(init_state);
            let (restart_sender, _) = watch::channel(());
            let (cancel_sender, _) = watch::channel(());
            let (delete_sender, _) = watch::channel(());

            let task = Arc::new(Self {
                app: task_context(app, &progress),
                state_sender,
                restart_sender,
                cancel_sender,
                delete_sender,
                task_id: progress.task_id.clone(),
                trace_fields: DownloadTaskTraceFields::from(&progress),
                last_error: RwLock::new(progress.last_error.clone()),
                process_handle: Mutex::new(None),
                chunk_count: watch::channel(0).0,
                progress: RwLock::new(progress),
            });

            *task.process_handle.lock() = Some(tokio::spawn(task.clone().process()));

            tasks.push(task);
        }

        Ok(tasks)
    }

    pub fn from_progress(app: AppContext, progress: DownloadProgress) -> Arc<Self> {
        let init_state = if progress.last_error.is_some() {
            DownloadTaskState::Failed
        } else if progress.is_completed() {
            DownloadTaskState::Completed
        } else {
            DownloadTaskState::Paused
        };
        let (state_sender, _) = watch::channel(init_state);
        let (restart_sender, _) = watch::channel(());
        let (cancel_sender, _) = watch::channel(());
        let (delete_sender, _) = watch::channel(());

        let task = Arc::new(Self {
            app: task_context(&app, &progress),
            state_sender,
            restart_sender,
            cancel_sender,
            delete_sender,
            task_id: progress.task_id.clone(),
            trace_fields: DownloadTaskTraceFields::from(&progress),
            last_error: RwLock::new(progress.last_error.clone()),
            process_handle: Mutex::new(None),
            chunk_count: watch::channel(0).0,
            progress: RwLock::new(progress),
        });

        *task.process_handle.lock() = Some(tokio::spawn(task.clone().process()));

        task
    }

    #[instrument(
        level = "error",
        skip_all,
        fields(
            task_id = self.trace_fields.task_id,
            episode_type = ?self.trace_fields.episode_type,
            aid = self.trace_fields.aid,
            bvid = self.trace_fields.bvid,
            cid = self.trace_fields.cid,
            ep_id = self.trace_fields.ep_id,
            collection_title = self.trace_fields.collection_title,
            episode_title = self.trace_fields.episode_title,
            episode_order = self.trace_fields.episode_order,
            part_title = self.trace_fields.part_title,
            part_order = self.trace_fields.part_order,
            up_name = self.trace_fields.up_name,
            up_uid = self.trace_fields.up_uid,
        )
    )]
    async fn process(self: Arc<Self>) {
        let state = *self.state_sender.borrow();
        let progress = self.progress.read().clone();
        let _ = DownloadEvent::TaskCreate { state, progress }.emit(&self.app);

        let mut state_receiver = self.state_sender.subscribe();
        state_receiver.mark_changed();

        let mut restart_receiver = self.restart_sender.subscribe();
        let mut cancel_receiver = self.cancel_sender.subscribe();
        let mut delete_receiver = self.delete_sender.subscribe();

        let mut permit = None;
        let mut download_task_option = None;

        loop {
            let state = *state_receiver.borrow();
            let state_is_downloading = state == DownloadTaskState::Downloading;
            let state_is_pending = state == DownloadTaskState::Pending;

            let download_task = async {
                download_task_option
                    .get_or_insert_with(|| Box::pin(self.download()))
                    .await;
            };

            tokio::select! {
                biased;

                _ = restart_receiver.changed() => {
                    // Drop the old download before resetting its progress.
                    download_task_option = None;
                    self.handle_restart_notify();
                    tracing::debug!("下载任务已重来");
                }

                _ = cancel_receiver.changed() => return,

                _ = delete_receiver.changed() => {
                    let _ = DownloadEvent::TaskDelete {
                        task_id: self.task_id.clone(),
                    }
                    .emit(&self.app);

                    if permit.is_some() {
                        // 如果有permit则稍微等一下再退出
                        // 这是为了避免大批量删除时，本应删除的任务因拿到permit而又稍微下载一小段
                        sleep(Duration::from_millis(100)).await;
                    }

                    tracing::debug!("下载任务已删除");
                    return;
                }

                _ = state_receiver.changed() => {
                    self.handle_state_change(&mut permit, &mut state_receiver).await;
                }

                () = download_task, if state_is_downloading && permit.is_some() => {
                    download_task_option = None;
                    if let Some(permit) = permit.take() {
                        drop(permit);
                    }
                }

                () = self.acquire_task_permit(&mut permit), if state_is_pending => {},
            }
        }
    }

    #[instrument(level = "error", skip_all)]
    async fn download(self: &Arc<Self>) {
        let mut progress = self.progress.read().clone();

        if progress.is_completed() && progress.last_error.is_none() {
            tracing::info!("跳过下载，因为下载任务已完成");
            self.set_state(DownloadTaskState::Completed);
            return;
        }

        progress.last_error = None;
        *self.last_error.write() = None;
        self.update_progress(|p| p.last_error = None);
        tracing::debug!("开始下载");
        if let Err(err) = progress
            .process(self)
            .await
            .wrap_err("[继续]失败的任务可以断点续传")
        {
            if err.is::<DownloadInterrupted>() {
                return;
            }
            let err_title = "下载失败";
            let message = err.to_message();
            tracing::error!(err_title, message);
            *self.last_error.write() = Some(message.clone());
            self.update_progress(|p| p.last_error = Some(message));

            self.set_state(DownloadTaskState::Failed);

            return;
        }

        self.sleep_between_task().await;

        self.set_state(DownloadTaskState::Completed);
        tracing::info!("下载成功");
    }

    async fn sleep_between_task(&self) {
        let task_id = &self.task_id;
        let mut remaining_sec = self.app.get_config().read().task_download_interval_sec;
        while remaining_sec > 0 {
            // 发送章节休眠事件
            let _ = DownloadEvent::TaskSleeping {
                task_id: task_id.clone(),
                remaining_sec,
            }
            .emit(&self.app);
            sleep(Duration::from_secs(1)).await;
            remaining_sec -= 1;
        }
    }

    #[instrument(level = "error", skip_all)]
    async fn acquire_task_permit(&self, permit: &mut Option<OwnedSemaphorePermit>) {
        *permit = match permit.take() {
            // 如果有permit，则直接用
            Some(permit) => Some(permit),
            // 如果没有permit，则获取permit
            None => match self
                .app
                .get_download_manager()
                .task_sem
                .clone()
                .acquire_owned()
                .await
                .map_err(eyre::Report::from)
            {
                Ok(permit) => Some(permit),
                Err(err) => {
                    let err_title = "获取下载任务的permit失败";
                    let message = err.to_message();
                    tracing::error!(err_title, message);

                    self.set_state(DownloadTaskState::Failed);

                    return;
                }
            },
        };
        // 如果当前任务状态不是`Pending`，则不将任务状态设置为`Downloading`
        if *self.state_sender.borrow() != DownloadTaskState::Pending {
            return;
        }
        // 将任务状态设置为`Downloading`
        self.state_sender
            .send_replace(DownloadTaskState::Downloading);
    }

    #[instrument(level = "error", skip_all)]
    async fn handle_state_change(
        &self,
        permit: &mut Option<OwnedSemaphorePermit>,
        state_receiver: &mut watch::Receiver<DownloadTaskState>,
    ) {
        let state = *state_receiver.borrow();
        let task_id = self.task_id.clone();
        let _ = DownloadEvent::TaskStateUpdate { task_id, state }.emit(&self.app);

        if state == DownloadTaskState::Paused {
            // 稍微等一下再释放permit
            // 避免大批量暂停时，本应暂停的任务因拿到permit而稍微下载一小段(虽然最终会被暂停)
            sleep(Duration::from_millis(100)).await;
            tracing::debug!("下载任务已暂停");
            if let Some(permit) = permit.take() {
                drop(permit);
            }
        }
    }

    #[instrument(level = "error", skip_all)]
    fn handle_restart_notify(&self) {
        self.update_progress(|p| {
            p.mark_uncompleted();
        });
        self.set_state(DownloadTaskState::Pending);
    }

    #[instrument(
        level = "error",
        skip_all,
        fields(
            task_id = self.trace_fields.task_id,
            episode_type = ?self.trace_fields.episode_type,
            aid = self.trace_fields.aid,
            bvid = self.trace_fields.bvid,
            cid = self.trace_fields.cid,
            ep_id = self.trace_fields.ep_id,
            collection_title = self.trace_fields.collection_title,
            episode_title = self.trace_fields.episode_title,
            episode_order = self.trace_fields.episode_order,
            part_title = self.trace_fields.part_title,
            part_order = self.trace_fields.part_order,
            up_name = self.trace_fields.up_name,
            up_uid = self.trace_fields.up_uid,
        )
    )]
    pub fn set_state(&self, state: DownloadTaskState) {
        self.state_sender.send_replace(state);
    }

    pub(crate) fn track_chunk(&self) -> ChunkGuard {
        self.chunk_count.send_modify(|count| *count += 1);
        ChunkGuard(self.chunk_count.clone())
    }

    pub async fn shutdown(&self) {
        self.cancel_sender.send_replace(());
        let handle = self.process_handle.lock().take();
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }
        let mut chunks = self.chunk_count.subscribe();
        loop {
            if *chunks.borrow_and_update() == 0 {
                break;
            }
            if chunks.changed().await.is_err() {
                break;
            }
        }
        if matches!(
            *self.state_sender.borrow(),
            DownloadTaskState::Downloading | DownloadTaskState::Pending
        ) {
            self.set_state(DownloadTaskState::Paused);
        }
    }

    #[instrument(level = "error", skip_all)]
    pub fn update_progress(&self, update_fn: impl FnOnce(&mut DownloadProgress)) {
        // 修改数据
        let updated_progress = {
            let mut progress = self.progress.write();
            update_fn(&mut progress);
            // TODO: 这里应该返回 progress.clone()
            // 专门用一个 {} 框出来就是为了避免在emit和save期间仍持有写锁
            // 然而这里弄错了progress的类型
            // 错把progress当成了DownloadProgress，实则类型为RwLockWriteGuard
            progress
        };
        // 发送更新事件并保存到文件
        let _ = DownloadEvent::ProgressUpdate {
            progress: updated_progress.clone(),
        }
        .emit(&self.app);

        if let Err(err) = updated_progress.save(&self.app, false) {
            let err_title = "保存下载进度到文件失败";
            let message = err.to_message();
            tracing::error!(err_title, message);
        }
    }
}

pub struct DownloadTaskTraceFields {
    pub task_id: String,
    pub episode_type: EpisodeType,
    pub aid: i64,
    pub bvid: Option<String>,
    pub cid: i64,
    pub ep_id: Option<i64>,
    pub collection_title: String,
    pub episode_title: String,
    pub episode_order: i64,
    pub part_title: Option<String>,
    pub part_order: Option<i64>,
    pub up_name: Option<String>,
    pub up_uid: Option<i64>,
}

impl From<&DownloadProgress> for DownloadTaskTraceFields {
    fn from(progress: &DownloadProgress) -> Self {
        Self {
            task_id: progress.task_id.clone(),
            episode_type: progress.episode_type,
            aid: progress.aid,
            bvid: progress.bvid.clone(),
            cid: progress.cid,
            ep_id: progress.ep_id,
            collection_title: progress.collection_title.clone(),
            episode_title: progress.episode_title.clone(),
            episode_order: progress.episode_order,
            part_title: progress.part_title.clone(),
            part_order: progress.part_order,
            up_name: progress.up_name.clone(),
            up_uid: progress.up_uid,
        }
    }
}

fn task_context(app: &AppContext, progress: &DownloadProgress) -> AppContext {
    let mut config = app.get_config().read().clone();
    if let Some(options) = &progress.options {
        options.apply(&mut config);
    }
    app.with_config(config)
}

pub(crate) struct ChunkGuard(watch::Sender<usize>);
impl Drop for ChunkGuard {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
}
