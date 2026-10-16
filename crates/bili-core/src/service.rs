//! Typed, process-local operations shared by the command-line entry points.

use std::{collections::HashSet, path::Path, str::FromStr};

use eyre::{WrapErr, bail, eyre};
use serde::{Deserialize, Serialize};

use crate::{
    bili_client::BiliClient,
    downloader::{
        download_manager::DownloadManager, download_progress::DownloadProgress,
        download_task_state::DownloadTaskState,
    },
    types::{
        bangumi_info::BangumiInfo,
        cheese_info::CheeseInfo,
        create_download_task_params::{
            CreateBangumiDownloadTaskParams, CreateCheeseDownloadTaskParams,
            CreateDownloadTaskParams, CreateNormalDownloadTaskParams,
        },
        get_bangumi_info_params::GetBangumiInfoParams,
        get_cheese_info_params::GetCheeseInfoParams,
        get_normal_info_params::GetNormalInfoParams,
        normal_info::NormalInfo,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum NormalId {
    Bvid(String),
    Aid(i64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaTarget {
    Normal { id: NormalId, page: Option<u32> },
    BangumiEpisode { id: i64 },
    BangumiSeason { id: i64 },
    CheeseEpisode { id: i64 },
    CheeseSeason { id: i64 },
}

impl MediaTarget {
    /// Accept a Bilibili URL or a BV/av/ep/ss identifier. Bare ep/ss identifiers
    /// refer to Bangumi; Cheese requires an explicit `/cheese/play/` URL.
    pub fn parse(input: &str) -> eyre::Result<Self> {
        let input = input.trim();
        if input.is_empty() {
            bail!("请提供 B 站视频 URL 或 BV/av/ep/ss ID");
        }
        if !input.contains("://") {
            return parse_identifier(input, false, None);
        }

        let url = reqwest::Url::parse(input).wrap_err("无法解析视频 URL")?;
        if !matches!(url.scheme(), "https" | "http") {
            bail!("仅支持 http/https 视频 URL");
        }
        let host = url.host_str().unwrap_or_default();
        if host != "bilibili.com" && !host.ends_with(".bilibili.com") {
            bail!("仅支持 bilibili.com 视频 URL；短链接请先展开为完整视频 URL");
        }
        let segments: Vec<_> = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|segment| !segment.is_empty())
            .collect();
        let page_values: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key == "p")
            .map(|(_, value)| value.into_owned())
            .collect();
        if page_values.len() > 1 {
            bail!("URL 中不能重复指定分 P 参数 p");
        }
        let page = page_values
            .first()
            .map(|value| {
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|page| *page > 0)
                    .ok_or_else(|| eyre!("分 P 参数 p 必须是正整数"))
            })
            .transpose()?;

        match segments.as_slice() {
            ["video", id]
                if starts_with_ignore_ascii_case(id, "BV")
                    || starts_with_ignore_ascii_case(id, "av") =>
            {
                parse_identifier(id, false, page)
            }
            ["bangumi", "play", id] if page.is_none() => {
                let target = parse_identifier(id, false, None)?;
                if !matches!(
                    target,
                    Self::BangumiEpisode { .. } | Self::BangumiSeason { .. }
                ) {
                    bail!("番剧 URL 必须包含 ep 或 ss ID");
                }
                Ok(target)
            }
            ["cheese", "play", id] if page.is_none() => parse_identifier(id, true, None),
            _ => bail!("不支持此 URL；请使用 /video/BV…、/bangumi/play/ep… 或 /cheese/play/ep…"),
        }
    }
}

impl FromStr for MediaTarget {
    type Err = eyre::Report;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

fn parse_identifier(input: &str, cheese: bool, page: Option<u32>) -> eyre::Result<MediaTarget> {
    if !cheese && starts_with_ignore_ascii_case(input, "BV") {
        if input.len() != 12 || !input.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            bail!("BV ID 必须是 BV 开头的 12 位字母数字字符串");
        }
        return Ok(MediaTarget::Normal {
            id: NormalId::Bvid(format!("BV{}", &input[2..])),
            page,
        });
    }

    let prefix = input.get(..2).unwrap_or_default().to_ascii_lowercase();
    let id = input
        .get(2..)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or_else(|| eyre!("无法识别视频 ID；请使用 BV/av/ep/ss ID 或完整视频 URL"))?;
    match (prefix.as_str(), cheese) {
        ("av", false) => Ok(MediaTarget::Normal {
            id: NormalId::Aid(id),
            page,
        }),
        ("ep", false) if page.is_none() => Ok(MediaTarget::BangumiEpisode { id }),
        ("ss", false) if page.is_none() => Ok(MediaTarget::BangumiSeason { id }),
        ("ep", true) if page.is_none() => Ok(MediaTarget::CheeseEpisode { id }),
        ("ss", true) if page.is_none() => Ok(MediaTarget::CheeseSeason { id }),
        _ => bail!("无法识别视频 ID 类型或分 P 参数"),
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "info", rename_all = "snake_case")]
pub enum MediaMetadata {
    Normal(NormalInfo),
    Bangumi(BangumiInfo),
    Cheese(CheeseInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub target: MediaTarget,
    pub info: MediaMetadata,
}

/// Explicit selection overrides a URL's selected part/episode. `all` expands a
/// collection/season, while the default only expands the current video's parts
/// or the main episodes of a season URL.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub all: bool,
    pub pages: Vec<u32>,
    pub episode_ids: Vec<i64>,
}

pub async fn lookup(client: &BiliClient, target: &MediaTarget) -> eyre::Result<MediaInfo> {
    let info = match target {
        MediaTarget::Normal { id, .. } => {
            let params = match id {
                NormalId::Bvid(id) => GetNormalInfoParams::Bvid(id.clone()),
                NormalId::Aid(id) => GetNormalInfoParams::Aid(*id),
            };
            MediaMetadata::Normal(client.get_normal_info(params).await?)
        }
        MediaTarget::BangumiEpisode { id } => MediaMetadata::Bangumi(
            client
                .get_bangumi_info(GetBangumiInfoParams::EpId(*id))
                .await?,
        ),
        MediaTarget::BangumiSeason { id } => MediaMetadata::Bangumi(
            client
                .get_bangumi_info(GetBangumiInfoParams::SeasonId(*id))
                .await?,
        ),
        MediaTarget::CheeseEpisode { id } => MediaMetadata::Cheese(
            client
                .get_cheese_info(GetCheeseInfoParams::EpId(*id))
                .await?,
        ),
        MediaTarget::CheeseSeason { id } => MediaMetadata::Cheese(
            client
                .get_cheese_info(GetCheeseInfoParams::SeasonId(*id))
                .await?,
        ),
    };
    Ok(MediaInfo {
        target: target.clone(),
        info,
    })
}

impl MediaInfo {
    pub fn create_params(&self, selection: &Selection) -> eyre::Result<CreateDownloadTaskParams> {
        if !selection.pages.is_empty() && !selection.episode_ids.is_empty() {
            bail!("不能同时指定序号和 ep ID");
        }
        if selection.all && (!selection.pages.is_empty() || !selection.episode_ids.is_empty()) {
            bail!("--all 不能与指定分 P 或指定集数一起使用");
        }
        match (&self.info, &self.target) {
            (MediaMetadata::Normal(info), MediaTarget::Normal { page, .. }) => {
                if !selection.episode_ids.is_empty() {
                    bail!("普通视频请指定分 P，不能指定 ep ID");
                }
                let pages = if selection.pages.is_empty() && !selection.all {
                    page.iter().copied().collect()
                } else {
                    selection.pages.clone()
                };
                let aid_cid_pairs = if !pages.is_empty() {
                    let mut pairs = Vec::new();
                    for page in unique(pages) {
                        let matching_page = info
                            .pages
                            .iter()
                            .find(|candidate| u32::try_from(candidate.page) == Ok(page))
                            .ok_or_else(|| eyre!("视频不存在第 {page} 个分 P"))?;
                        pairs.push((info.aid, Some(matching_page.cid)));
                    }
                    pairs
                } else if selection.all {
                    if let Some(season) = &info.ugc_season {
                        unique(
                            season
                                .sections
                                .iter()
                                .flat_map(|section| section.episodes.iter())
                                .map(|episode| (episode.aid, None)),
                        )
                    } else {
                        vec![(info.aid, None)]
                    }
                } else {
                    vec![(info.aid, None)]
                };
                if aid_cid_pairs.is_empty() {
                    bail!("视频没有可下载的分 P");
                }
                Ok(CreateDownloadTaskParams::Normal(
                    CreateNormalDownloadTaskParams {
                        info: info.clone(),
                        aid_cid_pairs,
                    },
                ))
            }
            (MediaMetadata::Bangumi(info), target) => {
                let all_ids = || {
                    info.episodes.iter().map(|episode| episode.id).chain(
                        info.section
                            .iter()
                            .flatten()
                            .flat_map(|section| section.episodes.iter())
                            .map(|episode| episode.id),
                    )
                };
                let ep_ids = if !selection.pages.is_empty() {
                    select_episode_positions(&selection.pages, all_ids())?
                } else if !selection.episode_ids.is_empty() {
                    unique(selection.episode_ids.iter().copied())
                } else if selection.all {
                    unique(all_ids())
                } else if let MediaTarget::BangumiEpisode { id } = target {
                    vec![*id]
                } else if matches!(target, MediaTarget::BangumiSeason { .. }) {
                    unique(info.episodes.iter().map(|episode| episode.id))
                } else {
                    bail!("视频类型与查询结果不一致");
                };
                validate_episode_selection(&ep_ids, all_ids())?;
                for ep_id in &ep_ids {
                    let (episode, _) = info.get_episode_with_order(*ep_id)?;
                    if episode.duration.is_none() {
                        bail!("ep{ep_id} 尚无视频时长，无法创建下载任务");
                    }
                }
                Ok(CreateDownloadTaskParams::Bangumi(
                    CreateBangumiDownloadTaskParams {
                        ep_ids,
                        info: info.clone(),
                    },
                ))
            }
            (MediaMetadata::Cheese(info), target) => {
                let ep_ids = if !selection.pages.is_empty() {
                    select_episode_positions(
                        &selection.pages,
                        info.episodes.iter().map(|episode| episode.id),
                    )?
                } else if !selection.episode_ids.is_empty() {
                    unique(selection.episode_ids.iter().copied())
                } else if !selection.all
                    && let MediaTarget::CheeseEpisode { id } = target
                {
                    vec![*id]
                } else if matches!(
                    target,
                    MediaTarget::CheeseEpisode { .. } | MediaTarget::CheeseSeason { .. }
                ) {
                    unique(info.episodes.iter().map(|episode| episode.id))
                } else {
                    bail!("视频类型与查询结果不一致");
                };
                validate_episode_selection(
                    &ep_ids,
                    info.episodes.iter().map(|episode| episode.id),
                )?;
                Ok(CreateDownloadTaskParams::Cheese(
                    CreateCheeseDownloadTaskParams {
                        ep_ids,
                        info: info.clone(),
                    },
                ))
            }
            _ => bail!("视频类型与查询结果不一致"),
        }
    }
}

fn unique<T: Copy + Eq + std::hash::Hash>(values: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(*value))
        .collect()
}

fn select_episode_positions(
    positions: &[u32],
    ids: impl IntoIterator<Item = i64>,
) -> eyre::Result<Vec<i64>> {
    let ids: Vec<_> = ids.into_iter().collect();
    let selected = positions
        .iter()
        .map(|position| {
            position
                .checked_sub(1)
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| ids.get(index).copied())
                .ok_or_else(|| eyre!("查询结果中不存在第 {position} 集"))
        })
        .collect::<eyre::Result<Vec<_>>>()?;
    Ok(unique(selected))
}

fn validate_episode_selection(
    selected: &[i64],
    available: impl IntoIterator<Item = i64>,
) -> eyre::Result<()> {
    if selected.is_empty() {
        bail!("没有可下载的集数");
    }
    let available: HashSet<_> = available.into_iter().collect();
    for id in selected {
        if *id <= 0 || !available.contains(id) {
            bail!("查询结果中不存在 ep{id}");
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub state: DownloadTaskState,
    pub progress: DownloadProgress,
    pub error: Option<String>,
}

pub fn task_snapshots(manager: &DownloadManager) -> Vec<TaskSnapshot> {
    let tasks = manager.download_tasks.read();
    let mut snapshots: Vec<_> = tasks
        .values()
        .map(|task| TaskSnapshot {
            task_id: task.task_id.clone(),
            state: *task.state_sender.borrow(),
            progress: task.progress.read().clone(),
            error: task.last_error.read().clone(),
        })
        .collect();
    sort_snapshots(&mut snapshots);
    snapshots
}

pub fn task_snapshot(manager: &DownloadManager, task_id: &str) -> eyre::Result<TaskSnapshot> {
    let tasks = manager.download_tasks.read();
    let task = tasks
        .get(task_id)
        .ok_or_else(|| eyre!("未找到下载任务 {task_id}"))?;
    Ok(TaskSnapshot {
        task_id: task.task_id.clone(),
        state: *task.state_sender.borrow(),
        progress: task.progress.read().clone(),
        error: task.last_error.read().clone(),
    })
}

/// Validate every ID before changing any state so a misspelled ID cannot cause
/// a partially resumed batch.
pub fn validate_resume(manager: &DownloadManager, task_ids: &[String]) -> eyre::Result<()> {
    if task_ids.is_empty() {
        bail!("请至少指定一个下载任务 ID");
    }
    for task_id in task_ids {
        let task = task_snapshot(manager, task_id)?;
        if !matches!(
            task.state,
            DownloadTaskState::Paused | DownloadTaskState::Failed
        ) {
            bail!(
                "任务 {task_id} 状态为 {:?}，仅暂停或失败的任务可以恢复",
                task.state
            );
        }
    }
    Ok(())
}

/// Inspect saved progress without starting task workers or changing files.
/// Writers atomically replace records, allowing concurrent read-only inspection.
pub fn read_persisted_tasks(data_dir: &Path) -> eyre::Result<Vec<TaskSnapshot>> {
    let task_dir = data_dir.join(".下载任务");
    let entries = match std::fs::read_dir(&task_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).wrap_err_with(|| format!("读取任务目录 {} 失败", task_dir.display()));
        }
    };
    let mut snapshots = Vec::new();
    for entry in entries {
        let entry = entry.wrap_err("读取任务文件目录项失败")?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let json = std::fs::read_to_string(&path)
            .wrap_err_with(|| format!("读取任务进度 {} 失败", path.display()))?;
        let progress: DownloadProgress = serde_json::from_str(&json)
            .wrap_err_with(|| format!("解析任务进度 {} 失败；原文件已保留", path.display()))?;
        if path.file_stem().and_then(|value| value.to_str()) != Some(progress.task_id.as_str()) {
            bail!("任务进度文件 {} 的 ID 与文件名不一致", path.display());
        }
        snapshots.push(TaskSnapshot {
            task_id: progress.task_id.clone(),
            state: if progress.last_error.is_some() {
                DownloadTaskState::Failed
            } else if progress.is_completed() {
                DownloadTaskState::Completed
            } else {
                DownloadTaskState::Paused
            },
            error: progress.last_error.clone(),
            progress,
        });
    }
    sort_snapshots(&mut snapshots);
    Ok(snapshots)
}

fn sort_snapshots(snapshots: &mut [TaskSnapshot]) {
    snapshots.sort_by(|left, right| {
        left.progress
            .create_ts
            .cmp(&right.progress.create_ts)
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        bangumi_info::{EpInBangumi, SectionInBangumi},
        cheese_info::EpInCheese,
        normal_info::PageInNormal,
    };

    #[test]
    fn parses_ids_urls_and_cheese_without_episode_ambiguity() {
        assert_eq!(
            MediaTarget::parse("av123").unwrap(),
            MediaTarget::Normal {
                id: NormalId::Aid(123),
                page: None,
            }
        );
        assert_eq!(
            MediaTarget::parse(
                "https://www.bilibili.com/video/BV1xx411c7mD/?p=2&share_source=copy_link"
            )
            .unwrap(),
            MediaTarget::Normal {
                id: NormalId::Bvid("BV1xx411c7mD".to_owned()),
                page: Some(2),
            }
        );
        assert_eq!(
            MediaTarget::parse("ep42").unwrap(),
            MediaTarget::BangumiEpisode { id: 42 }
        );
        assert_eq!(
            MediaTarget::parse("https://www.bilibili.com/bangumi/play/ss42").unwrap(),
            MediaTarget::BangumiSeason { id: 42 }
        );
        assert_eq!(
            MediaTarget::parse("https://www.bilibili.com/cheese/play/ep42").unwrap(),
            MediaTarget::CheeseEpisode { id: 42 }
        );
        assert_eq!(
            MediaTarget::parse("https://www.bilibili.com/cheese/play/ss42").unwrap(),
            MediaTarget::CheeseSeason { id: 42 }
        );
    }

    #[test]
    fn rejects_invalid_ids_hosts_and_part_queries() {
        for input in [
            "",
            "关键词",
            "av0",
            "av-1",
            "ep9223372036854775808",
            "BVshort",
            "https://evilbilibili.com/video/av123",
            "https://b23.tv/abcd",
            "https://www.bilibili.com/video/av123?p=0",
            "https://www.bilibili.com/video/av123?p=1&p=2",
            "https://www.bilibili.com/cheese/play/av123",
            "https://www.bilibili.com/bangumi/play/ep42?p=2",
        ] {
            assert!(
                MediaTarget::parse(input).is_err(),
                "unexpectedly accepted {input}"
            );
        }
    }

    fn normal_info(page: Option<u32>) -> MediaInfo {
        MediaInfo {
            target: MediaTarget::Normal {
                id: NormalId::Aid(1),
                page,
            },
            info: MediaMetadata::Normal(NormalInfo {
                aid: 1,
                pages: vec![
                    PageInNormal {
                        page: 1,
                        cid: 11,
                        ..Default::default()
                    },
                    PageInNormal {
                        page: 2,
                        cid: 12,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
        }
    }

    #[test]
    fn normal_selection_defaults_to_current_video_and_respects_url_part() {
        let CreateDownloadTaskParams::Normal(default) = normal_info(None)
            .create_params(&Selection::default())
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(default.aid_cid_pairs, vec![(1, None)]);
        let CreateDownloadTaskParams::Normal(part) = normal_info(Some(2))
            .create_params(&Selection::default())
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(part.aid_cid_pairs, vec![(1, Some(12))]);
        let CreateDownloadTaskParams::Normal(override_part) = normal_info(Some(2))
            .create_params(&Selection {
                pages: vec![1, 1],
                ..Default::default()
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(override_part.aid_cid_pairs, vec![(1, Some(11))]);
        assert!(
            normal_info(None)
                .create_params(&Selection {
                    pages: vec![3],
                    ..Default::default()
                })
                .is_err()
        );
        assert!(
            normal_info(None)
                .create_params(&Selection {
                    all: true,
                    pages: vec![1],
                    ..Default::default()
                })
                .is_err()
        );
    }

    #[test]
    fn parts_map_bangumi_main_and_section_episodes_in_display_order() {
        let info = bangumi_info(MediaTarget::BangumiSeason { id: 42 });
        let CreateDownloadTaskParams::Bangumi(selected) = info
            .create_params(&Selection {
                pages: vec![1, 3, 1],
                ..Default::default()
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(selected.ep_ids, vec![1, 3]);
        for pages in [vec![0], vec![4]] {
            assert!(
                info.create_params(&Selection {
                    pages,
                    ..Default::default()
                })
                .is_err()
            );
        }
    }

    fn bangumi_info(target: MediaTarget) -> MediaInfo {
        let episode = |id| EpInBangumi {
            id,
            duration: Some(1_000),
            ..Default::default()
        };
        MediaInfo {
            target,
            info: MediaMetadata::Bangumi(BangumiInfo {
                episodes: vec![episode(1), episode(2)],
                section: Some(vec![SectionInBangumi {
                    episodes: vec![episode(3)],
                    ..Default::default()
                }]),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn bangumi_episode_season_and_all_have_distinct_selection() {
        let ep = bangumi_info(MediaTarget::BangumiEpisode { id: 2 });
        let CreateDownloadTaskParams::Bangumi(single) =
            ep.create_params(&Selection::default()).unwrap()
        else {
            panic!()
        };
        assert_eq!(single.ep_ids, vec![2]);
        let CreateDownloadTaskParams::Bangumi(all) = ep
            .create_params(&Selection {
                all: true,
                ..Default::default()
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(all.ep_ids, vec![1, 2, 3]);
        let season = bangumi_info(MediaTarget::BangumiSeason { id: 42 });
        let CreateDownloadTaskParams::Bangumi(main) =
            season.create_params(&Selection::default()).unwrap()
        else {
            panic!()
        };
        assert_eq!(main.ep_ids, vec![1, 2]);
        assert!(
            season
                .create_params(&Selection {
                    episode_ids: vec![4],
                    ..Default::default()
                })
                .is_err()
        );
        assert!(
            season
                .create_params(&Selection {
                    pages: vec![4],
                    ..Default::default()
                })
                .is_err()
        );
    }

    #[test]
    fn cheese_only_selects_requested_episode_unless_expanded() {
        let info = MediaInfo {
            target: MediaTarget::CheeseEpisode { id: 2 },
            info: MediaMetadata::Cheese(CheeseInfo {
                episodes: vec![
                    EpInCheese {
                        id: 1,
                        ..Default::default()
                    },
                    EpInCheese {
                        id: 2,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
        };
        let CreateDownloadTaskParams::Cheese(single) =
            info.create_params(&Selection::default()).unwrap()
        else {
            panic!()
        };
        assert_eq!(single.ep_ids, vec![2]);
        let CreateDownloadTaskParams::Cheese(all) = info
            .create_params(&Selection {
                all: true,
                ..Default::default()
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(all.ep_ids, vec![1, 2]);
        let CreateDownloadTaskParams::Cheese(selected) = info
            .create_params(&Selection {
                pages: vec![2, 1, 2],
                ..Default::default()
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(selected.ep_ids, vec![2, 1]);
        assert!(
            info.create_params(&Selection {
                pages: vec![3],
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn reading_progress_preserves_unrelated_and_invalid_files() {
        let temp_dir =
            std::env::temp_dir().join(format!("bili-service-test-{}", uuid::Uuid::new_v4()));
        let task_dir = temp_dir.join(".下载任务");
        std::fs::create_dir_all(&task_dir).unwrap();
        let mut progress = DownloadProgress {
            task_id: "one".to_owned(),
            ..Default::default()
        };
        progress.video_task.selected = true;
        let json = serde_json::to_string(&progress).unwrap();
        std::fs::write(task_dir.join("one.json"), &json).unwrap();
        std::fs::write(task_dir.join("keep.txt"), "handwritten").unwrap();
        let snapshots = read_persisted_tasks(&temp_dir).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].state, DownloadTaskState::Paused);
        assert_eq!(
            std::fs::read_to_string(task_dir.join("one.json")).unwrap(),
            json
        );
        progress.last_error = Some("connection reset after retry limit".to_owned());
        std::fs::write(
            task_dir.join("one.json"),
            serde_json::to_string(&progress).unwrap(),
        )
        .unwrap();
        let snapshots = read_persisted_tasks(&temp_dir).unwrap();
        assert_eq!(snapshots[0].state, DownloadTaskState::Failed);
        assert_eq!(snapshots[0].error, progress.last_error);
        // Finishing the bytes does not hide a later post-processing failure.
        progress.video_task.completed = true;
        std::fs::write(
            task_dir.join("one.json"),
            serde_json::to_string(&progress).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_persisted_tasks(&temp_dir).unwrap()[0].state,
            DownloadTaskState::Failed
        );
        std::fs::write(task_dir.join("broken.json"), "{broken").unwrap();
        assert!(read_persisted_tasks(&temp_dir).is_err());
        assert!(task_dir.join("broken.json").exists());
        assert_eq!(
            std::fs::read_to_string(task_dir.join("keep.txt")).unwrap(),
            "handwritten"
        );
        std::fs::remove_dir_all(temp_dir).unwrap();
    }
}
