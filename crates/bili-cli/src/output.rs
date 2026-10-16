use bili_core::{
    api_error::BiliApiError, downloader::media_chunk::MediaChunk, service::TaskSnapshot,
};
use serde_json::{Value, json};

pub struct Outcome {
    pub result: Value,
    pub exit_code: u8,
}

impl Outcome {
    pub fn success(result: Value) -> Self {
        Self {
            result,
            exit_code: 0,
        }
    }
}

#[derive(Debug)]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
    pub exit_code: u8,
}

impl CliError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code: 1,
        }
    }

    pub fn report(code: &'static str, error: eyre::Report) -> Self {
        if let Some(api_error) = error
            .downcast_ref::<BiliApiError>()
            .filter(|error| error.is_risk_control())
        {
            return Self::new(
                "BILIBILI_RISK_CONTROL",
                format!(
                    "{api_error}。请先运行 `bili auth status`，会话失效时运行 `bili auth login`；如网页要求验证，请人工完成后再试，或稍后重试。登录不保证解除风控，本次请求未自动重试。"
                ),
            );
        }
        Self::new(code, format!("{error:#}"))
    }

    pub fn interrupted() -> Self {
        Self {
            code: "INTERRUPTED",
            message: "Interrupted".into(),
            exit_code: 130,
        }
    }
}

pub fn emit_result(json_output: bool, outcome: &Outcome) {
    let result = &outcome.result;
    if json_output {
        let mut envelope = json!({"ok": outcome.exit_code == 0, "result": result});
        if outcome.exit_code != 0 {
            envelope["error"] = if outcome.exit_code == 130 {
                json!({"code": "INTERRUPTED", "message": "Progress saved; resume using the task ID"})
            } else {
                json!({"code": "DOWNLOAD_FAILED", "message": "One or more tasks did not complete; inspect task errors"})
            };
        }
        println!("{envelope}");
    } else {
        let text = crate::presentation::render_result(result, crate::format::terminal_width());
        if crate::format::stdout_color() {
            if let Some((title, rest)) = text.split_once('\n') {
                println!("\x1b[1;36m{title}\x1b[0m\n{rest}");
            } else {
                println!("\x1b[1;36m{text}\x1b[0m");
            }
        } else {
            println!("{text}");
        }
    }
}

pub fn emit_error(json_output: bool, err: &CliError) {
    if json_output {
        println!(
            "{}",
            json!({"ok": false, "error": {"code": err.code, "message": err.message}})
        );
    } else {
        let title = format!("错误 [{}]", err.code);
        let message = crate::format::wrap(&err.message, crate::format::terminal_width());
        if crate::format::stderr_color() {
            eprintln!("\x1b[1;31m{title}\x1b[0m\n{message}");
        } else {
            eprintln!("{title}\n{message}");
        }
    }
}

// Do not serialize raw DownloadProgress: it contains signed CDN URLs.
pub fn task_summary(task: &TaskSnapshot) -> Value {
    let progress = &task.progress;
    let video_bytes = completed_bytes(&progress.video_task.chunks);
    let audio_bytes = completed_bytes(&progress.audio_task.chunks);
    json!({
        "task_id": task.task_id,
        "state": task.state,
        "title": progress.episode_title,
        "collection": progress.collection_title,
        "part": progress.part_order,
        "part_title": progress.part_title,
        "bvid": progress.bvid,
        "aid": progress.aid,
        "cid": progress.cid,
        "ep_id": progress.ep_id,
        "output_dir": progress.episode_dir,
        "filename": progress.filename,
        "video": {
            "selected": progress.video_task.selected,
            "completed": progress.video_task.completed,
            "bytes_completed": video_bytes,
            "bytes_total": progress.video_task.content_length,
            "quality": progress.video_task.video_quality,
            "codec": progress.video_task.codec_type,
        },
        "audio": {
            "selected": progress.audio_task.selected,
            "completed": progress.audio_task.completed,
            "bytes_completed": audio_bytes,
            "bytes_total": progress.audio_task.content_length,
            "quality": progress.audio_task.audio_quality,
        },
        "created_at": progress.create_ts,
        "completed_at": progress.completed_ts,
        "error": task.error,
    })
}

fn completed_bytes(chunks: &[MediaChunk]) -> u64 {
    chunks
        .iter()
        .filter(|chunk| chunk.completed)
        .map(|chunk| {
            chunk
                .end
                .checked_sub(chunk.start)
                .and_then(|length| length.checked_add(1))
                .unwrap_or(0)
        })
        .fold(0, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_control_errors_have_a_stable_code_and_actionable_safe_message() {
        const ENDPOINT: &str = "/x/space/wbi/arc/search";
        for api_error in [
            BiliApiError::http(ENDPOINT, 412),
            BiliApiError::api(ENDPOINT, -352),
            BiliApiError::api(ENDPOINT, -412),
        ] {
            let error = eyre::Report::new(api_error)
                .wrap_err("query failed: <html>untrusted response body</html>");
            let error = CliError::report("DISCOVERY_FAILED", error);
            assert_eq!(error.code, "BILIBILI_RISK_CONTROL");
            assert_eq!(error.exit_code, 1);
            assert!(error.message.contains(ENDPOINT));
            assert!(error.message.contains("bili auth status"));
            assert!(error.message.contains("bili auth login"));
            assert!(error.message.contains("人工完成"));
            assert!(error.message.contains("稍后重试"));
            assert!(error.message.contains("登录不保证解除风控"));
            assert!(error.message.contains("未自动重试"));
            assert!(!error.message.contains("<html>"));
            assert!(!error.message.contains("untrusted response body"));
        }
    }

    #[test]
    fn other_errors_keep_their_original_code_and_context() {
        for api_error in [
            BiliApiError::http("/x/space/wbi/arc/search", 429),
            BiliApiError::http("/x/space/wbi/arc/search", 500),
            BiliApiError::api("/x/space/wbi/arc/search", -101),
        ] {
            let error = CliError::report(
                "DISCOVERY_FAILED",
                eyre::Report::new(api_error).wrap_err("UP query failed"),
            );
            assert_eq!(error.code, "DISCOVERY_FAILED");
            assert!(error.message.contains("UP query failed"));
        }
        let error = CliError::report("TASK_READ_FAILED", eyre::eyre!("record is unreadable"));
        assert_eq!(error.code, "TASK_READ_FAILED");
        assert_eq!(error.message, "record is unreadable");
    }

    #[test]
    fn summary_excludes_signed_media_urls() {
        let mut progress = bili_core::downloader::download_progress::DownloadProgress::default();
        progress.video_task.url = "https://cdn.invalid/video?secret=do-not-print".into();
        progress.audio_task.url = "https://cdn.invalid/audio?secret=do-not-print".into();
        let snapshot = TaskSnapshot {
            task_id: "test".into(),
            state: bili_core::downloader::download_task_state::DownloadTaskState::Paused,
            progress,
            error: None,
        };
        assert!(!task_summary(&snapshot).to_string().contains("do-not-print"));
    }

    #[test]
    fn completed_bytes_counts_only_complete_inclusive_ranges() {
        assert_eq!(
            completed_bytes(&[
                MediaChunk {
                    start: 0,
                    end: 9,
                    completed: true
                },
                MediaChunk {
                    start: 10,
                    end: 19,
                    completed: false
                },
                MediaChunk {
                    start: 20,
                    end: 24,
                    completed: true
                },
            ]),
            15
        );
    }
}
