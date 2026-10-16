use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::runtime::AppContext;
use eyre::{WrapErr, eyre};
use parking_lot::{Mutex, RwLock};
use tokio::sync::Semaphore;
use tracing::instrument;

use crate::{
    events::DownloadEvent,
    extensions::EyreReportToMessage,
    types::{
        create_download_task_params::CreateDownloadTaskParams,
        restart_download_task_params::RestartDownloadTaskParams,
    },
};

use super::{
    download_progress::DownloadProgress, download_task::DownloadTask,
    download_task_state::DownloadTaskState,
};

pub struct DownloadManager {
    pub app: AppContext,
    pub task_sem: Arc<Semaphore>,
    pub media_chunk_sem: Arc<Semaphore>,
    pub byte_per_sec: Arc<AtomicU64>,
    pub download_tasks: RwLock<HashMap<String, Arc<DownloadTask>>>,
    output_paths: Mutex<HashMap<PathBuf, String>>,
}

impl DownloadManager {
    pub fn new(app: AppContext) -> Self {
        let (task_concurrency, chunk_concurrency) = {
            let config = app.get_config().inner().read();
            (config.task_concurrency, config.chunk_concurrency)
        };

        let manager = Self {
            app,
            task_sem: Arc::new(Semaphore::new(task_concurrency)),
            media_chunk_sem: Arc::new(Semaphore::new(chunk_concurrency)),
            byte_per_sec: Arc::new(AtomicU64::new(0)),
            download_tasks: RwLock::new(HashMap::new()),
            output_paths: Mutex::new(HashMap::new()),
        };

        tokio::spawn(Self::emit_download_speed_loop(
            manager.app.clone(),
            manager.byte_per_sec.clone(),
        ));

        manager
    }

    pub(crate) fn reserve_output(&self, progress: &DownloadProgress) -> eyre::Result<()> {
        std::fs::create_dir_all(&progress.episode_dir)?;
        let path = std::fs::canonicalize(&progress.episode_dir)?.join(&progress.filename);
        let mut paths = self.output_paths.lock();
        if let Some(owner) = paths.get(&path) {
            if owner != &progress.task_id {
                return Err(eyre!(
                    "输出路径与任务 {owner} 冲突：{}；请使用不同输出目录或在命名模板中加入 {{aid}}、{{cid}}",
                    path.display()
                ));
            }
        } else {
            paths.insert(path, progress.task_id.clone());
        }
        Ok(())
    }

    #[instrument(level = "error", skip_all)]
    pub fn restore_download_tasks(&self) -> eyre::Result<()> {
        let task_dir = self.get_task_dir()?;
        std::fs::create_dir_all(&task_dir)
            .wrap_err(format!("创建下载任务目录`{}`失败", task_dir.display()))?;

        let mut tasks = self.download_tasks.write();
        for entry in std::fs::read_dir(&task_dir)?.filter_map(Result::ok) {
            let path = entry.path();
            let extension = path.extension().and_then(|s| s.to_str());
            if extension != Some("json") {
                continue;
            }

            let progress_json = std::fs::read_to_string(&path)?;

            let progress: DownloadProgress = serde_json::from_str(&progress_json)
                .wrap_err_with(|| format!("读取下载任务失败: {}", path.display()))?;

            let new_task = DownloadTask::from_progress(self.app.clone(), progress);
            let old_task = tasks.insert(new_task.task_id.clone(), new_task);
            if let Some(old_task) = old_task {
                // 如果同一个ID的下载任务已经存在，则取消旧的任务
                let _ = old_task.cancel_sender.send(());
            }
        }

        Ok(())
    }

    pub fn create_download_tasks(
        &self,
        params: &CreateDownloadTaskParams,
    ) -> eyre::Result<Vec<String>> {
        let new_tasks = DownloadTask::from_params(&self.app, params)?;
        if new_tasks.is_empty() {
            return Err(eyre!("没有可创建的下载任务"));
        }
        let ids = new_tasks.iter().map(|task| task.task_id.clone()).collect();
        let mut tasks = self.download_tasks.write();
        for new_task in new_tasks {
            tasks.insert(new_task.task_id.clone(), new_task);
        }
        Ok(ids)
    }

    #[instrument(level = "error", skip_all)]
    pub fn pause_download_tasks(&self, task_ids: &Vec<String>) {
        let tasks = self.download_tasks.read();
        for task_id in task_ids {
            let span = tracing::error_span!("pause_download_task", task_id = task_id);
            let _enter = span.enter();

            let Some(task) = tasks.get(task_id) else {
                let err = eyre!("未找到ID对应的下载任务");
                let err_title = "暂停下载任务失败";
                let message = err.to_message();
                tracing::error!(err_title, message);
                continue;
            };
            task.set_state(DownloadTaskState::Paused);
            tracing::debug!("已将ID对应的下载任务状态设置为`Paused`");
        }
    }

    #[instrument(level = "error", skip_all)]
    pub fn resume_download_tasks(&self, task_ids: &Vec<String>) {
        let tasks = self.download_tasks.read();
        for task_id in task_ids {
            let span = tracing::error_span!("resume_download_task", task_id = task_id);
            let _enter = span.enter();

            let Some(task) = tasks.get(task_id) else {
                let err = eyre!("未找到ID对应的下载任务");
                let err_title = "继续下载任务失败";
                let message = err.to_message();
                tracing::error!(err_title, message);
                continue;
            };
            task.set_state(DownloadTaskState::Pending);
            tracing::debug!("已将ID对应的下载任务状态设置为`Pending`");
        }
    }

    #[instrument(level = "error", skip_all)]
    pub fn delete_download_tasks(&self, task_ids: &Vec<String>) {
        let mut tasks = self.download_tasks.write();
        for task_id in task_ids {
            let span = tracing::error_span!("delete_download_task", task_id = task_id);
            let _enter = span.enter();

            let Some(task) = tasks.remove(task_id) else {
                let err = eyre!("未找到ID对应的下载任务");
                let err_title = "删除下载任务失败";
                let message = err.to_message();
                tracing::error!(err_title, message);
                continue;
            };
            // TODO: 应该先发删除新号再删文件
            // 因为发信号失败会把任务重新塞回去
            // 目前先删文件会导致发信号失败时出现 任务还在但文件没了的情况
            if let Err(err) = self.delete_progress_file(task_id) {
                let err_title = "删除下载任务失败";
                let message = err.to_message();
                tracing::error!(err_title, message);
                tasks.insert(task_id.clone(), task);
                continue;
            }

            if let Err(err) = task.delete_sender.send(()).map_err(eyre::Report::from) {
                let err = err.wrap_err("通知ID对应的下载任务删除失败");
                let err_title = "删除下载任务失败";
                let message = err.to_message();
                tracing::error!(err_title, message);
                tasks.insert(task_id.clone(), task);
                continue;
            }

            tracing::debug!("已通知ID对应的下载任务删除");
        }
    }

    #[instrument(level = "error", skip_all)]
    pub fn restart_download_tasks(&self, task_ids: &Vec<String>) {
        let tasks = self.download_tasks.read();
        for task_id in task_ids {
            let span = tracing::error_span!("restart_download_task", task_id = task_id);
            let _enter = span.enter();

            let Some(task) = tasks.get(task_id) else {
                let err = eyre!("未找到ID对应的下载任务");
                let err_title = "重来下载任务失败";
                let message = err.to_message();
                tracing::error!(err_title, message);
                continue;
            };

            if let Err(err) = task.restart_sender.send(()).map_err(eyre::Report::from) {
                let err_title = "重来下载任务失败";
                let err = err.wrap_err("通知ID对应的下载任务重来失败");
                let message = err.to_message();
                tracing::error!(err_title, message);
                continue;
            }

            tracing::debug!("已通知ID对应的下载任务重来");
        }
    }

    #[instrument(level = "error", skip_all, fields(task_id = params.task_id))]
    pub fn restart_download_task(&self, params: &RestartDownloadTaskParams) {
        let task_id = &params.task_id;

        let tasks = self.download_tasks.read();
        let Some(task) = tasks.get(task_id) else {
            let err = eyre!("未找到ID对应的下载任务");
            let err_title = "重来下载任务失败";
            let message = err.to_message();
            tracing::error!(err_title, message);
            return;
        };

        // TODO: 把这块代码变成DownloadProgress的mark_restart函数
        {
            let mut progress = task.progress.write();

            progress.video_task.selected = params.video_task_selected;
            progress.audio_task.selected = params.audio_task_selected;
            progress.video_process_task.merge_selected = params.merge_selected;
            progress.video_process_task.embed_chapter_selected = params.embed_chapter_selected;
            progress.video_process_task.embed_skip_selected = params.embed_skip_selected;
            progress.subtitle_task.selected = params.subtitle_task_selected;
            progress.danmaku_task.xml_selected = params.xml_danmaku_selected;
            progress.danmaku_task.ass_selected = params.ass_danmaku_selected;
            progress.danmaku_task.json_selected = params.json_danmaku_selected;
            progress.cover_task.selected = params.cover_task_selected;
            progress.nfo_task.selected = params.nfo_task_selected;
            progress.json_task.selected = params.json_task_selected;

            progress.video_task.video_quality = params.video_quality;
            progress.video_task.codec_type = params.codec_type;
            progress.audio_task.audio_quality = params.audio_quality;
        }

        if let Err(err) = task.restart_sender.send(()).map_err(eyre::Report::from) {
            let err_title = "重来下载任务失败";
            let err = err.wrap_err("通知ID对应的下载任务重来失败");
            let message = err.to_message();
            tracing::error!(err_title, message);
            return;
        }

        tracing::debug!("已通知ID对应的下载任务重来");
    }

    pub fn flush_progress(&self) -> eyre::Result<()> {
        for task in self.download_tasks.read().values() {
            task.progress.read().save(&self.app, false)?;
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> eyre::Result<()> {
        let tasks: Vec<_> = self.download_tasks.read().values().cloned().collect();
        for task in &tasks {
            task.shutdown().await;
        }
        self.flush_progress()
    }

    async fn emit_download_speed_loop(app: AppContext, byte_per_sec: Arc<AtomicU64>) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        while app.is_active() {
            interval.tick().await;
            let byte_per_sec = byte_per_sec.swap(0, Ordering::Relaxed);
            #[allow(clippy::cast_precision_loss)]
            let mega_byte_per_sec = byte_per_sec as f64 / 1024.0 / 1024.0;
            let speed = format!("{mega_byte_per_sec:.2}MB/s");
            let _ = DownloadEvent::Speed { speed }.emit(&app);
        }
    }

    #[instrument(level = "error", skip_all)]
    fn get_task_dir(&self) -> eyre::Result<PathBuf> {
        let app_data_dir = self.app.path().app_data_dir()?;
        let task_dir = app_data_dir.join(".下载任务");
        Ok(task_dir)
    }

    #[instrument(level = "error", skip_all, fields(task_id = task_id))]
    fn delete_progress_file(&self, task_id: &str) -> eyre::Result<()> {
        let task_dir = self.get_task_dir()?;
        let task_file = task_dir.join(format!("{task_id}.json"));
        if task_file.exists() {
            std::fs::remove_file(task_file)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, runtime::Runtime};

    #[tokio::test]
    async fn different_tasks_cannot_reserve_same_output_or_alias_directory() {
        let dir = std::env::temp_dir().join(format!("bili-path-test-{}", uuid::Uuid::new_v4()));
        let runtime = Runtime::new(&dir, Config::defaults(&dir)).unwrap();
        let manager = runtime.context().get_download_manager();
        let first = DownloadProgress {
            task_id: "first".into(),
            episode_dir: dir.join("output"),
            filename: "same-title".into(),
            ..Default::default()
        };
        manager.reserve_output(&first).unwrap();
        manager.reserve_output(&first).unwrap();
        let mut second = first.clone();
        second.task_id = "second".into();
        second.episode_dir = second.episode_dir.join(".");
        assert!(manager.reserve_output(&second).is_err());
        second.filename = "different-title".into();
        manager.reserve_output(&second).unwrap();
        runtime.shutdown().await.unwrap();
        drop(manager);
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
