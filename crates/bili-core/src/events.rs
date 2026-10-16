use serde::{Deserialize, Serialize};
use specta::Type;

use crate::downloader::{
    download_progress::DownloadProgress, download_task_state::DownloadTaskState,
};
use crate::types::plugin_info::PluginInfo;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    pub json_raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "event", content = "data")]
pub enum DownloadEvent {
    Speed {
        speed: String,
    },

    TaskCreate {
        state: DownloadTaskState,
        progress: DownloadProgress,
    },

    TaskStateUpdate {
        task_id: String,
        state: DownloadTaskState,
    },

    TaskSleeping {
        task_id: String,
        remaining_sec: u64,
    },

    TaskDelete {
        task_id: String,
    },

    ProgressPreparing {
        task_id: String,
    },

    ProgressUpdate {
        progress: DownloadProgress,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "event", content = "data")]
pub enum PluginEvent {
    Loaded { plugin_info: PluginInfo },
    Update { plugin_info: PluginInfo },
    Uninstall { plugin_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CoreEvent {
    Download(Box<DownloadEvent>),
    Plugin(PluginEvent),
    Log(LogEvent),
}

impl DownloadEvent {
    pub fn emit(&self, app: &crate::runtime::AppContext) -> eyre::Result<()> {
        app.emit(CoreEvent::Download(Box::new(self.clone())));
        Ok(())
    }
}

impl PluginEvent {
    pub fn emit(&self, app: &crate::runtime::AppContext) -> eyre::Result<()> {
        app.emit(CoreEvent::Plugin(self.clone()));
        Ok(())
    }
}

impl LogEvent {
    pub fn emit(&self, app: &crate::runtime::AppContext) -> eyre::Result<()> {
        app.emit(CoreEvent::Log(self.clone()));
        Ok(())
    }
}
