mod args;
mod auth;
mod discovery;
mod format;
mod output;
mod presentation;
mod progress;

use std::{path::PathBuf, process::ExitCode, time::Duration};

use bili_core::{
    Runtime,
    config::Config,
    downloader::download_task_state::DownloadTaskState,
    service::{
        MediaTarget, Selection, TaskSnapshot, lookup, read_persisted_tasks, task_snapshot,
        validate_resume,
    },
    types::{codec_type::CodecType, video_quality::VideoQuality},
};
use clap::Parser;
use serde_json::json;

use args::{AuthCommand, Cli, Command, DownloadArgs, TaskCommand};
use output::{CliError, Outcome, task_summary};

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<_> = std::env::args_os().collect();
    let json_requested = argv.iter().any(|arg| arg == "--json");
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(err) => {
            if json_requested && err.use_stderr() {
                output::emit_error(true, &CliError::new("INVALID_ARGUMENT", err.to_string()));
            } else {
                let _ = err.print();
            }
            return ExitCode::from(u8::try_from(err.exit_code()).unwrap_or(2));
        }
    };
    let json_output = cli.json;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();

    match run(cli).await {
        Ok(outcome) => {
            output::emit_result(json_output, &outcome);
            ExitCode::from(outcome.exit_code)
        }
        Err(err) => {
            output::emit_error(json_output, &err);
            ExitCode::from(err.exit_code)
        }
    }
}

async fn run(cli: Cli) -> Result<Outcome, CliError> {
    let discovery_request = discovery::Request::from_command(&cli.command)?;
    let data_dir = absolute_path(&cli.data_dir.unwrap_or_else(default_data_dir))?;

    // Reading task records does not start workers or contend with the active downloader lock.
    if let Command::Tasks { command } = &cli.command {
        let tasks = read_persisted_tasks(&data_dir)
            .map_err(|err| CliError::report("TASK_READ_FAILED", err))?;
        let result = match command {
            TaskCommand::List => {
                json!({"tasks": tasks.iter().map(task_summary).collect::<Vec<_>>(), "live": false })
            }
            TaskCommand::Get { id } => {
                let task = tasks
                    .iter()
                    .find(|task| task.task_id == *id)
                    .ok_or_else(|| {
                        CliError::new("TASK_NOT_FOUND", format!("No task with ID {id}"))
                    })?;
                json!({"task": task_summary(task), "live": false})
            }
        };
        return Ok(Outcome::success(result));
    }

    let mut config =
        Config::load(&data_dir).map_err(|err| CliError::report("CONFIG_READ_FAILED", err))?;
    // Restoring progress must never resume unrelated downloads.
    config.auto_start_download_task = false;
    if let Some(ffmpeg) = cli.ffmpeg {
        config.ffmpeg_path = Some(ffmpeg);
    }
    if let Command::Download(args) = &cli.command {
        apply_download_options(&mut config, args)?;
    }
    let runtime = Runtime::new(data_dir.clone(), config)
        .map_err(|err| CliError::report("RUNTIME_START_FAILED", err))?;
    let context = runtime.context();

    let result = async {
        if let Some(request) = discovery_request {
            return request.execute(&context.get_bili_client()).await;
        }
        match cli.command {
            Command::Search(_) | Command::Up { .. } | Command::Fav { .. } => {
                unreachable!("handled by validated discovery request")
            }
            Command::Info { source } => {
                let target = MediaTarget::parse(&source)
                    .map_err(|err| CliError::report("INVALID_SOURCE", err))?;
                let info = lookup(&context.get_bili_client(), &target)
                    .await
                    .map_err(|err| CliError::report("LOOKUP_FAILED", err))?;
                Ok(Outcome::success(json!({"media": info})))
            }
            Command::Download(args) => {
                let target = MediaTarget::parse(&args.source)
                    .map_err(|err| CliError::report("INVALID_SOURCE", err))?;
                let info = lookup(&context.get_bili_client(), &target)
                    .await
                    .map_err(|err| CliError::report("LOOKUP_FAILED", err))?;
                let params = info
                    .create_params(&Selection {
                        all: args.all,
                        pages: args.parts,
                        episode_ids: Vec::new(),
                    })
                    .map_err(|err| CliError::report("INVALID_SELECTION", err))?;
                let manager = context.get_download_manager();
                manager
                    .restore_download_tasks()
                    .map_err(|err| CliError::report("TASK_READ_FAILED", err))?;
                let task_ids = manager
                    .create_download_tasks(&params)
                    .map_err(|err| CliError::report("TASK_CREATE_FAILED", err))?;
                if task_ids.is_empty() {
                    return Err(CliError::new(
                        "EMPTY_SELECTION",
                        "No downloadable tasks were created",
                    ));
                }
                validate_resume(&manager, &task_ids)
                    .map_err(|err| CliError::report("TASK_RESUME_FAILED", err))?;
                manager.resume_download_tasks(&task_ids);
                eprintln!("开始下载 {} 个任务 · Ctrl-C 保存进度", task_ids.len());
                wait_for_downloads(&runtime, &task_ids, cli.json).await
            }
            Command::Resume { id } => {
                let manager = context.get_download_manager();
                manager
                    .restore_download_tasks()
                    .map_err(|err| CliError::report("TASK_READ_FAILED", err))?;
                let ids = vec![id];
                task_snapshot(&manager, &ids[0])
                    .map_err(|err| CliError::report("TASK_NOT_FOUND", err))?;
                validate_resume(&manager, &ids)
                    .map_err(|err| CliError::report("TASK_RESUME_FAILED", err))?;
                manager.resume_download_tasks(&ids);
                wait_for_downloads(&runtime, &ids, cli.json).await
            }
            Command::Auth { command } => match command {
                AuthCommand::Status => auth::status(&context).await,
                AuthCommand::SetCookie => auth::set_cookie(&context, &data_dir).await,
                AuthCommand::Login { timeout } => auth::login(&context, &data_dir, timeout).await,
            },
            Command::Tasks { .. } => unreachable!("handled before runtime construction"),
        }
    }
    .await;
    let shutdown = runtime.shutdown().await;
    match result {
        Err(error) => Err(error),
        Ok(outcome) => {
            shutdown.map_err(|err| CliError::report("TASK_SAVE_FAILED", err))?;
            Ok(outcome)
        }
    }
}

fn apply_download_options(config: &mut Config, args: &DownloadArgs) -> Result<(), CliError> {
    if let Some(output) = &args.output {
        config.download_dir = absolute_path(output)?;
    }
    config.download_video = !args.audio_only;
    config.download_audio = !args.video_only;
    config.auto_merge = !args.no_merge && !args.video_only && !args.audio_only;
    if args.audio_only {
        config.embed_chapter = false;
        config.embed_skip = false;
    }
    if args.no_extras {
        config.embed_chapter = false;
        config.embed_skip = false;
        config.download_xml_danmaku = false;
        config.download_ass_danmaku = false;
        config.download_json_danmaku = false;
        config.download_subtitle = false;
        config.download_cover = false;
        config.download_nfo = false;
        config.download_json = false;
    }
    if let Some(quality) = args.quality {
        let quality = VideoQuality::from(quality);
        if quality == VideoQuality::Unknown {
            return Err(CliError::new(
                "INVALID_QUALITY",
                "Unsupported video quality ID",
            ));
        }
        config.video_quality_priority = vec![quality];
    }
    if let Some(codec) = args.codec {
        config.codec_type_priority = vec![match codec {
            args::Codec::Avc => CodecType::AVC,
            args::Codec::Hevc => CodecType::HEVC,
            args::Codec::Av1 => CodecType::AV1,
        }];
    }
    Ok(())
}

async fn wait_for_downloads(
    runtime: &Runtime,
    ids: &[String],
    json_output: bool,
) -> Result<Outcome, CliError> {
    let context = runtime.context();
    let manager = context.get_download_manager();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut display = progress::DownloadProgressDisplay::new(json_output);
    loop {
        tokio::select! {
            result = &mut interrupt => {
                result.map_err(|err| CliError::new("SIGNAL_FAILED", err.to_string()))?;
                runtime.shutdown().await.map_err(|err| CliError::report("TASK_SAVE_FAILED", err))?;
                let tasks = selected_tasks(&manager, ids)?;
                display.finish();
                return Ok(Outcome {
                    result: json!({"interrupted": true, "tasks": tasks.iter().map(task_summary).collect::<Vec<_>>() }),
                    exit_code: 130,
                });
            }
            _ = tick.tick() => {
                let tasks = selected_tasks(&manager, ids)?;
                display.update(&tasks);
                let finished = tasks.iter().all(|task| matches!(task.state, DownloadTaskState::Completed | DownloadTaskState::Failed | DownloadTaskState::Paused));
                if finished {
                    runtime.flush_progress().map_err(|err| CliError::report("TASK_SAVE_FAILED", err))?;
                    display.finish();
                    let failed = tasks.iter().any(|task| task.state != DownloadTaskState::Completed);
                    return Ok(Outcome {
                        result: json!({"completed": !failed, "tasks": tasks.iter().map(task_summary).collect::<Vec<_>>() }),
                        exit_code: u8::from(failed),
                    });
                }
            }
        }
    }
}

fn selected_tasks(
    manager: &bili_core::downloader::download_manager::DownloadManager,
    ids: &[String],
) -> Result<Vec<TaskSnapshot>, CliError> {
    ids.iter()
        .map(|id| task_snapshot(manager, id).map_err(|err| CliError::report("TASK_NOT_FOUND", err)))
        .collect()
}

fn absolute_path(path: &std::path::Path) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|err| CliError::new("INVALID_PATH", err.to_string()))
    }
}

fn default_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("BILI_DATA_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("bili");
    }
    if let Some(path) = std::env::var_os("HOME") {
        #[cfg(target_os = "macos")]
        return PathBuf::from(path).join("Library/Application Support/bili");
        #[cfg(not(target_os = "macos"))]
        return PathBuf::from(path).join(".local/share/bili");
    }
    PathBuf::from(".bili")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn options_reject_conflicting_media_selection() {
        assert!(
            Cli::try_parse_from([
                "bili",
                "download",
                "BV1xx411c7mD",
                "--audio-only",
                "--video-only"
            ])
            .is_err()
        );
    }

    #[test]
    fn json_global_argument_and_parts_work_after_subcommand() {
        let cli = Cli::try_parse_from([
            "bili",
            "download",
            "BV1xx411c7mD",
            "--parts",
            "1,3",
            "--json",
        ])
        .unwrap();
        assert!(cli.json);
        let Command::Download(args) = cli.command else {
            panic!("wrong command")
        };
        assert_eq!(args.parts, vec![1, 3]);
    }

    #[test]
    fn zero_part_is_invalid() {
        assert!(Cli::try_parse_from(["bili", "download", "BV1xx411c7mD", "--parts", "0"]).is_err());
    }

    #[test]
    fn discovery_commands_dispatch_and_accept_global_json() {
        let cli =
            Cli::try_parse_from(["bili", "search", "uid123", "--page", "2", "--json"]).unwrap();
        assert!(cli.json);
        let Command::Search(args) = cli.command else {
            panic!("wrong command")
        };
        assert_eq!(args.source, "uid123");
        assert_eq!(args.page, 2);

        let cli = Cli::try_parse_from(["bili", "up", "videos", "123", "--page", "3"]).unwrap();
        let Command::Up {
            command: args::UpCommand::Videos(args),
        } = cli.command
        else {
            panic!("wrong command")
        };
        assert_eq!(args.source, "123");
        assert_eq!(args.page, 3);

        let cli = Cli::try_parse_from(["bili", "fav", "list", "123"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Fav {
                command: args::FavCommand::List { .. }
            }
        ));
        let cli = Cli::try_parse_from(["bili", "fav", "videos", "456"]).unwrap();
        let Command::Fav {
            command: args::FavCommand::Videos(args),
        } = cli.command
        else {
            panic!("wrong command")
        };
        assert_eq!(args.source, "456");
        assert_eq!(args.page, 1);
    }

    #[test]
    fn discovery_pages_must_be_positive() {
        for command in [
            vec!["bili", "search", "uid123"],
            vec!["bili", "up", "videos", "123"],
            vec!["bili", "fav", "videos", "456"],
        ] {
            for page in ["0", "-1", "4294967296", "invalid"] {
                let mut argv = command.clone();
                argv.extend(["--page", page]);
                assert!(Cli::try_parse_from(argv).is_err());
            }
        }
    }

    #[test]
    fn media_options_do_not_require_extras_or_merging() {
        let cli = Cli::try_parse_from([
            "bili",
            "download",
            "BV1xx411c7mD",
            "--audio-only",
            "--no-extras",
        ])
        .unwrap();
        let Command::Download(args) = cli.command else {
            panic!("wrong command")
        };
        let mut config = Config::defaults(std::path::Path::new("/tmp/bili-test"));
        apply_download_options(&mut config, &args).unwrap();
        assert!(!config.download_video);
        assert!(config.download_audio);
        assert!(!config.auto_merge);
        assert!(!config.download_subtitle);
        assert!(!config.embed_chapter);
    }
}
