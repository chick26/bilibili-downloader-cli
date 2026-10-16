use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use crate::runtime::AppContext;
use eyre::{WrapErr, eyre};
use serde::{Deserialize, Serialize};
use specta::Type;
use tracing::instrument;

use crate::{
    downloader::{
        chapter_segments::{ChapterSegment, ChapterSegments},
        download_progress::DownloadProgress,
        download_task::DownloadTask,
    },
    extensions::GetOrInitPlayerInfo,
    types::player_info::PlayerInfo,
    utils,
};

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct VideoProcessTask {
    pub merge_selected: bool,
    pub embed_chapter_selected: bool,
    pub embed_skip_selected: bool,
    pub completed: bool,
    pub skipped: bool,
}

impl VideoProcessTask {
    pub fn mark_uncompleted(&mut self) {
        self.completed = false;
        self.skipped = false;
    }

    pub fn is_completed(&self) -> bool {
        !self.merge_selected && !self.embed_chapter_selected && !self.embed_skip_selected
            || self.completed
    }

    #[instrument(level = "error", skip_all)]
    pub async fn process(
        &self,
        download_task: &Arc<DownloadTask>,
        progress: &DownloadProgress,
        player_info: &mut Option<PlayerInfo>,
    ) -> eyre::Result<()> {
        let embed_selected = self.embed_chapter_selected || self.embed_skip_selected;

        if self.merge_selected && embed_selected {
            self.merge_and_embed(download_task, progress, player_info)
                .await
                .wrap_err("自动合并+嵌入章节元数据失败")?;
        } else if self.merge_selected {
            self.merge(download_task, progress)
                .await
                .wrap_err("自动合并失败")?;
        } else if embed_selected {
            self.embed(download_task, progress, player_info)
                .await
                .wrap_err("嵌入章节元数据失败")?;
        }

        Ok(())
    }

    #[instrument(level = "error", skip_all)]
    async fn merge_and_embed(
        &self,
        download_task: &Arc<DownloadTask>,
        progress: &DownloadProgress,
        player_info: &mut Option<PlayerInfo>,
    ) -> eyre::Result<()> {
        let (episode_dir, filename) = (&progress.episode_dir, &progress.filename);

        let ffmpeg_program =
            utils::get_ffmpeg_program(&download_task.app).wrap_err("获取FFmpeg程序路径失败")?;

        let video_path = episode_dir.join(format!("{filename}.mp4"));
        if !video_path.exists() {
            download_task.update_progress(|p| p.video_process_task.completed = true);
            return Ok(());
        }

        let audio_path = episode_dir.join(format!("{filename}.m4a"));
        if !audio_path.exists() {
            // 如果音频文件不存在，则只嵌入章节元数据
            self.embed(download_task, progress, player_info)
                .await
                .wrap_err("嵌入章节元数据失败")?;
            return Ok(());
        }

        let metadata_path = self
            .create_chapter_metadata(&download_task.app, progress, player_info)
            .await
            .wrap_err("创建章节元数据失败")?;

        let output_path = episode_dir.join(format!("{filename}-merged.mp4"));

        let mut command = tokio::process::Command::new(&ffmpeg_program);
        command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .arg("-nostdin");
        command.arg("-i").arg(&video_path);
        command.arg("-i").arg(&audio_path);
        if let Some(metadata_path) = &metadata_path {
            command.arg("-i").arg(metadata_path);
            command.arg("-map_metadata").arg("2");
        }
        command.arg("-c").arg("copy");
        command.arg("-map").arg("0:v:0");
        command.arg("-map").arg("1:a:0");
        command.arg(&output_path).arg("-y");
        #[cfg(target_os = "windows")]
        command.creation_flags(0x0800_0000);
        let output = command.output().await.wrap_err_with(|| {
            format!(
                "启动 FFmpeg ({}) 失败；请安装 FFmpeg 并加入 PATH，或用 --ffmpeg 指定可执行文件",
                ffmpeg_program.display()
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let err = eyre!(format!("STDOUT: {stdout}"))
                .wrap_err(format!("STDERR: {stderr}"))
                .wrap_err("原因可能是视频或音频文件损坏，建议[重来]试试");
            return Err(err);
        }

        publish_processed_output(&output_path, &video_path)?;
        cleanup_processed_input(&audio_path);

        if let Some(metadata_path) = metadata_path {
            cleanup_processed_input(&metadata_path);
        }

        download_task.update_progress(|p| p.video_process_task.completed = true);

        Ok(())
    }

    #[instrument(level = "error", skip_all)]
    async fn merge(
        &self,
        download_task: &Arc<DownloadTask>,
        progress: &DownloadProgress,
    ) -> eyre::Result<()> {
        let (episode_dir, filename) = (&progress.episode_dir, &progress.filename);

        let video_path = episode_dir.join(format!("{filename}.mp4"));
        if !video_path.exists() {
            download_task.update_progress(|p| p.video_process_task.completed = true);
            return Ok(());
        }

        let audio_path = episode_dir.join(format!("{filename}.m4a"));
        if !audio_path.exists() {
            download_task.update_progress(|p| p.video_process_task.completed = true);
            return Ok(());
        }

        let output_path = episode_dir.join(format!("{filename}-merged.mp4"));

        let ffmpeg_program =
            utils::get_ffmpeg_program(&download_task.app).wrap_err("获取FFmpeg程序路径失败")?;
        let mut command = tokio::process::Command::new(&ffmpeg_program);
        command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .arg("-nostdin");
        command.arg("-i").arg(&video_path);
        command.arg("-i").arg(&audio_path);
        command.arg("-c").arg("copy");
        command.arg("-map").arg("0:v:0");
        command.arg("-map").arg("1:a:0");
        command.arg(&output_path).arg("-y");
        #[cfg(target_os = "windows")]
        command.creation_flags(0x0800_0000);
        let output = command.output().await.wrap_err_with(|| {
            format!(
                "启动 FFmpeg ({}) 失败；请安装 FFmpeg 并加入 PATH，或用 --ffmpeg 指定可执行文件",
                ffmpeg_program.display()
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let err = eyre!(format!("STDOUT: {stdout}"))
                .wrap_err(format!("STDERR: {stderr}"))
                .wrap_err("原因可能是视频或音频文件损坏，建议[重来]试试");
            return Err(err);
        }

        publish_processed_output(&output_path, &video_path)?;
        cleanup_processed_input(&audio_path);

        download_task.update_progress(|p| p.video_process_task.completed = true);

        Ok(())
    }

    #[instrument(level = "error", skip_all)]
    async fn embed(
        &self,
        download_task: &Arc<DownloadTask>,
        progress: &DownloadProgress,
        player_info: &mut Option<PlayerInfo>,
    ) -> eyre::Result<()> {
        let (episode_dir, filename) = (&progress.episode_dir, &progress.filename);

        let ffmpeg_program =
            utils::get_ffmpeg_program(&download_task.app).wrap_err("获取FFmpeg程序路径失败")?;

        let video_path = episode_dir.join(format!("{filename}.mp4"));
        if !video_path.exists() {
            download_task.update_progress(|p| p.video_process_task.completed = true);
            return Ok(());
        }

        let output_path = episode_dir.join(format!("{filename}-embed.mp4"));

        let metadata_path = self
            .create_chapter_metadata(&download_task.app, progress, player_info)
            .await
            .wrap_err("创建章节元数据失败")?;

        let Some(metadata_path) = metadata_path else {
            download_task.update_progress(|p| p.video_process_task.completed = true);
            return Ok(());
        };

        let mut command = tokio::process::Command::new(&ffmpeg_program);
        command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .arg("-nostdin");
        command.arg("-i").arg(&video_path);
        command.arg("-i").arg(&metadata_path);
        command.arg("-map_metadata").arg("1");
        command.arg("-c").arg("copy");
        command.arg(&output_path).arg("-y");
        #[cfg(target_os = "windows")]
        command.creation_flags(0x0800_0000);
        let output = command.output().await.wrap_err_with(|| {
            format!(
                "启动 FFmpeg ({}) 失败；请安装 FFmpeg 并加入 PATH，或用 --ffmpeg 指定可执行文件",
                ffmpeg_program.display()
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let err = eyre!(format!("STDOUT: {stdout}"))
                .wrap_err(format!("STDERR: {stderr}"))
                .wrap_err("原因可能是视频或音频文件损坏，建议[重来]试试");
            return Err(err);
        }

        publish_processed_output(&output_path, &video_path)?;
        cleanup_processed_input(&metadata_path);

        download_task.update_progress(|p| p.video_process_task.completed = true);

        Ok(())
    }

    #[instrument(level = "error", skip_all)]
    async fn create_chapter_metadata(
        &self,
        app: &AppContext,
        progress: &DownloadProgress,
        player_info: &mut Option<PlayerInfo>,
    ) -> eyre::Result<Option<PathBuf>> {
        let mut chapter_segments = ChapterSegments {
            segments: Vec::new(),
        };

        if self.embed_chapter_selected {
            let player_info = player_info.get_or_init(app, progress).await?;
            let segments = player_info
                .view_points
                .iter()
                .map(|vp| ChapterSegment {
                    title: vp.content.clone(),
                    start: vp.from,
                    end: vp.to,
                })
                .collect();
            chapter_segments = ChapterSegments { segments };
        }

        if let (true, Some(bvid)) = (self.embed_skip_selected, &progress.bvid) {
            let bili_client = app.get_bili_client();
            let cid = Some(progress.cid);

            let skip_segments = bili_client.get_skip_segments(bvid, cid).await?;
            for segment in skip_segments.0 {
                if let Some(chapter_segment) = segment.into_chapter_segment() {
                    chapter_segments.insert(chapter_segment);
                }
            }
        }

        if chapter_segments.segments.is_empty() {
            return Ok(None);
        }

        let metadata_content = chapter_segments.generate_chapter_metadata(progress.duration);
        let (episode_dir, filename) = (&progress.episode_dir, &progress.filename);
        let metadata_path = episode_dir.join(format!("{filename}.FFMETA.ini"));
        std::fs::write(&metadata_path, metadata_content)
            .wrap_err(format!("保存章节元数据到`{}`失败", metadata_path.display()))?;

        Ok(Some(metadata_path))
    }
}

// Replace the video only after FFmpeg succeeds. A failed publish leaves both
// original inputs intact; a failed cleanup must not invalidate the merged output.
fn publish_processed_output(output: &Path, video: &Path) -> eyre::Result<()> {
    std::fs::rename(output, video)
        .wrap_err_with(|| format!("保存处理后的视频到 `{}` 失败", video.display()))
}

fn cleanup_processed_input(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        tracing::warn!(path = %path.display(), %error, "视频已保存，清理中间文件失败");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::{config::Config, runtime::Runtime};

    #[test]
    fn publication_survives_cleanup_failure_and_failed_publication_preserves_input() {
        let dir = std::env::temp_dir().join(format!("bili-publish-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("video.mp4");
        let output = dir.join("merged.mp4");
        let audio = dir.join("audio.m4a");
        std::fs::write(&video, b"original").unwrap();
        assert!(publish_processed_output(&output, &video).is_err());
        assert_eq!(std::fs::read(&video).unwrap(), b"original");
        std::fs::write(&output, b"merged").unwrap();
        // A directory deterministically simulates an input that cannot be unlinked as a file.
        std::fs::create_dir(&audio).unwrap();
        publish_processed_output(&output, &video).unwrap();
        cleanup_processed_input(&audio);
        assert_eq!(std::fs::read(&video).unwrap(), b"merged");
        assert!(audio.exists());
        assert!(!output.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn plain_merge_replaces_inputs_and_persists_completion() {
        let test_dir =
            std::env::temp_dir().join(format!("bili-merge-test-{}", uuid::Uuid::new_v4()));
        let media_dir = test_dir.join("media with spaces");
        std::fs::create_dir_all(&media_dir).unwrap();
        let ffmpeg = test_dir.join("fake-ffmpeg");
        std::fs::write(
            &ffmpeg,
            r##"#!/bin/sh
[ "$1" = "-nostdin" ] || exit 31
for arg in "$@"; do
    case "$arg" in
        *-merged.mp4) printf 'merged video and audio' > "$arg" || exit 32 ;;
    esac
done
"##,
        )
        .unwrap();
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = Config::defaults(&test_dir);
        config.ffmpeg_path = Some(ffmpeg);
        let runtime = Runtime::new(test_dir.clone(), config).unwrap();
        let context = runtime.context();
        let process = VideoProcessTask {
            merge_selected: true,
            ..Default::default()
        };
        let progress = DownloadProgress {
            task_id: "plain-merge".to_owned(),
            episode_dir: media_dir.clone(),
            filename: "video".to_owned(),
            video_process_task: process.clone(),
            ..Default::default()
        };
        progress.save(&context, true).unwrap();
        std::fs::write(media_dir.join("video.mp4"), b"video input").unwrap();
        std::fs::write(media_dir.join("video.m4a"), b"audio input").unwrap();
        let task = DownloadTask::from_progress(context, progress.clone());

        process.process(&task, &progress, &mut None).await.unwrap();

        assert_eq!(
            std::fs::read(media_dir.join("video.mp4")).unwrap(),
            b"merged video and audio"
        );
        assert!(!media_dir.join("video.m4a").exists());
        assert!(!media_dir.join("video-merged.mp4").exists());
        assert!(task.progress.read().video_process_task.completed);
        let saved: DownloadProgress = serde_json::from_str(
            &std::fs::read_to_string(test_dir.join(".下载任务/plain-merge.json")).unwrap(),
        )
        .unwrap();
        assert!(saved.video_process_task.completed);

        task.shutdown().await;
        runtime.shutdown().await.unwrap();
        drop(task);
        drop(runtime);
        std::fs::remove_dir_all(test_dir).unwrap();
    }
}
