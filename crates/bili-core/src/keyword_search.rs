//! Small, read-only keyword search using the same session and WBI signing as discovery.

use eyre::{OptionExt, bail};
use serde::{Deserialize, Serialize};

use crate::{
    api_error::{BiliApiError, check_code, read_response},
    bili_client::BiliClient,
    discovery::video_reference,
};

const ENDPOINT: &str = "/x/web-interface/wbi/search/type";
const VISITOR_ENDPOINT: &str = "/x/frontend/finger/spi";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeywordSearchType {
    User,
    Video,
}

#[derive(Debug, Serialize)]
pub struct KeywordSearchResult {
    pub search_type: KeywordSearchType,
    pub keyword: String,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub has_more: bool,
    pub items: Vec<KeywordSearchItem>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeywordSearchItem {
    User {
        uid: i64,
        name: String,
        fans: u64,
        video_count: u64,
        url: String,
    },
    Video {
        bvid: Option<String>,
        aid: Option<i64>,
        title: String,
        author: String,
        uid: i64,
        duration: String,
        url: Option<String>,
        downloadable: bool,
        unavailable_reason: Option<String>,
    },
}

#[derive(Deserialize)]
struct SearchPage {
    #[serde(rename = "pagesize")]
    page_size: u32,
    #[serde(rename = "numResults")]
    total: u64,
    #[serde(rename = "numPages")]
    pages: u32,
    result: Option<Vec<SearchEntry>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct SearchEntry {
    id: i64,
    mid: i64,
    uname: String,
    fans: u64,
    videos: u64,
    bvid: String,
    title: String,
    author: String,
    duration: String,
}

pub async fn search(
    client: &BiliClient,
    keyword: &str,
    kind: KeywordSearchType,
    page: u32,
) -> eyre::Result<KeywordSearchResult> {
    let keyword = keyword.trim();
    if keyword.is_empty() || page == 0 {
        bail!("搜索关键词不能为空，页码必须大于 0");
    }
    // WBI requires these characters to be removed from both the signed values and query.
    let query_keyword: String = keyword.chars().filter(|c| !"!'()*".contains(*c)).collect();
    if query_keyword.trim().is_empty() {
        bail!("搜索关键词需要包含文字或数字");
    }
    let mut params = vec![
        ("keyword", query_keyword),
        (
            "search_type",
            match kind {
                KeywordSearchType::User => "bili_user",
                KeywordSearchType::Video => "video",
            }
            .into(),
        ),
        ("page", page.to_string()),
    ];
    client.wbi(&mut params).await?;

    // Bilibili search also requires a visitor ID, even for anonymous requests.
    // Obtain it from the site's API for this invocation; never expose it in results.
    let request = client
        .api_client
        .read()
        .get(format!("https://api.bilibili.com{VISITOR_ENDPOINT}"))
        .header("cookie", client.get_cookie());
    let visitor = read_response(request.send().await?, VISITOR_ENDPOINT).await?;
    check_code(&visitor, VISITOR_ENDPOINT)?;
    let buvid = visitor
        .data
        .as_ref()
        .and_then(|data| data["b_3"].as_str())
        .filter(|value| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        .ok_or(BiliApiError::InvalidResponse {
            endpoint: VISITOR_ENDPOINT,
        })?;
    let request = client
        .api_client
        .read()
        .get(format!("https://api.bilibili.com{ENDPOINT}"))
        .query(&params)
        .header("cookie", format!("{}; buvid3={buvid}", client.get_cookie()));
    let response = read_response(request.send().await?, ENDPOINT).await?;
    check_code(&response, ENDPOINT)?;
    let data = response.data.ok_or_eyre("搜索接口缺少 data")?;
    let data: SearchPage = serde_json::from_value(data)
        .map_err(|_| BiliApiError::InvalidResponse { endpoint: ENDPOINT })?;
    if data.page_size == 0 {
        return Err(BiliApiError::InvalidResponse { endpoint: ENDPOINT }.into());
    }
    let items = data
        .result
        .unwrap_or_default()
        .into_iter()
        .map(|entry| normalize(entry, kind))
        .collect::<eyre::Result<Vec<_>>>()?;
    Ok(KeywordSearchResult {
        search_type: kind,
        keyword: keyword.into(),
        page,
        page_size: data.page_size,
        total: data.total,
        has_more: !items.is_empty() && page < data.pages,
        items,
    })
}

fn normalize(entry: SearchEntry, kind: KeywordSearchType) -> eyre::Result<KeywordSearchItem> {
    if kind == KeywordSearchType::User && entry.mid <= 0 {
        return Err(BiliApiError::InvalidResponse { endpoint: ENDPOINT }.into());
    }
    Ok(match kind {
        KeywordSearchType::User => KeywordSearchItem::User {
            uid: entry.mid,
            name: plain_text(&entry.uname),
            fans: entry.fans,
            video_count: entry.videos,
            url: format!("https://space.bilibili.com/{}", entry.mid),
        },
        KeywordSearchType::Video => {
            let (bvid, url) = video_reference(&entry.bvid, entry.id);
            let downloadable = url.is_some();
            KeywordSearchItem::Video {
                url,
                bvid,
                aid: (entry.id > 0).then_some(entry.id),
                downloadable,
                unavailable_reason: (!downloadable).then(|| "缺少有效视频 ID".into()),
                title: plain_text(&entry.title),
                author: plain_text(&entry.author),
                uid: entry.mid,
                duration: entry.duration,
            }
        }
    })
}

/// Search returns HTML fragments with highlighted keywords, rather than plain titles.
fn plain_text(html: &str) -> String {
    let mut in_tag = false;
    let text: String = html
        .chars()
        .filter(|c| match c {
            '<' => {
                in_tag = true;
                false
            }
            '>' => {
                in_tag = false;
                false
            }
            _ => !in_tag,
        })
        .collect();
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}
