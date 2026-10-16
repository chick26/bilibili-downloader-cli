use std::{
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use specta::Type;
use tracing::instrument;

use crate::{
    danmaku_xml_to_ass::canvas::CanvasConfig,
    runtime::AppContext,
    types::{audio_quality::AudioQuality, codec_type::CodecType, video_quality::VideoQuality},
};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[allow(clippy::struct_excessive_bools)]
#[allow(clippy::struct_field_names)]
pub struct Config {
    pub download_dir: PathBuf,
    #[serde(default)]
    pub ffmpeg_path: Option<PathBuf>,
    pub enable_file_logger: bool,
    pub sessdata: String,
    pub video_quality_priority: Vec<VideoQuality>,
    pub codec_type_priority: Vec<CodecType>,
    pub audio_quality_priority: Vec<AudioQuality>,
    pub download_video: bool,
    pub download_audio: bool,
    pub auto_merge: bool,
    pub embed_chapter: bool,
    pub embed_skip: bool,
    pub download_xml_danmaku: bool,
    pub download_ass_danmaku: bool,
    pub download_json_danmaku: bool,
    pub download_subtitle: bool,
    pub download_cover: bool,
    pub download_nfo: bool,
    pub download_json: bool,
    pub dir_fmt: String,
    pub dir_fmt_for_part: String,
    pub time_fmt: String,
    pub proxy_mode: ProxyMode,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub task_concurrency: usize,
    pub task_download_interval_sec: u64,
    pub chunk_concurrency: usize,
    pub chunk_download_interval_sec: u64,
    pub danmaku_config: CanvasConfig,
    pub file_exist_action: FileExistAction,
    pub auto_start_download_task: bool,
}

impl Config {
    #[instrument(level = "error", skip_all)]
    pub fn load(app_data_dir: &Path) -> eyre::Result<Config> {
        let config_path = app_data_dir.join("config.json");

        let config = if config_path.exists() {
            let config_string = std::fs::read_to_string(config_path)?;
            match serde_json::from_str(&config_string) {
                // 如果能够直接解析为Config，则直接返回
                Ok(config) => config,
                // 否则，将默认配置与文件中已有的配置合并
                // 以免新版本添加了新的配置项，用户升级到新版本后，所有配置项都被重置
                Err(_) => Config::merge_config(&config_string, app_data_dir)?,
            }
        } else {
            Config::defaults(app_data_dir)
        };
        Ok(config)
    }

    #[instrument(level = "error", skip_all)]
    pub fn save(&self, app: &AppContext) -> eyre::Result<()> {
        let app_data_dir = app.path().app_data_dir()?;
        self.save_to(&app_data_dir)
    }

    pub fn save_to(&self, app_data_dir: &Path) -> eyre::Result<()> {
        std::fs::create_dir_all(app_data_dir)?;
        let config_path = app_data_dir.join("config.json");
        let config_string = serde_json::to_string_pretty(self)?;
        let temporary_path = app_data_dir.join("config.json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // An interrupted earlier save may have left a permissive temporary file.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.set_len(0)?;
        file.write_all(config_string.as_bytes())?;
        drop(file);
        std::fs::rename(temporary_path, config_path)?;
        Ok(())
    }

    fn merge_config(config_string: &str, app_data_dir: &Path) -> eyre::Result<Config> {
        let mut json_value = serde_json::from_str::<serde_json::Value>(config_string)?;
        let serde_json::Value::Object(ref mut map) = json_value else {
            return Err(eyre::eyre!("config.json 必须是 JSON 对象"));
        };
        let default_config_value = serde_json::to_value(Config::defaults(app_data_dir))?;
        let serde_json::Value::Object(default_map) = default_config_value else {
            unreachable!("Config serializes as an object");
        };
        for (key, value) in default_map {
            map.entry(key).or_insert(value);
        }
        Ok(serde_json::from_value(json_value)?)
    }

    pub fn defaults(app_data_dir: &Path) -> Config {
        const DEFAULT_FMT_FOR_PART: &str =
            "{collection_title}/{episode_title}/{episode_title}-P{part_order} {part_title}";
        let default_video_quality_priority = vec![
            VideoQuality::Video8K,
            VideoQuality::VideoDolby,
            VideoQuality::VideoHDR,
            VideoQuality::Video4K,
            VideoQuality::Video1080P60,
            VideoQuality::Video1080PPlus,
            VideoQuality::Video1080P,
            VideoQuality::VideoAiRepair,
            VideoQuality::Video720P60,
            VideoQuality::Video720P,
            VideoQuality::Video480P,
            VideoQuality::Video360P,
            VideoQuality::Video240P,
        ];
        let default_audio_quality_priority = vec![
            AudioQuality::AudioHiRes,
            AudioQuality::AudioDolby,
            AudioQuality::Audio192K,
            AudioQuality::Audio132K,
            AudioQuality::Audio64K,
        ];

        Config {
            download_dir: app_data_dir.join("视频下载"),
            ffmpeg_path: None,
            enable_file_logger: true,
            sessdata: String::new(),
            video_quality_priority: default_video_quality_priority,
            codec_type_priority: vec![CodecType::AVC, CodecType::HEVC, CodecType::AV1],
            audio_quality_priority: default_audio_quality_priority,
            download_video: true,
            download_audio: true,
            auto_merge: true,
            embed_chapter: true,
            embed_skip: true,
            download_xml_danmaku: true,
            download_ass_danmaku: true,
            download_json_danmaku: true,
            download_subtitle: true,
            download_cover: true,
            download_nfo: true,
            download_json: true,
            dir_fmt: "{collection_title}/{episode_title}".to_string(),
            dir_fmt_for_part: DEFAULT_FMT_FOR_PART.to_string(),
            time_fmt: "%Y-%m-%d_%H-%M-%S".to_string(),
            proxy_mode: ProxyMode::NoProxy,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 7890,
            task_concurrency: 3,
            task_download_interval_sec: 0,
            chunk_concurrency: 16,
            chunk_download_interval_sec: 0,
            danmaku_config: CanvasConfig::default(),
            file_exist_action: FileExistAction::Overwrite,
            auto_start_download_task: true,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
pub enum ProxyMode {
    #[default]
    NoProxy,
    System,
    Custom,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
pub enum FileExistAction {
    #[default]
    Overwrite,
    Skip,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn saved_login_config_is_private_even_after_replacing_permissive_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("bili-config-{}", uuid::Uuid::new_v4()));
        let mut config = Config::defaults(&dir);
        config.sessdata = "synthetic-session".into();
        config.save_to(&dir).unwrap();
        let path = dir.join("config.json");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let temporary_path = dir.join("config.json.tmp");
        std::fs::write(&temporary_path, b"old content").unwrap();
        std::fs::set_permissions(&temporary_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        config.save_to(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(Config::load(&dir).unwrap().sessdata, config.sessdata);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_merges_new_fields_without_changing_existing_config() {
        let dir = std::env::temp_dir().join(format!("bili-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let original = r#"{"download_dir":"/tmp/custom-videos","chunk_concurrency":7}"#;
        std::fs::write(&path, original).unwrap();

        let config = Config::load(&dir).unwrap();
        assert_eq!(config.download_dir, PathBuf::from("/tmp/custom-videos"));
        assert_eq!(config.chunk_concurrency, 7);
        assert!(config.ffmpeg_path.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_config_is_reported_and_preserved() {
        let dir = std::env::temp_dir().join(format!("bili-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let original = "{invalid json";
        std::fs::write(&path, original).unwrap();

        assert!(Config::load(&dir).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
