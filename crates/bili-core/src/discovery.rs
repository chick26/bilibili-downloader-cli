//! Read-only media discovery shared by CLI entry points.

use eyre::{WrapErr, bail, eyre};
use serde::{Deserialize, Serialize};

use crate::{
    bili_client::BiliClient,
    service::{MediaInfo, MediaMetadata, MediaTarget, NormalId, lookup},
    types::{
        fav_folders::FavFolders, fav_info::FavInfo, get_fav_info_params::GetFavInfoParams,
        get_user_video_info_params::GetUserVideoInfoParams, user_video_info::UserVideoInfo,
    },
};

// Keep these aligned with the existing BiliClient API request sizes.
const UP_PAGE_SIZE: u32 = 42;
const FAVORITE_PAGE_SIZE: u32 = 36;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoveryTarget {
    Up { uid: i64 },
    Favorite { fid: i64 },
    Media { target: MediaTarget },
}

impl DiscoveryTarget {
    /// Recognize the GUI's uid/fid identifiers and complete Bilibili URLs.
    /// Bare numbers are deliberately ambiguous and require an explicit command.
    pub fn parse(source: &str) -> eyre::Result<Self> {
        let source = source.trim();
        if let Some(id) = prefixed_id(source, "uid") {
            return Ok(Self::Up { uid: id? });
        }
        if let Some(id) = prefixed_id(source, "fid") {
            return Ok(Self::Favorite { fid: id? });
        }
        if source.contains("://") {
            let url = reqwest::Url::parse(source).wrap_err("无法解析搜索 URL")?;
            let host = url.host_str().unwrap_or_default();
            if !matches!(url.scheme(), "http" | "https")
                || (host != "bilibili.com" && !host.ends_with(".bilibili.com"))
            {
                bail!("仅支持 bilibili.com 的 http/https URL");
            }
            let segments: Vec<_> = url
                .path_segments()
                .into_iter()
                .flatten()
                .filter(|segment| !segment.is_empty())
                .collect();
            let is_space_favorite =
                host == "space.bilibili.com" && matches!(segments.as_slice(), [_, "favlist"]);
            let is_medialist = segments.first() == Some(&"medialist");
            if is_space_favorite || is_medialist {
                let fid_values: Vec<_> = url
                    .query_pairs()
                    .filter(|(key, _)| key == "fid")
                    .map(|(_, value)| value.into_owned())
                    .collect();
                if fid_values.len() > 1 {
                    bail!("收藏夹 URL 不能重复指定 fid");
                }
                if let Some(fid) = fid_values.first() {
                    return Ok(Self::Favorite {
                        fid: positive_id(fid)?,
                    });
                }
                if is_medialist
                    && let ["medialist", "play" | "detail", identifier] = segments.as_slice()
                    && let Some(fid) = prefixed_id(identifier, "ml")
                {
                    return Ok(Self::Favorite { fid: fid? });
                }
                bail!("收藏夹 URL 需要 fid 参数或 /medialist/play/ml… 路径");
            }
            if host == "space.bilibili.com" {
                let uid = match segments.as_slice() {
                    [uid] | [uid, "video"] | [uid, "upload", "video"] => *uid,
                    _ => bail!("请使用 UP 空间主页或投稿页 URL"),
                };
                return Ok(Self::Up {
                    uid: positive_id(uid)?,
                });
            }
        }
        MediaTarget::parse(source)
            .map(|target| Self::Media { target })
            .wrap_err("请使用视频 URL、BV/av/ep/ss ID、uid…、fid…、UP 空间或收藏夹 URL")
    }

    pub fn parse_up(source: &str) -> eyre::Result<Self> {
        if source.trim().bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(Self::Up {
                uid: positive_id(source.trim())?,
            });
        }
        match Self::parse(source)? {
            target @ Self::Up { .. } => Ok(target),
            _ => bail!("请提供 UP 的 UID、uid… 或空间 URL"),
        }
    }

    pub fn parse_favorite(source: &str) -> eyre::Result<Self> {
        if source.trim().bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(Self::Favorite {
                fid: positive_id(source.trim())?,
            });
        }
        match Self::parse(source)? {
            target @ Self::Favorite { .. } => Ok(target),
            _ => bail!("请提供收藏夹的 FID、fid… 或收藏夹 URL"),
        }
    }
}

fn positive_id(value: &str) -> eyre::Result<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| eyre!("UID/FID 必须是有效的正整数"))
}

fn prefixed_id(value: &str, prefix: &str) -> Option<eyre::Result<i64>> {
    value
        .get(..prefix.len())
        .filter(|start| start.eq_ignore_ascii_case(prefix))
        .map(|_| positive_id(&value[prefix.len()..]))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResult {
    pub target: DiscoveryTarget,
    pub title: Option<String>,
    pub page: u32,
    pub page_size: u32,
    /// Upstream total, including favorite entries that cannot be downloaded.
    pub total: Option<u64>,
    pub has_more: bool,
    pub items: Vec<DiscoveryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryItem {
    pub title: String,
    pub bvid: Option<String>,
    /// Canonical input accepted by `bili download`, when downloadable is true.
    pub url: Option<String>,
    pub author: Option<String>,
    pub duration: Option<String>,
    pub downloadable: bool,
    pub unavailable_reason: Option<String>,
}

/// A discovery call never creates downloads and propagates upstream errors.
pub async fn discover(
    client: &BiliClient,
    target: &DiscoveryTarget,
    page: u32,
) -> eyre::Result<DiscoveryResult> {
    if page == 0 {
        bail!("页码必须大于 0");
    }
    match target {
        DiscoveryTarget::Up { uid } => {
            positive_id(&uid.to_string())?;
            let info = client
                .get_user_video_info(GetUserVideoInfoParams {
                    mid: *uid,
                    pn: i64::from(page),
                })
                .await?;
            Ok(up_result(*uid, page, info))
        }
        DiscoveryTarget::Favorite { fid } => {
            positive_id(&fid.to_string())?;
            let info = client
                .get_fav_info(GetFavInfoParams {
                    media_list_id: *fid,
                    pn: i64::from(page),
                })
                .await?;
            Ok(favorite_result(*fid, page, info))
        }
        DiscoveryTarget::Media { target: media } => {
            if page != 1 {
                bail!("单个视频或番剧查询不支持翻页；请使用 info 查看分 P 或剧集");
            }
            let info = lookup(client, media).await?;
            Ok(media_result(info))
        }
    }
}

pub(crate) fn video_reference(bvid: &str, aid: i64) -> (Option<String>, Option<String>) {
    if let Ok(MediaTarget::Normal {
        id: NormalId::Bvid(id),
        ..
    }) = MediaTarget::parse(bvid)
    {
        let url = format!("https://www.bilibili.com/video/{id}");
        (Some(id), Some(url))
    } else if aid > 0 {
        (
            None,
            Some(format!("https://www.bilibili.com/video/av{aid}")),
        )
    } else {
        (None, None)
    }
}

fn up_result(uid: i64, page: u32, info: UserVideoInfo) -> DiscoveryResult {
    let page_size = u32::try_from(info.page.ps)
        .ok()
        .filter(|size| *size > 0)
        .unwrap_or(UP_PAGE_SIZE);
    let total = u64::try_from(info.page.count).ok();
    let items: Vec<_> = info
        .list
        .vlist
        .into_iter()
        .map(|video| {
            let (bvid, url) = video_reference(&video.bvid, video.aid);
            let downloadable = url.is_some();
            DiscoveryItem {
                title: video.title,
                bvid,
                url,
                author: Some(video.author),
                duration: Some(video.length),
                downloadable,
                unavailable_reason: (!downloadable).then(|| "缺少有效视频 ID".to_owned()),
            }
        })
        .collect();
    let has_more = !items.is_empty()
        && total.is_some_and(|count| u64::from(page) * u64::from(page_size) < count);
    DiscoveryResult {
        target: DiscoveryTarget::Up { uid },
        title: None,
        page,
        page_size,
        total,
        has_more,
        items,
        media: None,
    }
}

fn favorite_result(fid: i64, page: u32, info: FavInfo) -> DiscoveryResult {
    let items = info
        .medias
        .unwrap_or_default()
        .into_iter()
        .map(|video| {
            let id = if video.bvid.is_empty() {
                &video.bv_id
            } else {
                &video.bvid
            };
            let (bvid, mut url) = video_reference(id, video.id);
            let unavailable_reason = if video.attr != 0 {
                Some("收藏条目已失效或不可访问".to_owned())
            } else if video.type_field != 2 {
                Some(format!(
                    "收藏条目类型 {} 不是普通视频，请单独查询对应媒体",
                    video.type_field
                ))
            } else if url.is_none() {
                Some("缺少有效视频 ID".to_owned())
            } else {
                None
            };
            // Do not emit an av URL for IDs belonging to audio or PGC entries.
            if unavailable_reason.is_some() {
                url = None;
            }
            DiscoveryItem {
                title: video.title,
                bvid,
                url,
                author: Some(video.upper.name),
                duration: Some(video.duration.to_string()),
                downloadable: unavailable_reason.is_none(),
                unavailable_reason,
            }
        })
        .collect();
    DiscoveryResult {
        target: DiscoveryTarget::Favorite { fid },
        title: Some(info.info.title),
        page,
        page_size: FAVORITE_PAGE_SIZE,
        total: u64::try_from(info.info.media_count).ok(),
        has_more: info.has_more,
        items,
        media: None,
    }
}

fn media_result(info: MediaInfo) -> DiscoveryResult {
    let (title, author, bvid) = match &info.info {
        MediaMetadata::Normal(media) => (
            media.title.clone(),
            Some(media.owner.name.clone()),
            Some(media.bvid.clone()),
        ),
        MediaMetadata::Bangumi(media) => (media.title.clone(), None, None),
        MediaMetadata::Cheese(media) => (media.title.clone(), None, None),
    };
    let url = match &info.target {
        MediaTarget::Normal { id, page } => {
            let id = match id {
                NormalId::Bvid(id) => id.clone(),
                NormalId::Aid(id) => format!("av{id}"),
            };
            let mut url = format!("https://www.bilibili.com/video/{id}");
            if let Some(page) = page {
                url.push_str(&format!("?p={page}"));
            }
            url
        }
        MediaTarget::BangumiEpisode { id } => {
            format!("https://www.bilibili.com/bangumi/play/ep{id}")
        }
        MediaTarget::BangumiSeason { id } => {
            format!("https://www.bilibili.com/bangumi/play/ss{id}")
        }
        MediaTarget::CheeseEpisode { id } => {
            format!("https://www.bilibili.com/cheese/play/ep{id}")
        }
        MediaTarget::CheeseSeason { id } => {
            format!("https://www.bilibili.com/cheese/play/ss{id}")
        }
    };
    DiscoveryResult {
        target: DiscoveryTarget::Media {
            target: info.target.clone(),
        },
        title: Some(title.clone()),
        page: 1,
        page_size: 1,
        total: Some(1),
        has_more: false,
        items: vec![DiscoveryItem {
            title,
            bvid,
            url: Some(url),
            author,
            duration: None,
            downloadable: true,
            unavailable_reason: None,
        }],
        media: Some(info),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoriteFoldersResult {
    pub uid: i64,
    pub total: u64,
    pub items: Vec<FavoriteFolderItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoriteFolderItem {
    pub fid: i64,
    pub title: String,
    pub media_count: u64,
    pub url: String,
}

pub async fn favorite_folders(
    client: &BiliClient,
    uid: i64,
) -> eyre::Result<FavoriteFoldersResult> {
    positive_id(&uid.to_string())?;
    let folders = client.get_fav_folders(uid).await?;
    Ok(folder_result(uid, folders))
}

fn folder_result(uid: i64, folders: FavFolders) -> FavoriteFoldersResult {
    FavoriteFoldersResult {
        uid,
        total: u64::try_from(folders.count).unwrap_or_default(),
        items: folders
            .list
            .into_iter()
            .map(|folder| FavoriteFolderItem {
                // The resource-list endpoint uses folder.id (not folder.fid).
                fid: folder.id,
                title: folder.title,
                media_count: u64::try_from(folder.media_count).unwrap_or_default(),
                url: format!("https://space.bilibili.com/{uid}/favlist?fid={}", folder.id),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        fav_folders::Folder,
        fav_info::{Info, MediaInFav},
        user_video_info::{EpInUserVideo, PageInUserVideo, UserVideoList},
    };

    #[test]
    fn parses_up_and_favorite_identifiers_and_urls() {
        for input in [
            "uid123",
            " UID123 ",
            "https://space.bilibili.com/123",
            "https://space.bilibili.com/123/video/?spm_id_from=333",
            "https://space.bilibili.com/123/upload/video",
        ] {
            assert_eq!(
                DiscoveryTarget::parse(input).unwrap(),
                DiscoveryTarget::Up { uid: 123 }
            );
        }
        for input in [
            "fid456",
            "FID456",
            "https://space.bilibili.com/123/favlist?fid=456&ftype=create",
            "https://www.bilibili.com/medialist/play/456?fid=456",
            "https://www.bilibili.com/medialist/play/ml456",
            "https://www.bilibili.com/medialist/detail/ml456",
        ] {
            assert_eq!(
                DiscoveryTarget::parse(input).unwrap(),
                DiscoveryTarget::Favorite { fid: 456 }
            );
        }
    }

    #[test]
    fn media_inputs_reuse_existing_strict_parser() {
        for input in [
            "BV1xx411c7mD",
            "av123",
            "ep42",
            "ss42",
            "https://www.bilibili.com/video/BV1xx411c7mD?p=2",
        ] {
            assert!(matches!(
                DiscoveryTarget::parse(input).unwrap(),
                DiscoveryTarget::Media { .. }
            ));
        }
    }

    #[test]
    fn bare_numbers_require_explicit_target() {
        assert!(DiscoveryTarget::parse("123").is_err());
        assert_eq!(
            DiscoveryTarget::parse_up("123").unwrap(),
            DiscoveryTarget::Up { uid: 123 }
        );
        assert_eq!(
            DiscoveryTarget::parse_favorite("123").unwrap(),
            DiscoveryTarget::Favorite { fid: 123 }
        );
        assert!(DiscoveryTarget::parse_up("fid123").is_err());
        assert!(DiscoveryTarget::parse_favorite("uid123").is_err());
    }

    #[test]
    fn rejects_invalid_ids_hosts_and_conflicting_queries() {
        for input in [
            "",
            "uid0",
            "uid-1",
            "uid+1",
            "uid１２３",
            "uid12abc",
            "fid1.5",
            "fid999999999999999999999999",
            "https://space.bilibili.com.evil.test/123/video",
            "https://evil.test/medialist/play/ml123?fid=123",
            "file://space.bilibili.com/123",
            "https://space.bilibili.com/0/video",
            "https://space.bilibili.com/123/favlist?fid=1&fid=2",
            "https://space.bilibili.com/123/favlist?fid=0",
            "https://space.bilibili.com/123/favlist?fid=12oops",
            "https://space.bilibili.com/123/favlist",
            "https://www.bilibili.com/medialist/play/ml0",
        ] {
            assert!(
                DiscoveryTarget::parse(input).is_err(),
                "unexpectedly accepted {input}"
            );
        }
        assert!(DiscoveryTarget::parse_up("").is_err());
        assert!(DiscoveryTarget::parse_favorite("-1").is_err());
    }

    #[test]
    fn up_page_maps_download_inputs_and_pagination() {
        let info = UserVideoInfo {
            list: UserVideoList {
                vlist: vec![EpInUserVideo {
                    title: "Demo".into(),
                    bvid: "BV1xx411c7mD".into(),
                    author: "UP".into(),
                    length: "01:30".into(),
                    ..Default::default()
                }],
            },
            page: PageInUserVideo {
                pn: 2,
                ps: 42,
                count: 90,
            },
        };
        let result = up_result(123, 2, info.clone());
        assert_eq!(result.page, 2);
        assert_eq!(result.page_size, 42);
        assert_eq!(result.total, Some(90));
        assert!(result.has_more);
        assert_eq!(
            result.items[0].url.as_deref(),
            Some("https://www.bilibili.com/video/BV1xx411c7mD")
        );
        assert!(result.items[0].downloadable);
        assert!(!up_result(123, 3, info).has_more);
    }

    #[test]
    fn empty_up_result_is_successful_without_more_pages() {
        let result = up_result(123, 1, UserVideoInfo::default());
        assert!(result.items.is_empty());
        assert!(!result.has_more);
        assert_eq!(result.total, Some(0));
    }

    #[test]
    fn favorites_preserve_unavailable_and_nonvideo_entries() {
        let item = MediaInFav {
            id: 123,
            bvid: "BV1xx411c7mD".into(),
            type_field: 2,
            ..Default::default()
        };
        let info = FavInfo {
            info: Info {
                title: "Collection".into(),
                media_count: 4,
                ..Default::default()
            },
            medias: Some(vec![
                item.clone(),
                MediaInFav {
                    attr: 9,
                    ..item.clone()
                },
                MediaInFav {
                    type_field: 12,
                    ..item.clone()
                },
                MediaInFav {
                    type_field: 24,
                    ..item
                },
            ]),
            has_more: true,
            ..Default::default()
        };
        let result = favorite_result(456, 2, info);
        assert_eq!(result.page, 2);
        assert_eq!(result.total, Some(4));
        assert!(result.has_more);
        assert!(result.items[0].downloadable);
        for item in &result.items[1..] {
            assert!(!item.downloadable);
            assert!(item.url.is_none());
            assert!(item.unavailable_reason.is_some());
        }
        assert!(favorite_result(456, 1, FavInfo::default()).items.is_empty());
    }

    #[test]
    fn favorite_folder_uses_resource_id_and_round_trips_url() {
        let result = folder_result(
            123,
            FavFolders {
                count: 1,
                list: vec![Folder {
                    id: 456,
                    fid: 7,
                    media_count: 8,
                    title: "Favorites".into(),
                    ..Default::default()
                }],
            },
        );
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].fid, 456);
        assert_eq!(result.items[0].media_count, 8);
        assert_eq!(
            DiscoveryTarget::parse(&result.items[0].url).unwrap(),
            DiscoveryTarget::Favorite { fid: 456 }
        );
    }
}
