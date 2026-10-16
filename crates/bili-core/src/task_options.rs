use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{
    config::{Config, FileExistAction},
    types::{audio_quality::AudioQuality, codec_type::CodecType, video_quality::VideoQuality},
};

/// The non-secret choices that belong to a task and must survive a later resume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct DownloadOptions {
    pub download_dir: PathBuf,
    pub dir_fmt: String,
    pub dir_fmt_for_part: String,
    pub time_fmt: String,
    pub video_quality_priority: Vec<VideoQuality>,
    pub codec_type_priority: Vec<CodecType>,
    pub audio_quality_priority: Vec<AudioQuality>,
    pub file_exist_action: FileExistAction,
}

impl From<&Config> for DownloadOptions {
    fn from(config: &Config) -> Self {
        Self {
            download_dir: config.download_dir.clone(),
            dir_fmt: config.dir_fmt.clone(),
            dir_fmt_for_part: config.dir_fmt_for_part.clone(),
            time_fmt: config.time_fmt.clone(),
            video_quality_priority: config.video_quality_priority.clone(),
            codec_type_priority: config.codec_type_priority.clone(),
            audio_quality_priority: config.audio_quality_priority.clone(),
            file_exist_action: config.file_exist_action,
        }
    }
}

impl DownloadOptions {
    /// Apply these choices to a config snapshot; credentials and runtime settings remain global.
    pub fn apply(&self, config: &mut Config) {
        config.download_dir.clone_from(&self.download_dir);
        config.dir_fmt.clone_from(&self.dir_fmt);
        config.dir_fmt_for_part.clone_from(&self.dir_fmt_for_part);
        config.time_fmt.clone_from(&self.time_fmt);
        config
            .video_quality_priority
            .clone_from(&self.video_quality_priority);
        config
            .codec_type_priority
            .clone_from(&self.codec_type_priority);
        config
            .audio_quality_priority
            .clone_from(&self.audio_quality_priority);
        config.file_exist_action = self.file_exist_action;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_options_round_trip_without_credentials() {
        let mut config = Config::defaults(std::path::Path::new("/tmp/bili"));
        config.sessdata = "test-secret-session".into();
        config.proxy_host = "test-private-proxy".into();
        let options = DownloadOptions::from(&config);
        let value = serde_json::to_value(&options).unwrap();
        assert!(value.get("sessdata").is_none());
        assert!(value.get("proxy_host").is_none());
        assert!(value.get("ffmpeg_path").is_none());
        let restored: DownloadOptions = serde_json::from_value(value).unwrap();
        assert_eq!(restored, options);
    }

    #[test]
    fn apply_restores_task_paths_and_quality_without_changing_global_settings() {
        let mut original = Config::defaults(std::path::Path::new("/tmp/old"));
        original.download_dir = PathBuf::from("/tmp/custom-output");
        original.dir_fmt = "{episode_title}".into();
        original.dir_fmt_for_part = "P{part_order}/{part_title}".into();
        original.time_fmt = "%Y%m%d".into();
        original.video_quality_priority = vec![VideoQuality::Video720P, VideoQuality::Video480P];
        original.codec_type_priority = vec![CodecType::AV1, CodecType::HEVC];
        original.audio_quality_priority = vec![AudioQuality::Audio64K];
        original.file_exist_action = FileExistAction::Skip;
        let options = DownloadOptions::from(&original);

        let mut current = Config::defaults(std::path::Path::new("/tmp/new"));
        current.sessdata = "current-session".into();
        current.task_concurrency = 9;
        current.ffmpeg_path = Some(PathBuf::from("/usr/local/bin/ffmpeg"));
        options.apply(&mut current);

        assert_eq!(DownloadOptions::from(&current), options);
        assert_eq!(current.sessdata, "current-session");
        assert_eq!(current.task_concurrency, 9);
        assert_eq!(
            current.ffmpeg_path,
            Some(PathBuf::from("/usr/local/bin/ffmpeg"))
        );
    }
}
