use std::{
    collections::VecDeque,
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};

use bili_core::{
    downloader::{download_task_state::DownloadTaskState, media_chunk::MediaChunk},
    service::TaskSnapshot,
};

use crate::format::{display_width, format_bytes, sanitize, terminal_width, truncate};

const LOG_INTERVAL: Duration = Duration::from_secs(5);
const SPEED_WINDOW: Duration = Duration::from_secs(5);

/// Human-readable progress stays on stderr so stdout remains a single JSON result.
pub struct DownloadProgressDisplay {
    terminal: bool,
    line_visible: bool,
    finished: bool,
    last_output: Option<Instant>,
    last_status: String,
    samples: VecDeque<(Instant, u64)>,
}

impl DownloadProgressDisplay {
    pub fn new(json_output: bool) -> Self {
        Self {
            terminal: !json_output
                && io::stderr().is_terminal()
                && std::env::var("TERM").is_ok_and(|term| term != "dumb"),
            line_visible: false,
            finished: false,
            last_output: None,
            last_status: String::new(),
            samples: VecDeque::new(),
        }
    }

    pub fn update(&mut self, tasks: &[TaskSnapshot]) {
        if self.finished || tasks.is_empty() {
            return;
        }
        let now = Instant::now();
        let (completed, total) = media_bytes(tasks);
        let speed = self.sample_speed(now, completed);
        let status = tasks
            .iter()
            .map(|task| format!("{}:{}", task.task_id, phase(task)))
            .collect::<Vec<_>>()
            .join(";");
        let due = self
            .last_output
            .is_none_or(|last| now.duration_since(last) >= LOG_INTERVAL);
        if !self.terminal && !due && status == self.last_status {
            return;
        }

        let width = if self.terminal {
            terminal_width().saturating_sub(1)
        } else {
            usize::MAX
        };
        let text = render(tasks, completed, total, speed, width);
        let mut stderr = io::stderr().lock();
        if self.terminal {
            let _ = write!(stderr, "\r\x1b[2K{text}");
            self.line_visible = true;
        } else {
            let _ = writeln!(stderr, "{text}");
        }
        let _ = stderr.flush();
        self.last_output = Some(now);
        self.last_status = status;
    }

    pub fn finish(&mut self) {
        if self.line_visible {
            let mut stderr = io::stderr().lock();
            let _ = write!(stderr, "\r\x1b[2K");
            let _ = stderr.flush();
            self.line_visible = false;
        }
        self.finished = true;
    }

    fn sample_speed(&mut self, now: Instant, completed: u64) -> Option<u64> {
        if self
            .samples
            .back()
            .is_some_and(|(_, bytes)| completed < *bytes)
        {
            // Restarted or re-prepared media can invalidate a previous bitmap.
            self.samples.clear();
        }
        self.samples.push_back((now, completed));
        while self.samples.len() > 2
            && self
                .samples
                .get(1)
                .is_some_and(|(time, _)| now.duration_since(*time) > SPEED_WINDOW)
        {
            self.samples.pop_front();
        }
        let (time, bytes) = self.samples.front()?;
        let elapsed = now.duration_since(*time).as_secs_f64();
        (elapsed >= 1.0).then(|| ((completed.saturating_sub(*bytes)) as f64 / elapsed) as u64)
    }
}

impl Drop for DownloadProgressDisplay {
    fn drop(&mut self) {
        self.finish();
    }
}

fn completed_bytes(chunks: &[MediaChunk], total: u64) -> u64 {
    chunks
        .iter()
        .filter(|chunk| chunk.completed)
        .filter_map(|chunk| chunk.end.checked_sub(chunk.start)?.checked_add(1))
        .fold(0, u64::saturating_add)
        .min(total)
}

fn media_bytes(tasks: &[TaskSnapshot]) -> (u64, u64) {
    tasks.iter().fold((0_u64, 0_u64), |(done, total), task| {
        let progress = &task.progress;
        let tracks = [
            (
                progress.video_task.selected,
                progress.video_task.content_length,
                progress.video_task.chunks.as_slice(),
            ),
            (
                progress.audio_task.selected,
                progress.audio_task.content_length,
                progress.audio_task.chunks.as_slice(),
            ),
        ];
        tracks
            .into_iter()
            .filter(|(selected, _, _)| *selected)
            .fold((done, total), |(done, total), (_, length, chunks)| {
                (
                    done.saturating_add(completed_bytes(chunks, length)),
                    total.saturating_add(length),
                )
            })
    })
}

fn phase(task: &TaskSnapshot) -> &'static str {
    match task.state {
        DownloadTaskState::Pending => "等待中",
        DownloadTaskState::Paused => "已暂停",
        DownloadTaskState::Completed => "已完成",
        DownloadTaskState::Failed => "失败",
        DownloadTaskState::Downloading => {
            let progress = &task.progress;
            if !progress.video_task.is_completed() && progress.video_task.content_length != 0 {
                if completed_bytes(
                    &progress.video_task.chunks,
                    progress.video_task.content_length,
                ) == progress.video_task.content_length
                {
                    "整理视频"
                } else {
                    "下载视频"
                }
            } else if !progress.audio_task.is_completed() && progress.audio_task.content_length != 0
            {
                if completed_bytes(
                    &progress.audio_task.chunks,
                    progress.audio_task.content_length,
                ) == progress.audio_task.content_length
                {
                    "整理音频"
                } else {
                    "下载音频"
                }
            } else if (!progress.video_task.is_completed()
                && progress.video_task.content_length == 0)
                || (!progress.audio_task.is_completed() && progress.audio_task.content_length == 0)
            {
                "准备下载"
            } else if !progress.video_process_task.is_completed() {
                "合并/处理"
            } else if !progress.danmaku_task.is_completed()
                || !progress.subtitle_task.is_completed()
                || !progress.cover_task.is_completed()
                || !progress.nfo_task.is_completed()
                || !progress.json_task.is_completed()
            {
                "附加文件"
            } else if progress.video_task.content_length == 0
                && progress.audio_task.content_length == 0
            {
                "准备下载"
            } else {
                "完成收尾"
            }
        }
    }
}

fn state_counts(tasks: &[TaskSnapshot]) -> String {
    let mut counts = [0; 5];
    for task in tasks {
        counts[match task.state {
            DownloadTaskState::Completed => 0,
            DownloadTaskState::Downloading => 1,
            DownloadTaskState::Pending => 2,
            DownloadTaskState::Paused => 3,
            DownloadTaskState::Failed => 4,
        }] += 1;
    }
    ["完成", "进行中", "等待", "暂停", "失败"]
        .into_iter()
        .zip(counts)
        .filter(|(_, count)| *count != 0)
        .map(|(label, count)| format!("{label} {count}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn render(
    tasks: &[TaskSnapshot],
    completed: u64,
    total: u64,
    speed: Option<u64>,
    width: usize,
) -> String {
    let fraction = if total == 0 {
        0.0
    } else {
        (completed as f64 / total as f64).min(1.0)
    };
    let progress = if total == 0 {
        "媒体大小待定".to_owned()
    } else {
        let bar_width = if width < 100 { 8 } else { 12 };
        let filled = (fraction * bar_width as f64).floor() as usize;
        format!(
            "[{}{}] {:5.1}% {}/{}",
            "=".repeat(filled),
            ".".repeat(bar_width - filled),
            fraction * 100.0,
            format_bytes(completed),
            format_bytes(total),
        )
    };
    let speed = speed.map_or_else(
        || "—".to_owned(),
        |rate| format!("{}/s", format_bytes(rate)),
    );
    let metrics = format!("{progress} · 分片速度 {speed}");
    let state = if tasks.len() == 1 {
        phase(&tasks[0]).to_owned()
    } else {
        format!("{} 项：{}", tasks.len(), state_counts(tasks))
    };
    let primary = format!("{state}  {metrics}");
    let active = tasks
        .iter()
        .find(|task| task.state == DownloadTaskState::Downloading)
        .unwrap_or(&tasks[0]);
    let title = sanitize(&active.progress.episode_title);
    let title = if let Some(part) = active.progress.part_order {
        format!("P{part} {title}")
    } else {
        title
    };
    if title.is_empty() {
        return truncate(&primary, width);
    }
    let remaining = width.saturating_sub(display_width(&primary).saturating_add(3));
    if remaining < 4 {
        truncate(&primary, width)
    } else {
        format!("{primary} · {}", truncate(&title, remaining))
    }
}

#[cfg(test)]
mod tests {
    use bili_core::downloader::download_progress::DownloadProgress;

    use super::*;

    fn task() -> TaskSnapshot {
        TaskSnapshot {
            task_id: "task-1".to_owned(),
            state: DownloadTaskState::Downloading,
            progress: DownloadProgress::default(),
            error: None,
        }
    }

    #[test]
    fn full_media_bytes_do_not_mean_the_task_is_finished() {
        let mut task = task();
        task.progress.video_task.selected = true;
        task.progress.video_task.content_length = 100;
        task.progress.video_task.completed = true;
        task.progress.video_task.chunks = vec![MediaChunk {
            start: 0,
            end: 99,
            completed: true,
        }];
        task.progress.video_process_task.merge_selected = true;
        assert_eq!(media_bytes(&[task.clone()]), (100, 100));
        assert_eq!(phase(&task), "合并/处理");
        let text = render(&[task.clone()], 100, 100, None, 160);
        assert!(text.contains("100.0%"));
        assert!(text.contains("合并/处理"));
        assert!(!text.contains("已完成"));
        task.state = DownloadTaskState::Completed;
        assert_eq!(phase(&task), "已完成");
    }

    #[test]
    fn media_without_known_sizes_is_preparing_before_merging() {
        let mut task = task();
        task.progress.video_task.selected = true;
        task.progress.audio_task.selected = true;
        task.progress.video_process_task.merge_selected = true;
        assert_eq!(phase(&task), "准备下载");
    }

    #[test]
    fn byte_totals_only_include_selected_valid_completed_chunks() {
        let mut task = task();
        task.progress.video_task.selected = true;
        task.progress.video_task.content_length = 100;
        task.progress.video_task.chunks = vec![
            MediaChunk {
                start: 0,
                end: 49,
                completed: true,
            },
            MediaChunk {
                start: 50,
                end: 99,
                completed: false,
            },
            MediaChunk {
                start: 9,
                end: 1,
                completed: true,
            },
        ];
        task.progress.audio_task.content_length = 200;
        task.progress.audio_task.chunks = vec![MediaChunk {
            start: 0,
            end: 199,
            completed: true,
        }];
        assert_eq!(media_bytes(&[task]), (50, 100));
    }

    #[test]
    fn render_is_bounded_and_contains_no_control_sequences_from_titles() {
        let mut task = task();
        task.progress.episode_title = "中文视频\n\u{1b}[31m标题".repeat(20);
        let text = render(&[task], 0, 100, None, 100);
        assert!(display_width(&text) <= 100);
        assert!(!text.contains('\n'));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn resumed_bytes_are_not_counted_as_initial_transfer_speed() {
        let mut display = DownloadProgressDisplay::new(true);
        let now = Instant::now();
        assert_eq!(display.sample_speed(now, 500_000), None);
        assert_eq!(
            display.sample_speed(now + Duration::from_secs(2), 500_100),
            Some(50)
        );
        assert_eq!(display.sample_speed(now + Duration::from_secs(3), 0), None);
    }
}
