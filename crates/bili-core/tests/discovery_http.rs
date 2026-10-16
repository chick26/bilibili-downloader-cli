//! Exercise the real discovery -> BiliClient -> HTTP request path without Bilibili access.

use std::{collections::BTreeMap, future::Future, pin::Pin, time::Duration};

use bili_core::{
    Runtime,
    api_error::BiliApiError,
    config::Config,
    discovery::{DiscoveryTarget, discover},
    service::MediaTarget,
    types::{
        get_bangumi_info_params::GetBangumiInfoParams, get_cheese_info_params::GetCheeseInfoParams,
        get_normal_info_params::GetNormalInfoParams,
    },
};
use reqwest::{Request, Response, Url};
use reqwest_middleware::{ClientBuilder, Extension, Next};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

fn rewrite_to_local<'a>(
    mut request: Request,
    extensions: &'a mut http::Extensions,
    next: Next<'a>,
) -> Pin<Box<dyn Future<Output = reqwest_middleware::Result<Response>> + Send + 'a>> {
    Box::pin(async move {
        let base = extensions.get::<Url>().expect("local API base URL");
        assert_eq!(request.url().host_str(), Some("api.bilibili.com"));
        let query = request.url().query().map(str::to_owned);
        let mut url = base.join(request.url().path()).unwrap();
        url.set_query(query.as_deref());
        *request.url_mut() = url;
        next.run(request, extensions).await
    })
}

struct CapturedRequest {
    url: Url,
    headers: String,
}

impl CapturedRequest {
    fn query(&self) -> BTreeMap<String, String> {
        self.url.query_pairs().into_owned().collect()
    }

    fn cookie(&self) -> &str {
        self.headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| name.eq_ignore_ascii_case("cookie").then_some(value.trim()))
            .unwrap_or_default()
    }
}

async fn mock_api(responses: Vec<Value>) -> (Url, JoinHandle<Vec<CapturedRequest>>) {
    mock_http_api(
        responses.into_iter().map(MockResponse::json).collect(),
        false,
    )
    .await
}

struct MockResponse {
    status: &'static str,
    content_type: &'static str,
    body: String,
}

impl MockResponse {
    fn json(body: Value) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/json",
            body: body.to_string(),
        }
    }

    fn risk_control() -> Self {
        Self {
            status: "412 Precondition Failed",
            content_type: "text/html; charset=utf-8",
            body: "<!DOCTYPE html><html><body>PRIVATE_412_HTML_MARKER</body></html>".into(),
        }
    }
}

async fn mock_http_api(
    responses: Vec<MockResponse>,
    assert_no_more_requests: bool,
) -> (Url, JoinHandle<Vec<CapturedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server_base = base.clone();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        for response in responses {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("discovery should issue the next API request")
                .unwrap();
            let mut bytes = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut buffer = [0; 1024];
                while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).await.unwrap();
                    assert!(read > 0, "request closed before its headers were complete");
                    bytes.extend_from_slice(&buffer[..read]);
                }
            })
            .await
            .expect("API request headers should arrive promptly");
            let headers = String::from_utf8(bytes).unwrap();
            let request_target = headers
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            captured.push(CapturedRequest {
                url: server_base.join(request_target).unwrap(),
                headers,
            });
            let message = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status,
                response.content_type,
                response.body.len(),
                response.body
            );
            stream.write_all(message.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
        if assert_no_more_requests {
            assert!(
                tokio::time::timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "risk control must stop the request flow without retrying"
            );
        }
        captured
    });
    (base, server)
}

fn wbi_response() -> Value {
    json!({
        "code": 0,
        "data": {"wbi_img": {
            "img_url": "https://i0.hdslb.com/bfs/wbi/0123456789abcdef0123456789abcdef.png",
            "sub_url": "https://i0.hdslb.com/bfs/wbi/fedcba9876543210fedcba9876543210.png"
        }}
    })
}

fn runtime_with_api(base: Url) -> (std::path::PathBuf, Runtime) {
    let data_dir = std::env::temp_dir().join(format!("bili-discovery-{}", uuid::Uuid::new_v4()));
    let runtime = Runtime::new(&data_dir, Config::defaults(&data_dir)).unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    *runtime.context().get_bili_client().api_client.write() = ClientBuilder::new(http)
        .with_init(Extension(base))
        .with(rewrite_to_local)
        .with(RetryTransientMiddleware::new_with_policy(
            ExponentialBackoff::builder().build_with_max_retries(2),
        ))
        .build();
    (data_dir, runtime)
}

#[tokio::test]
async fn keyword_search_signs_queries_keeps_the_session_and_normalizes_results() {
    use bili_core::keyword_search::{KeywordSearchType, search};

    for kind in [KeywordSearchType::User, KeywordSearchType::Video] {
        let mut nav = wbi_response();
        nav["data"]["isLogin"] = json!(true);
        nav["data"]["mid"] = json!(77);
        let entry = if kind == KeywordSearchType::User {
            json!({"mid":123,"uname":"<em class=\"keyword\">Rust</em> &amp; CLI", "fans":99,"videos":12})
        } else {
            json!({"mid":123,"bvid":"BV17x411w7KC","title":"<em class=\"keyword\">Rust</em> &amp; &lt;CLI&gt;", "author":"Local UP","duration":"1:30"})
        };
        let entries = if kind == KeywordSearchType::Video {
            json!([entry, {"id":170001,"mid":123,"bvid":"","title":"AV fallback"}, {"mid":123,"title":"Missing ID"}])
        } else {
            json!([entry])
        };
        let (base, server) = mock_api(vec![
            nav,
            json!({"code":0,"data":{"b_3":"SYNTHETIC-VISITOR"}}),
            json!({"code":0,"data":{"pagesize":20,"numResults":60,"numPages":3,"result":entries}}),
        ])
        .await;
        let (data_dir, runtime) = runtime_with_api(base);
        runtime.context().get_config().write().sessdata = "synthetic-search-session".into();
        let result = search(
            &runtime.context().get_bili_client(),
            "  Rust's & CLI  ",
            kind,
            2,
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].url.path(), "/x/web-interface/nav");
        assert_eq!(requests[1].url.path(), "/x/frontend/finger/spi");
        assert_eq!(requests[2].url.path(), "/x/web-interface/wbi/search/type");
        let query = requests[2].query();
        assert_eq!(query["keyword"], "Rusts & CLI");
        assert_eq!(query["page"], "2");
        assert_eq!(
            query["search_type"],
            if kind == KeywordSearchType::User {
                "bili_user"
            } else {
                "video"
            }
        );
        assert_eq!(query["w_rid"].len(), 32);
        assert!(requests[2].cookie().contains("buvid3=SYNTHETIC-VISITOR"));
        assert!(
            requests[2]
                .cookie()
                .contains("SESSDATA=synthetic-search-session")
        );
        assert!(requests[2].cookie().contains("DedeUserID=77"));
        assert_eq!(result.page, 2);
        assert_eq!(result.page_size, 20);
        assert_eq!(result.total, 60);
        assert!(result.has_more);
        let public = serde_json::to_value(result).unwrap();
        let item = &public["items"][0];
        assert_eq!(item["uid"], 123);
        if kind == KeywordSearchType::User {
            assert_eq!(item["name"], "Rust & CLI");
            assert_eq!(item["url"], "https://space.bilibili.com/123");
            assert_eq!(item["video_count"], 12);
        } else {
            assert_eq!(item["title"], "Rust & <CLI>");
            assert_eq!(
                public["items"][1]["url"],
                "https://www.bilibili.com/video/av170001"
            );
            assert_eq!(public["items"][1]["downloadable"], true);
            assert_eq!(public["items"][2]["downloadable"], false);
            assert_eq!(
                MediaTarget::parse(item["url"].as_str().unwrap()).unwrap(),
                MediaTarget::parse("BV17x411w7KC").unwrap()
            );
        }
        assert!(!public.to_string().contains("SYNTHETIC-VISITOR"));
        assert!(!public.to_string().contains("synthetic-search-session"));
        runtime.shutdown().await.unwrap();
        std::fs::remove_dir_all(data_dir).unwrap();
    }
}

#[tokio::test]
async fn keyword_search_returns_empty_results_and_stops_on_risk_control() {
    use bili_core::keyword_search::{KeywordSearchType, search};

    for failure in [None, Some(0), Some(1), Some(2)] {
        let mut responses = vec![MockResponse::json(wbi_response())];
        if failure == Some(0) {
            responses.push(MockResponse::risk_control());
        } else {
            responses.push(MockResponse::json(
                json!({"code":0,"data":{"b_3":"SYNTHETIC-VISITOR"}}),
            ));
            responses.push(match failure {
                Some(1) => MockResponse::risk_control(),
                Some(2) => MockResponse::json(json!({"code":-352,"message":"PRIVATE_RESPONSE_MARKER"})),
                _ => MockResponse::json(json!({"code":0,"data":{"pagesize":20,"numResults":0,"numPages":0,"result":null}})),
            });
        }
        let (base, server) = mock_http_api(responses, failure.is_some()).await;
        let (data_dir, runtime) = runtime_with_api(base);
        let result = search(
            &runtime.context().get_bili_client(),
            "不存在的用户",
            KeywordSearchType::User,
            1,
        )
        .await;
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), if failure == Some(0) { 2 } else { 3 });
        if failure.is_some() {
            let error = result.unwrap_err();
            assert!(
                error
                    .downcast_ref::<BiliApiError>()
                    .unwrap()
                    .is_risk_control()
            );
            assert!(!error.to_string().contains("PRIVATE_"));
        } else {
            let result = result.unwrap();
            assert!(result.items.is_empty());
            assert!(!result.has_more);
        }
        runtime.shutdown().await.unwrap();
        std::fs::remove_dir_all(data_dir).unwrap();
    }
}

#[tokio::test]
async fn up_discovery_issues_signed_paginated_request_and_returns_download_input() {
    let (base, server) = mock_api(vec![
        wbi_response(),
        json!({"code": 0, "data": {
            "list": {"vlist": [{
                "aid": 170001, "bvid": "BV17x411w7KC", "title": "Local UP video",
                "author": "Local UP", "mid": 123, "length": "01:30"
            }]},
            "page": {"pn": 2, "ps": 42, "count": 90}
        }}),
    ])
    .await;
    let (data_dir, runtime) = runtime_with_api(base);
    let target = DiscoveryTarget::parse("https://space.bilibili.com/123/video").unwrap();
    let result = discover(&runtime.context().get_bili_client(), &target, 2)
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.path(), "/x/web-interface/nav");
    assert_eq!(requests[1].url.path(), "/x/space/wbi/arc/search");
    let query = requests[1].query();
    assert_eq!(query["mid"], "123");
    assert_eq!(query["pn"], "2");
    assert_eq!(query["ps"], "42");
    assert_eq!(query["w_rid"].len(), 32);
    assert!(query["w_rid"].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(query["wts"].parse::<u64>().unwrap() > 0);
    assert!(requests[1].headers.to_ascii_lowercase().contains("cookie:"));
    assert_eq!(result.page, 2);
    assert_eq!(result.page_size, 42);
    assert_eq!(result.total, Some(90));
    assert!(result.has_more);
    assert_eq!(result.items.len(), 1);
    let item = &result.items[0];
    assert!(item.downloadable);
    assert_eq!(item.bvid.as_deref(), Some("BV17x411w7KC"));
    assert_eq!(
        MediaTarget::parse(item.url.as_deref().unwrap()).unwrap(),
        MediaTarget::parse(item.bvid.as_deref().unwrap()).unwrap()
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

#[tokio::test]
async fn upstream_antibot_error_is_an_error_instead_of_an_empty_success() {
    let (base, server) = mock_api(vec![
        wbi_response(),
        json!({"code": -352, "message": "request blocked", "data": null}),
    ])
    .await;
    let (data_dir, runtime) = runtime_with_api(base);
    let result = discover(
        &runtime.context().get_bili_client(),
        &DiscoveryTarget::Up { uid: 123 },
        1,
    )
    .await;
    let error = result.expect_err("a nonzero API code must never become an empty list");
    assert!(format!("{error:?}").contains("-352"));
    assert!(
        error
            .downcast_ref::<BiliApiError>()
            .expect("upstream rejection should preserve its typed error")
            .is_risk_control()
    );
    assert_eq!(server.await.unwrap().len(), 2);
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

fn empty_up_response() -> Value {
    json!({"code": 0, "data": {
        "list": {"vlist": []},
        "page": {"pn": 1, "ps": 42, "count": 0}
    }})
}

fn logged_in_nav(user_id: i64) -> Value {
    let mut nav = wbi_response();
    nav["data"]["isLogin"] = json!(true);
    nav["data"]["mid"] = json!(user_id);
    nav
}

#[tokio::test]
async fn metadata_and_playback_share_one_authenticated_session() {
    let (base, server) = mock_api(vec![
        logged_in_nav(321),
        json!({"code":0,"data":{"bvid":"BV1J4pz6qE3V","title":"Local video","cid":42}}),
        json!({"code":0,"data":{"quality":80}}),
    ])
    .await;
    let (data_dir, runtime) = runtime_with_api(base);
    runtime.context().get_config().write().sessdata = "synthetic-session-a".into();
    let client = runtime.context().get_bili_client();
    let info = client
        .get_normal_info(GetNormalInfoParams::Bvid("BV1J4pz6qE3V".into()))
        .await
        .unwrap();
    let media = client.get_normal_url(&info.bvid, info.cid).await.unwrap();
    assert_eq!(media.quality, 80);
    let requests = server.await.unwrap();
    assert_eq!(
        requests.len(),
        3,
        "nav should initialize the session only once"
    );
    assert_eq!(requests[0].url.path(), "/x/web-interface/nav");
    assert_eq!(requests[1].url.path(), "/x/web-interface/view");
    assert_eq!(requests[2].url.path(), "/x/player/wbi/playurl");
    for request in &requests[1..] {
        assert!(request.cookie().contains("DedeUserID=321;"));
        assert!(request.cookie().contains("SESSDATA=synthetic-session-a"));
    }
    assert_eq!(requests[1].query()["bvid"], "BV1J4pz6qE3V");
    assert_eq!(requests[2].query()["cid"], "42");
    runtime.shutdown().await.unwrap();
    drop(client);
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

#[tokio::test]
async fn account_cookie_change_does_not_reuse_the_previous_user_identity() {
    let (base, server) = mock_api(vec![
        logged_in_nav(321),
        json!({"code":0,"data":{}}),
        logged_in_nav(654),
        json!({"code":0,"data":{}}),
    ])
    .await;
    let (data_dir, runtime) = runtime_with_api(base);
    let client = runtime.context().get_bili_client();
    runtime.context().get_config().write().sessdata = "synthetic-session-a".into();
    client
        .get_normal_info(GetNormalInfoParams::Aid(170001))
        .await
        .unwrap();
    runtime.context().get_config().write().sessdata = "synthetic-session-b".into();
    assert!(!client.get_cookie().contains("DedeUserID"));
    client
        .get_normal_info(GetNormalInfoParams::Aid(170001))
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert!(requests[1].cookie().contains("DedeUserID=321;"));
    assert!(
        requests[1]
            .cookie()
            .contains("SESSDATA=synthetic-session-a")
    );
    assert!(!requests[2].cookie().contains("DedeUserID=321"));
    assert!(requests[3].cookie().contains("DedeUserID=654;"));
    assert!(
        requests[3]
            .cookie()
            .contains("SESSDATA=synthetic-session-b")
    );
    runtime.shutdown().await.unwrap();
    drop(client);
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

#[tokio::test]
async fn download_api_http_412_is_safe_typed_and_never_retried() {
    for path in [
        "/x/web-interface/view",
        "/x/player/wbi/playurl",
        "/x/player/wbi/v2",
        "/pgc/view/web/season",
        "/pgc/player/web/v2/playurl",
        "/pugv/view/web/season",
        "/pugv/player/web/playurl",
        "/x/web-interface/view/detail/tag",
        "/x/v2/dm/web/seg.so",
        "字幕下载",
        "封面下载",
    ] {
        let (base, server) = mock_http_api(vec![MockResponse::risk_control()], true).await;
        let (data_dir, runtime) = runtime_with_api(base);
        let client = runtime.context().get_bili_client();
        let result = match path {
            "/x/web-interface/view" => client
                .get_normal_info(GetNormalInfoParams::Aid(170001))
                .await
                .map(|_| ()),
            "/x/player/wbi/playurl" => client.get_normal_url("BV1J4pz6qE3V", 42).await.map(|_| ()),
            "/x/player/wbi/v2" => client.get_player_info(170001, 42).await.map(|_| ()),
            "/pgc/view/web/season" => client
                .get_bangumi_info(GetBangumiInfoParams::SeasonId(123))
                .await
                .map(|_| ()),
            "/pgc/player/web/v2/playurl" => client.get_bangumi_url(42).await.map(|_| ()),
            "/pugv/view/web/season" => client
                .get_cheese_info(GetCheeseInfoParams::SeasonId(123))
                .await
                .map(|_| ()),
            "/pugv/player/web/playurl" => client.get_cheese_url(42).await.map(|_| ()),
            "/x/web-interface/view/detail/tag" => client.get_tags(170001).await.map(|_| ()),
            "/x/v2/dm/web/seg.so" => client.get_danmaku(170001, 42, 10).await.map(|_| ()),
            "字幕下载" => client
                .get_subtitle("https://api.bilibili.com/local-subtitle")
                .await
                .map(|_| ()),
            "封面下载" => client
                .get_cover_data_and_ext("https://api.bilibili.com/local-cover")
                .await
                .map(|_| ()),
            _ => unreachable!(),
        };
        let error = result.expect_err("412 must not become a successful API result");
        assert_risk_error_omits_html(&error, path);
        assert_eq!(
            server.await.unwrap().len(),
            1,
            "412 must not be retried: {path}"
        );
        runtime.shutdown().await.unwrap();
        drop(client);
        drop(runtime);
        std::fs::remove_dir_all(data_dir).unwrap();
    }
}

#[tokio::test]
async fn download_api_json_risk_rejection_preserves_typed_code() {
    for code in [-352, -412] {
        let (base, server) = mock_http_api(
            vec![MockResponse::json(
                json!({"code":code,"message":"<html>PRIVATE_412_HTML_MARKER</html>","data":null}),
            )],
            true,
        )
        .await;
        let (data_dir, runtime) = runtime_with_api(base);
        let error = runtime
            .context()
            .get_bili_client()
            .get_normal_info(GetNormalInfoParams::Aid(170001))
            .await
            .expect_err("risk rejection must stay typed");
        assert_risk_error_omits_html(&error, "/x/web-interface/view");
        assert_eq!(server.await.unwrap().len(), 1);
        runtime.shutdown().await.unwrap();
        drop(runtime);
        std::fs::remove_dir_all(data_dir).unwrap();
    }
}

#[tokio::test]
async fn authenticated_up_request_sends_real_logged_in_uid_instead_of_requested_up_uid() {
    let mut nav = wbi_response();
    nav["data"]["isLogin"] = json!(true);
    nav["data"]["mid"] = json!(321);
    let (base, server) = mock_api(vec![nav, empty_up_response()]).await;
    let (data_dir, runtime) = runtime_with_api(base);
    runtime.context().get_config().write().sessdata = "synthetic-test-session".into();
    let result = discover(
        &runtime.context().get_bili_client(),
        &DiscoveryTarget::Up { uid: 123 },
        1,
    )
    .await
    .unwrap();
    assert!(result.items.is_empty());
    let requests = server.await.unwrap();
    let cookies: Vec<_> = requests[1].cookie().split(';').map(str::trim).collect();
    assert!(cookies.contains(&"SESSDATA=synthetic-test-session"));
    assert!(cookies.contains(&"DedeUserID=321"));
    assert!(!cookies.contains(&"DedeUserID=123"));
    assert!(!cookies.contains(&"DedeUserID=1"));
    assert_eq!(requests[1].query()["mid"], "123");
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

#[tokio::test]
async fn anonymous_nav_keeps_valid_wbi_keys_without_inventing_user_cookie() {
    let mut nav = wbi_response();
    nav["code"] = json!(-101);
    nav["data"]["isLogin"] = json!(false);
    nav["data"]["mid"] = json!(0);
    let (base, server) = mock_api(vec![nav, empty_up_response()]).await;
    let (data_dir, runtime) = runtime_with_api(base);
    let result = discover(
        &runtime.context().get_bili_client(),
        &DiscoveryTarget::Up { uid: 123 },
        1,
    )
    .await
    .unwrap();
    assert!(result.items.is_empty());
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(!requests[1].cookie().contains("DedeUserID"));
    assert_eq!(requests[1].query()["w_rid"].len(), 32);
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

fn assert_risk_error_omits_html(error: &eyre::Report, path: &str) {
    assert!(
        error
            .downcast_ref::<BiliApiError>()
            .expect("risk control should preserve its typed error")
            .is_risk_control()
    );
    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(
            rendered.contains(path),
            "failed API stage should be visible"
        );
        assert!(!rendered.contains("<html"));
        assert!(!rendered.contains("<!DOCTYPE"));
        assert!(!rendered.contains("PRIVATE_412_HTML_MARKER"));
    }
}

#[tokio::test]
async fn nav_http_412_is_typed_and_safe_and_stops_before_up_request() {
    let (base, server) = mock_http_api(vec![MockResponse::risk_control()], true).await;
    let (data_dir, runtime) = runtime_with_api(base);
    let error = discover(
        &runtime.context().get_bili_client(),
        &DiscoveryTarget::Up { uid: 123 },
        1,
    )
    .await
    .expect_err("a rejected nav request must stop UP discovery");
    assert_risk_error_omits_html(&error, "/x/web-interface/nav");
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/x/web-interface/nav");
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

#[tokio::test]
async fn up_http_412_is_typed_and_safe_without_automatic_retry() {
    let (base, server) = mock_http_api(
        vec![
            MockResponse::json(wbi_response()),
            MockResponse::risk_control(),
        ],
        true,
    )
    .await;
    let (data_dir, runtime) = runtime_with_api(base);
    let error = discover(
        &runtime.context().get_bili_client(),
        &DiscoveryTarget::Up { uid: 123 },
        1,
    )
    .await
    .expect_err("a rejected UP request must not become an empty success");
    assert_risk_error_omits_html(&error, "/x/space/wbi/arc/search");
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].url.path(), "/x/space/wbi/arc/search");
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}

#[tokio::test]
async fn favorite_discovery_uses_original_paging_and_marks_invalid_entries() {
    let (base, server) = mock_api(vec![json!({"code": 0, "data": {
        "info": {"title": "Local favorites", "media_count": 37},
        "medias": [
            {"id": 170001, "type": 2, "bvid": "BV17x411w7KC", "attr": 0, "title": "Video"},
            {"id": 170002, "type": 2, "bvid": "BV1xx411c7mD", "attr": 9, "title": "Unavailable"}
        ],
        "has_more": false
    }})])
    .await;
    let (data_dir, runtime) = runtime_with_api(base);
    let result = discover(
        &runtime.context().get_bili_client(),
        &DiscoveryTarget::Favorite { fid: 456 },
        2,
    )
    .await
    .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/x/v3/fav/resource/list");
    let query = requests[0].query();
    assert_eq!(query["media_id"], "456");
    assert_eq!(query["pn"], "2");
    assert_eq!(query["ps"], "36");
    assert_eq!(result.page_size, 36);
    assert_eq!(result.total, Some(37));
    assert!(!result.has_more);
    assert!(result.items[0].downloadable);
    assert!(!result.items[1].downloadable);
    assert!(result.items[1].url.is_none());
    assert!(result.items[1].unavailable_reason.is_some());
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(data_dir).unwrap();
}
