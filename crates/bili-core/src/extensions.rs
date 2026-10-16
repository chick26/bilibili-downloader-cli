use eyre::WrapErr;
use tracing::instrument;

use crate::{
    downloader::download_progress::DownloadProgress, runtime::AppContext,
    types::player_info::PlayerInfo,
};

pub trait EyreReportToMessage {
    fn to_message(&self) -> String;
}

impl EyreReportToMessage for eyre::Report {
    fn to_message(&self) -> String {
        format!("{self:?}")
    }
}

pub(crate) trait GetOrInitPlayerInfo {
    async fn get_or_init<'a>(
        &'a mut self,
        app: &AppContext,
        progress: &DownloadProgress,
    ) -> eyre::Result<&'a mut PlayerInfo>;
}

impl GetOrInitPlayerInfo for Option<PlayerInfo> {
    #[instrument(level = "error", skip_all)]
    async fn get_or_init<'a>(
        &'a mut self,
        app: &AppContext,
        progress: &DownloadProgress,
    ) -> eyre::Result<&'a mut PlayerInfo> {
        if let Some(info) = self {
            return Ok(info);
        }

        let bili_client = app.get_bili_client();
        let info = bili_client
            .get_player_info(progress.aid, progress.cid)
            .await
            .wrap_err("获取播放器信息失败")?;

        Ok(self.insert(info))
    }
}
