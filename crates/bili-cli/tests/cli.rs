use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bili-cli-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bili"))
            .arg("--data-dir")
            .arg(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }

    fn run_text(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bili"))
            .arg("--data-dir")
            .arg(&self.0)
            .env("COLUMNS", "60")
            .env("NO_COLOR", "1")
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json_result(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout must contain exactly one JSON document: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn task_list_is_read_only_and_machine_readable() {
    let data = TempDir::new();
    let output = data.run(&["tasks", "list"]);
    assert!(output.status.success());
    let result = json_result(&output);
    assert_eq!(result["ok"], true);
    assert_eq!(result["result"]["tasks"], serde_json::json!([]));
    assert_eq!(fs::read_dir(&data.0).unwrap().count(), 0);
}

#[test]
fn default_output_is_readable_and_redirected_output_has_no_terminal_escapes() {
    let data = TempDir::new();
    let tasks = data.run_text(&["tasks", "list"]);
    assert!(tasks.status.success());
    let text = String::from_utf8(tasks.stdout).unwrap();
    assert!(text.contains("暂无下载任务"));
    assert!(!text.contains("\"tasks\""));
    assert!(!text.contains('\u{1b}'));
    assert_eq!(fs::read_dir(&data.0).unwrap().count(), 0);

    let auth = data.run_text(&["auth", "status"]);
    assert!(auth.status.success());
    let text = String::from_utf8(auth.stdout).unwrap();
    assert!(text.contains("尚未登录"));
    assert!(text.contains("bili auth login"));
    assert!(!text.contains('\u{1b}'));

    let error = data.run_text(&["tasks", "get", "missing-id"]);
    assert_eq!(error.status.code(), Some(1));
    assert!(error.stdout.is_empty());
    let text = String::from_utf8(error.stderr).unwrap();
    assert!(text.contains("错误 [TASK_NOT_FOUND]"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn missing_task_is_an_error_for_get_and_resume() {
    for args in [
        &["tasks", "get", "missing-id"][..],
        &["resume", "missing-id"][..],
    ] {
        let data = TempDir::new();
        let output = data.run(args);
        assert_eq!(output.status.code(), Some(1));
        let result = json_result(&output);
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "TASK_NOT_FOUND");
    }
}

#[test]
fn invalid_cli_arguments_keep_stdout_json_and_exit_two() {
    let data = TempDir::new();
    let output = data.run(&["download", "BV1xx411c7mD", "--parts", "0"]);
    assert_eq!(output.status.code(), Some(2));
    let result = json_result(&output);
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn malformed_source_is_rejected_before_network_access() {
    let data = TempDir::new();
    let output = data.run(&["info", "not-a-video"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json_result(&output)["error"]["code"], "INVALID_SOURCE");
}

#[test]
fn discovery_rejects_invalid_ids_and_foreign_urls_before_starting_runtime() {
    let cases: &[&[&str]] = &[
        &["search", "uid0"],
        &["search", "fid-1"],
        &["search", "Rust"],
        &["search", "https://example.com/video/BV1xx411c7mD"],
        &["up", "videos", "0"],
        &["up", "videos", "--", "-1"],
        &["up", "videos", "fid123"],
        &["up", "videos", "https://example.com/123"],
        &["fav", "list", "0"],
        &["fav", "list", "--", "-1"],
        &["fav", "list", "https://example.com/123"],
        &["fav", "videos", "0"],
        &["fav", "videos", "--", "-1"],
        &["fav", "videos", "uid123"],
        &["fav", "videos", "https://example.com/123/favlist?fid=456"],
    ];
    for args in cases {
        let data = TempDir::new();
        let output = data.run(args);
        assert_eq!(output.status.code(), Some(1), "args={args:?}");
        assert_eq!(
            json_result(&output)["error"]["code"],
            "INVALID_SOURCE",
            "args={args:?}"
        );
        assert_eq!(fs::read_dir(&data.0).unwrap().count(), 0);
    }
}

#[test]
fn discovery_invalid_pagination_keeps_stdout_json_and_exit_two() {
    for args in [
        &["search", "uid123", "--page", "0"][..],
        &["up", "videos", "123", "--page", "0"][..],
        &["fav", "videos", "456", "--page", "0"][..],
    ] {
        let data = TempDir::new();
        let output = data.run(args);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(json_result(&output)["error"]["code"], "INVALID_ARGUMENT");
        assert_eq!(fs::read_dir(&data.0).unwrap().count(), 0);
    }
}

#[test]
fn unauthenticated_status_does_not_require_network() {
    let data = TempDir::new();
    let output = data.run(&["auth", "status"]);
    assert!(output.status.success());
    let result = json_result(&output);
    assert_eq!(result["ok"], true);
    assert_eq!(result["result"]["authenticated"], false);
}

#[test]
fn saved_task_summary_excludes_signed_urls_and_keeps_partial_progress() {
    let data = TempDir::new();
    let mut progress = bili_core::downloader::download_progress::DownloadProgress {
        task_id: "fixture-id".into(),
        episode_title: "Fixture".into(),
        ..Default::default()
    };
    progress.video_task.selected = true;
    progress.video_task.url = "https://cdn.invalid/?secret=must-not-leak".into();
    progress.video_task.content_length = 20;
    progress.video_task.chunks = vec![
        bili_core::downloader::media_chunk::MediaChunk {
            start: 0,
            end: 9,
            completed: true,
        },
        bili_core::downloader::media_chunk::MediaChunk {
            start: 10,
            end: 19,
            completed: false,
        },
    ];
    fs::create_dir_all(data.0.join(".下载任务")).unwrap();
    fs::write(
        data.0.join(".下载任务/fixture-id.json"),
        serde_json::to_vec(&progress).unwrap(),
    )
    .unwrap();
    let output = data.run(&["tasks", "get", "fixture-id"]);
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("must-not-leak"));
    let result = json_result(&output);
    let task = &result["result"]["task"];
    assert_eq!(task["state"], "Paused");
    assert_eq!(task["video"]["bytes_completed"], 10);
    assert_eq!(task["video"]["bytes_total"], 20);

    let human = data.run_text(&["tasks", "get", "fixture-id"]);
    assert!(human.status.success());
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("已暂停"));
    assert!(text.contains("50.0%"));
    assert!(text.contains("bili resume fixture-id"));
    assert!(!text.contains("bytes_completed"));
    assert!(!text.contains("must-not-leak"));
    assert!(!text.contains('\u{1b}'));
}
