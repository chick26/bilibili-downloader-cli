use std::{path::Path, time::Duration};

use bili_core::{
    Runtime,
    config::{Config, FileExistAction},
    downloader::{download_progress::DownloadProgress, download_task_state::DownloadTaskState},
    service::{read_persisted_tasks, task_snapshot},
    task_options::DownloadOptions,
    types::{
        audio_quality::AudioQuality,
        codec_type::CodecType,
        create_download_task_params::{CreateDownloadTaskParams, CreateNormalDownloadTaskParams},
        normal_info::{NormalInfo, PageInNormal},
        video_quality::VideoQuality,
    },
};

fn disabled_config(data_dir: &Path) -> Config {
    let mut config = Config::defaults(data_dir);
    config.auto_start_download_task = false;
    config.download_video = false;
    config.download_audio = false;
    config.auto_merge = false;
    config.embed_chapter = false;
    config.embed_skip = false;
    config.download_xml_danmaku = false;
    config.download_ass_danmaku = false;
    config.download_json_danmaku = false;
    config.download_subtitle = false;
    config.download_cover = false;
    config.download_nfo = false;
    config.download_json = false;
    config
}

fn normal_info() -> NormalInfo {
    NormalInfo {
        aid: 170001,
        bvid: "BV17x411w7KC".into(),
        cid: 279786,
        title: "Local runtime flow test".into(),
        duration: 10,
        pages: vec![PageInNormal {
            cid: 279786,
            page: 1,
            part: "Part 1".into(),
            duration: 10,
            ..PageInNormal::default()
        }],
        ..NormalInfo::default()
    }
}

#[tokio::test]
async fn resume_before_worker_subscription_completes_and_persists() {
    let data_dir = std::env::temp_dir().join(format!("bili-flow-{}", uuid::Uuid::new_v4()));
    let runtime = Runtime::new(&data_dir, disabled_config(&data_dir)).unwrap();
    let context = runtime.context();
    let manager = context.get_download_manager();
    let info = normal_info();
    let params = CreateDownloadTaskParams::Normal(CreateNormalDownloadTaskParams {
        aid_cid_pairs: vec![(info.aid, None)],
        info,
    });

    // This current-thread test has not yielded, so the spawned worker cannot subscribe yet.
    let ids = manager.create_download_tasks(&params).unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        task_snapshot(&manager, &ids[0]).unwrap().state,
        DownloadTaskState::Paused
    );
    manager.resume_download_tasks(&ids);
    assert_eq!(
        task_snapshot(&manager, &ids[0]).unwrap().state,
        DownloadTaskState::Pending
    );
    let task = manager.download_tasks.read()[&ids[0]].clone();
    let mut state = task.state_sender.subscribe();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if *state.borrow_and_update() == DownloadTaskState::Completed {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .expect("an immediate resume must reach Completed without network requests");

    runtime.shutdown().await.unwrap();
    let persisted = read_persisted_tasks(&data_dir).unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].task_id, ids[0]);
    assert_eq!(persisted[0].state, DownloadTaskState::Completed);
    assert!(persisted[0].progress.is_completed());
    assert!(persisted[0].error.is_none());
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

#[tokio::test]
async fn saved_task_options_survive_restore_and_another_runtime() {
    let data_dir = std::env::temp_dir().join(format!("bili-flow-{}", uuid::Uuid::new_v4()));
    let mut global_config = disabled_config(&data_dir);
    global_config.download_dir = data_dir.join("current-default-output");
    global_config.video_quality_priority = vec![VideoQuality::Video1080P];
    global_config.codec_type_priority = vec![CodecType::AVC];
    global_config.sessdata = "current-global-session".into();
    global_config.save_to(&data_dir).unwrap();
    let global_options = DownloadOptions::from(&global_config);

    let mut original_config = disabled_config(&data_dir);
    original_config.download_dir = data_dir.join("task-original-output");
    original_config.dir_fmt = "{episode_title}".into();
    original_config.dir_fmt_for_part = "P{part_order}/{part_title}".into();
    original_config.time_fmt = "%Y%m%d".into();
    original_config.video_quality_priority = vec![VideoQuality::Video720P];
    original_config.codec_type_priority = vec![CodecType::HEVC, CodecType::AV1];
    original_config.audio_quality_priority = vec![AudioQuality::Audio64K];
    original_config.file_exist_action = FileExistAction::Skip;
    original_config.sessdata = "original-task-session-must-not-be-saved".into();
    let original_options = DownloadOptions::from(&original_config);

    let runtime = Runtime::new(&data_dir, global_config).unwrap();
    let context = runtime.context();
    let info = normal_info();
    let mut progress = DownloadProgress::from_normal(&context, &info, info.aid, None)
        .unwrap()
        .pop()
        .unwrap();
    progress.video_task.selected = true;
    progress.options = Some(original_options.clone());
    assert!(!progress.is_completed());
    let task_id = progress.task_id.clone();
    progress.save(&context, true).unwrap();

    let manager = context.get_download_manager();
    manager.restore_download_tasks().unwrap();
    let task = manager.download_tasks.read()[&task_id].clone();
    assert_eq!(*task.state_sender.borrow(), DownloadTaskState::Paused);
    assert_eq!(
        DownloadOptions::from(&*task.app.get_config().read()),
        original_options
    );
    assert_eq!(
        DownloadOptions::from(&*context.get_config().read()),
        global_options
    );
    assert_eq!(
        task.app.get_config().read().sessdata,
        "current-global-session"
    );
    runtime.shutdown().await.unwrap();
    drop(task);
    drop(manager);
    drop(context);
    drop(runtime);

    let saved = std::fs::read_to_string(data_dir.join(".下载任务").join(format!("{task_id}.json")))
        .unwrap();
    assert!(!saved.contains("sessdata"));
    assert!(!saved.contains("current-global-session"));
    assert!(!saved.contains("original-task-session-must-not-be-saved"));

    let reopened = Runtime::load(&data_dir).unwrap();
    let reopened_context = reopened.context();
    let reopened_manager = reopened_context.get_download_manager();
    reopened_manager.restore_download_tasks().unwrap();
    let reopened_task = reopened_manager.download_tasks.read()[&task_id].clone();
    assert_eq!(
        *reopened_task.state_sender.borrow(),
        DownloadTaskState::Paused
    );
    assert_eq!(
        DownloadOptions::from(&*reopened_task.app.get_config().read()),
        original_options
    );
    assert_eq!(
        DownloadOptions::from(&*reopened_context.get_config().read()),
        global_options
    );
    assert_eq!(
        reopened_task.progress.read().options.as_ref(),
        Some(&original_options)
    );
    reopened.shutdown().await.unwrap();
    drop(reopened);
    std::fs::remove_dir_all(data_dir).unwrap();
}
