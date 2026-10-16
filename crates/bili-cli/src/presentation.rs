use serde_json::Value;

use crate::format::{display_width, format_bytes, sanitize, truncate, wrap};

/// Render only the useful public fields; machine output keeps its own JSON path.
pub fn render_result(result: &Value, width: usize) -> String {
    let width = width.max(24);
    let mut lines = Vec::new();
    if result.get("search_type").is_some() {
        render_search(result, width, &mut lines);
    } else if result.get("authenticated").is_some() {
        render_auth(result, &mut lines);
    } else if let Some(task) = result.get("task") {
        lines.push("任务详情 · 磁盘快照".into());
        render_task(task, width, true, &mut lines);
    } else if let Some(tasks) = result.get("tasks").and_then(Value::as_array) {
        render_tasks(result, tasks, width, &mut lines);
    } else if result.get("items").is_some() && result.get("target").is_some() {
        if let Some(media) = result.get("media") {
            render_media(media, width, &mut lines);
        } else {
            render_discovery(result, width, &mut lines);
        }
    } else if result.get("items").is_some() && result.get("uid").is_some() {
        render_folders(result, width, &mut lines);
    } else if let Some(media) = result.get("media") {
        render_media(media, width, &mut lines);
    } else {
        lines.push("操作完成。使用 --json 查看结构化结果。".into());
    }
    lines.join("\n")
}

fn render_search(result: &Value, width: usize, lines: &mut Vec<String>) {
    let is_user = result["search_type"].as_str() == Some("user");
    let kind = if is_user { "UP" } else { "视频" };
    let keyword = text(result, "keyword");
    let page = result["page"].as_u64().unwrap_or(1);
    let page_size = result["page_size"].as_u64().unwrap_or(20);
    let items = array(result, "items");
    lines.push(fit(&format!("搜索{kind} · {keyword}"), width));
    lines.push(format!(
        "共 {} 条 · 第 {page} 页 · 本页 {} 条",
        number(result, "total"),
        items.len()
    ));
    lines.push(String::new());
    let wide = width >= 72;
    if items.is_empty() {
        lines.push("没有匹配结果。".into());
    } else if wide {
        lines.push(if is_user {
            format!("{}  {}  名称", pad("序号", 4), pad("UID", 20))
        } else {
            format!(
                "{}  {}  {}  标题",
                pad("序号", 4),
                pad("时长", 8),
                pad("视频ID", 20)
            )
        });
        lines.push("─".repeat(width));
    }
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    for (index, item) in items.iter().enumerate() {
        let ordinal = offset.saturating_add(index as u64).saturating_add(1);
        let reference = if is_user {
            number(item, "uid")
        } else if let Some(bvid) = item["bvid"].as_str() {
            bvid.into()
        } else if let Some(aid) = item["aid"].as_i64() {
            format!("av{aid}")
        } else {
            "—".into()
        };
        let title = text(item, if is_user { "name" } else { "title" });
        let title = if item["downloadable"].as_bool() == Some(false) {
            format!("[不可下载] {title}")
        } else {
            title
        };
        let duration = duration(&item["duration"], false);
        if wide && is_user {
            lines.push(format!(
                "{ordinal:>4}  {}  {}",
                pad(&reference, 20),
                fit(&title, width.saturating_sub(28))
            ));
        } else if wide {
            lines.push(format!(
                "{ordinal:>4}  {}  {}  {}",
                pad(&duration, 8),
                pad(&reference, 20),
                fit(&title, width.saturating_sub(38))
            ));
        } else {
            lines.push(fit(&format!("{ordinal}. {title}"), width));
            lines.push(if is_user {
                format!("   UID {reference}")
            } else {
                format!("   {duration} · {reference}")
            });
        }
        let detail = if is_user {
            format!(
                "粉丝 {} · 投稿 {}",
                number(item, "fans"),
                number(item, "video_count")
            )
        } else {
            format!("UP：{} · UID {}", text(item, "author"), number(item, "uid"))
        };
        lines.push(format!("   {}", fit(&detail, width.saturating_sub(3))));
    }
    if is_user && let Some(first) = items.first() {
        lines.push(String::new());
        lines.push(format!("查看投稿：bili up videos {}", number(first, "uid")));
    } else if let Some(source) = items.iter().find_map(download_source) {
        lines.push(String::new());
        lines.push(format!("下载：bili download {}", shell_arg(&source)));
    }
    if result["has_more"].as_bool() == Some(true) {
        lines.push(format!(
            "下一页：bili search {} --type {} --page {}",
            shell_arg(&keyword),
            if is_user { "user" } else { "video" },
            page.saturating_add(1)
        ));
    } else {
        lines.push("已到最后一页。".into());
    }
}

fn render_auth(result: &Value, lines: &mut Vec<String>) {
    if result["authenticated"].as_bool() != Some(true) {
        lines.push("尚未登录".into());
        lines.push("登录：bili auth login".into());
        return;
    }
    let user = &result["user"];
    lines.push(format!("已登录 · {}", text(user, "name")));
    lines.push(format!("UID：{}", number(user, "id")));
    if user["vip_status"].as_i64().is_some_and(|status| status > 0) {
        lines.push("大会员：有效".into());
    }
}

fn render_discovery(result: &Value, width: usize, lines: &mut Vec<String>) {
    let target = &result["target"];
    let is_up = target["kind"].as_str() == Some("up");
    let id = number(target, if is_up { "uid" } else { "fid" });
    let title = text(result, "title");
    lines.push(if is_up {
        format!("UP 投稿 · UID {id}")
    } else if title.is_empty() {
        format!("收藏夹 · FID {id}")
    } else {
        fit(&format!("收藏夹 · {title}"), width)
    });
    if !is_up && !title.is_empty() {
        lines.push(format!("FID：{id}"));
    }
    let page = result["page"].as_u64().unwrap_or(1);
    let page_size = result["page_size"].as_u64().unwrap_or(1);
    let items = array(result, "items");
    if is_up
        && let Some(author) = items
            .iter()
            .map(|item| text(item, "author"))
            .find(|author| !author.is_empty())
    {
        lines.push(fit(&format!("UP：{author}"), width));
    }
    let total = result["total"]
        .as_u64()
        .map_or_else(|| "总数未知".into(), |total| format!("共 {total} 条"));
    let pages = result["total"]
        .as_u64()
        .filter(|_| page_size > 0)
        .map(|total| total.div_ceil(page_size));
    lines.push(match pages {
        Some(pages) if pages > 0 => {
            format!("{total} · 第 {page}/{pages} 页 · 本页 {} 条", items.len())
        }
        _ => format!("{total} · 第 {page} 页 · 本页 {} 条", items.len()),
    });
    lines.push(String::new());
    if items.is_empty() {
        lines.push("本页没有视频。".into());
    } else {
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let index_width = offset
            .saturating_add(items.len() as u64)
            .to_string()
            .len()
            .max(4);
        let fixed_width = index_width + 8 + 12 + 6;
        let table = width >= 72;
        if table {
            lines.push(format!(
                "{}  {}  {}  标题",
                pad("序号", index_width),
                pad("时长", 8),
                pad("BV号", 12),
            ));
            lines.push("─".repeat(width));
        }
        for (index, item) in items.iter().enumerate() {
            let ordinal = offset.saturating_add(index as u64).saturating_add(1);
            let reference = text(item, "bvid");
            let reference = if reference.is_empty() {
                "—"
            } else {
                &reference
            };
            let duration = duration(&item["duration"], false);
            let title = text(item, "title");
            let unavailable = item["downloadable"].as_bool() == Some(false);
            let title = if unavailable {
                format!("[不可下载] {title}")
            } else {
                title
            };
            if table {
                lines.push(format!(
                    "{:>index_width$}  {}  {}  {}",
                    ordinal,
                    pad(&duration, 8),
                    pad(reference, 12),
                    fit(&title, width.saturating_sub(fixed_width)),
                ));
            } else {
                lines.push(fit(&format!("{ordinal}. {title}"), width));
                lines.push(format!("   {duration} · {reference}"));
            }
            let author = text(item, "author");
            if !author.is_empty() && !is_up {
                let indent = if table { index_width + 2 } else { 3 };
                lines.push(format!(
                    "{}{}",
                    " ".repeat(indent),
                    fit(&format!("UP：{author}"), width.saturating_sub(indent)),
                ));
            }
            if unavailable {
                lines.push(fit(
                    &format!("   {}", text(item, "unavailable_reason")),
                    width,
                ));
            }
        }
    }
    let first_source = items.iter().find_map(download_source);
    if let Some(source) = first_source {
        lines.push(String::new());
        lines.push(format!("下载：bili download {}", shell_arg(&source)));
    }
    if result["has_more"].as_bool() == Some(true) {
        let command = if is_up { "up videos" } else { "fav videos" };
        lines.push(format!(
            "下一页：bili {command} {id} --page {}",
            page.saturating_add(1)
        ));
    } else {
        lines.push("已到最后一页。".into());
    }
}

fn render_folders(result: &Value, width: usize, lines: &mut Vec<String>) {
    lines.push(format!("收藏夹 · UID {}", number(result, "uid")));
    let folders = array(result, "items");
    lines.push(format!("共 {} 个收藏夹", number(result, "total")));
    if folders.is_empty() {
        lines.push("没有可见的收藏夹。".into());
        return;
    }
    lines.push(String::new());
    for (index, folder) in folders.iter().enumerate() {
        lines.push(fit(
            &format!("{}. {}", index + 1, text(folder, "title")),
            width,
        ));
        lines.push(format!(
            "   FID {} · {} 条视频",
            number(folder, "fid"),
            number(folder, "media_count")
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "查看：bili fav videos {}",
        number(&folders[0], "fid")
    ));
}

fn render_media(media: &Value, width: usize, lines: &mut Vec<String>) {
    let metadata = &media["info"];
    let info = &metadata["info"];
    let kind = metadata["kind"].as_str().unwrap_or_default();
    let title = text(info, "title");
    lines.push(fit(&title, width));
    let source = media_source(&media["target"]);
    if kind == "normal" {
        let reference = format!("{} · UP：", text(info, "bvid"));
        lines.push(format!(
            "{reference}{}",
            fit(
                &text(&info["owner"], "name"),
                width.saturating_sub(display_width(&reference)),
            )
        ));
        let parts = array(info, "pages");
        lines.push(format!(
            "时长 {} · {} 个分 P",
            duration(&info["duration"], false),
            parts.len()
        ));
        let description = text(info, "desc");
        if !description.is_empty() {
            lines.push(fit(&format!("简介：{description}"), width));
        }
        lines.push(String::new());
        for (index, part) in parts.iter().enumerate() {
            let page = part["page"].as_u64().unwrap_or(index as u64 + 1);
            render_episode(
                &format!("P{page}"),
                &text(part, "part"),
                &duration(&part["duration"], false),
                width,
                lines,
            );
        }
        if let Some(season) = info.get("ugc_season").filter(|season| !season.is_null()) {
            lines.push(String::new());
            lines.push(fit(
                &format!(
                    "合集：{} · {} 条视频",
                    text(season, "title"),
                    number(season, "ep_count")
                ),
                width,
            ));
        }
    } else {
        let label = if kind == "cheese" { "课程" } else { "番剧" };
        let episodes = array(info, "episodes");
        lines.push(format!("{label} · {} 集", episodes.len()));
        let author = text(&info["up_info"], "uname");
        if !author.is_empty() {
            lines.push(fit(&format!("UP：{author}"), width));
        }
        lines.push(String::new());
        for episode in episodes {
            render_media_episode(episode, kind, width, lines);
        }
        for section in array(info, "section") {
            lines.push(String::new());
            lines.push(fit(&text(section, "title"), width));
            for episode in array(section, "episodes") {
                render_media_episode(episode, kind, width, lines);
            }
        }
    }
    if let Some(source) = source {
        lines.push(String::new());
        lines.push(format!("下载：bili download {}", shell_arg(&source)));
        if kind == "normal" && array(info, "pages").len() > 1 {
            lines.push(format!(
                "指定分 P：bili download {} --parts 1,2",
                shell_arg(&source)
            ));
        }
    }
}

fn render_media_episode(episode: &Value, kind: &str, width: usize, lines: &mut Vec<String>) {
    let title = text(episode, "title");
    let long_title = text(episode, "long_title");
    let title = if long_title.is_empty() {
        title
    } else {
        format!("{title} {long_title}")
    };
    render_episode(
        &format!("ep{}", number(episode, "id")),
        &title,
        &duration(&episode["duration"], kind == "bangumi"),
        width,
        lines,
    );
}

fn render_episode(id: &str, title: &str, duration: &str, width: usize, lines: &mut Vec<String>) {
    let prefix = format!("{id}  {duration}  ");
    if width >= 60 {
        lines.push(format!(
            "{prefix}{}",
            fit(title, width.saturating_sub(display_width(&prefix)))
        ));
    } else {
        lines.push(fit(title, width));
        lines.push(format!("  {id} · {duration}"));
    }
}

fn render_tasks(result: &Value, tasks: &[Value], width: usize, lines: &mut Vec<String>) {
    if result["live"].as_bool() == Some(false) {
        lines.push(format!("下载任务 · {} 项 · 磁盘快照", tasks.len()));
        lines.push("进度以最近一次保存为准。".into());
    } else if result["interrupted"].as_bool() == Some(true) {
        lines.push("下载已中断，进度已保存。".into());
    } else if result["completed"].as_bool() == Some(true) {
        lines.push(format!("下载完成 · {} 项任务", tasks.len()));
    } else {
        lines.push("部分任务未完成，进度已保存。".into());
    }
    if tasks.is_empty() {
        lines.push("暂无下载任务。".into());
        return;
    }
    for task in tasks {
        lines.push(String::new());
        render_task(task, width, result.get("live").is_none(), lines);
    }
    if result["live"].as_bool() == Some(false) {
        lines.push(String::new());
        lines.push(format!(
            "详情：bili tasks get {}",
            shell_arg(&text(&tasks[0], "task_id"))
        ));
    }
}

fn render_task(task: &Value, width: usize, detailed: bool, lines: &mut Vec<String>) {
    let state = text(task, "state");
    lines.push(fit(
        &format!("[{}] {}", state_label(&state), text(task, "title")),
        width,
    ));
    lines.push(format!("任务：{}", text(task, "task_id")));
    let (completed, total) = task_bytes(task);
    let progress = if total > 0 {
        let percent = completed.min(total) as f64 / total as f64 * 100.0;
        format!(
            "{percent:.1}% · {} / {}",
            format_bytes(completed),
            format_bytes(total)
        )
    } else if state == "Completed" {
        "100%".into()
    } else {
        "等待获取媒体大小".into()
    };
    lines.push(format!("媒体进度：{progress}"));
    if completed >= total && total > 0 && state != "Completed" {
        lines.push("分片已下载，任务尚未完成。".into());
    }
    if detailed {
        let bvid = text(task, "bvid");
        if !bvid.is_empty() {
            lines.push(format!("视频：{bvid}"));
        }
        let part = task["part"].as_i64().unwrap_or_default();
        let part_title = text(task, "part_title");
        if part > 0 && !part_title.is_empty() {
            lines.push(fit(&format!("分 P：{part} · {part_title}"), width));
        }
        let output = text(task, "output_dir");
        if !output.is_empty() {
            lines.push(format!("目录：{output}"));
        }
        let video = &task["video"];
        if video["selected"].as_bool() == Some(true) {
            lines.push(format!(
                "视频：{} · {}",
                quality_label(&text(video, "quality")),
                quality_label(&text(video, "codec"))
            ));
        }
        let audio = &task["audio"];
        if audio["selected"].as_bool() == Some(true) {
            lines.push(format!("音频：{}", quality_label(&text(audio, "quality"))));
        }
    }
    let error = text(task, "error");
    if !error.is_empty() {
        lines.push(wrap(&format!("原因：{error}"), width));
    }
    if state != "Completed" {
        lines.push(format!(
            "恢复：bili resume {}",
            shell_arg(&text(task, "task_id"))
        ));
    }
}

fn task_bytes(task: &Value) -> (u64, u64) {
    ["video", "audio"]
        .iter()
        .fold((0_u64, 0_u64), |(completed, total), key| {
            let stream = &task[key];
            if stream["selected"].as_bool() == Some(true) {
                (
                    completed.saturating_add(stream["bytes_completed"].as_u64().unwrap_or(0)),
                    total.saturating_add(stream["bytes_total"].as_u64().unwrap_or(0)),
                )
            } else {
                (completed, total)
            }
        })
}

fn state_label(state: &str) -> &str {
    match state {
        "Pending" => "等待中",
        "Downloading" => "下载中",
        "Paused" => "已暂停",
        "Completed" => "已完成",
        "Failed" => "失败",
        _ => "未知状态",
    }
}

fn quality_label(quality: &str) -> &str {
    match quality {
        "Unknown" | "" => "待解析",
        _ => quality,
    }
}

fn download_source(item: &Value) -> Option<String> {
    if item["downloadable"].as_bool() != Some(true) {
        return None;
    }
    ["bvid", "url"].iter().find_map(|key| {
        let value = text(item, key);
        (!value.is_empty()).then_some(value)
    })
}

fn media_source(target: &Value) -> Option<String> {
    let id = number(target, "id");
    match target["kind"].as_str()? {
        "normal" => {
            let normal_id = &target["id"];
            let id = match normal_id["kind"].as_str()? {
                "bvid" => text(normal_id, "id"),
                "aid" => format!("av{}", number(normal_id, "id")),
                _ => return None,
            };
            if let Some(page) = target["page"].as_u64() {
                Some(format!("https://www.bilibili.com/video/{id}?p={page}"))
            } else {
                Some(id)
            }
        }
        "bangumi_episode" => Some(format!("ep{id}")),
        "bangumi_season" => Some(format!("ss{id}")),
        "cheese_episode" => Some(format!("https://www.bilibili.com/cheese/play/ep{id}")),
        "cheese_season" => Some(format!("https://www.bilibili.com/cheese/play/ss{id}")),
        _ => None,
    }
}

fn duration(value: &Value, milliseconds: bool) -> String {
    let seconds = value.as_u64().or_else(|| value.as_str()?.parse().ok());
    let Some(seconds) = seconds else {
        return value
            .as_str()
            .filter(|value| !value.is_empty())
            .map_or_else(|| "—".into(), sanitize);
    };
    let seconds = if milliseconds {
        seconds / 1000
    } else {
        seconds
    };
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().map_or_else(String::new, sanitize)
}

fn number(value: &Value, key: &str) -> String {
    value[key]
        .as_i64()
        .map_or_else(|| "—".into(), |number| number.to_string())
}

fn fit(value: &str, width: usize) -> String {
    truncate(value, width)
}

fn pad(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(display_width(value)))
    )
}

fn shell_arg(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keyword_search_preserves_ids_and_quotes_the_next_page_command() {
        for width in [32, 80] {
            let rendered = render_result(
                &json!({
                    "search_type":"user","keyword":"Rust's 频道", "page":2,"page_size":20,"total":60,"has_more":true,
                    "items":[{"uid":1234567890123_i64,"name":"很长的中文账号名称需要按终端宽度显示", "fans":100,"video_count":12}]
                }),
                width,
            );
            assert!(rendered.contains("搜索UP"));
            assert!(rendered.contains("21"));
            assert!(rendered.contains("1234567890123"));
            assert!(rendered.contains("查看投稿：bili up videos 1234567890123"));
            assert!(rendered.contains("bili search 'Rust'\\''s 频道' --type user --page 3"));
            assert!(!rendered.contains("bili download"));
            let rendered = render_result(
                &json!({
                    "search_type":"video","keyword":"Rust", "page":1,"page_size":20,"total":1,"has_more":false,
                "items":[{"bvid":"BV17x411w7KC","title":"入门视频","duration":"1:30","author":"UP","uid":123,"downloadable":true}]
                }),
                width,
            );
            assert!(rendered.contains("搜索视频"));
            assert!(rendered.contains("下载：bili download BV17x411w7KC"));
            assert!(!rendered.contains("下一页"));
            let rendered = render_result(
                &json!({
                    "search_type":"video","keyword":"Rust","page":1,"total":2,"items":[
                        {"title":"无有效 ID","downloadable":false},
                        {"title":"仅有 AV 号","aid":170001,"bvid":null,"url":"https://www.bilibili.com/video/av170001","downloadable":true}
                    ]
                }),
                width,
            );
            assert!(rendered.contains("[不可下载]"));
            assert!(rendered.contains("av170001"));
            assert!(
                rendered.contains("下载：bili download https://www.bilibili.com/video/av170001")
            );
        }
        let rendered = render_result(
            &json!({"search_type":"user","keyword":"空","page":1,"total":0,"items":[]}),
            80,
        );
        assert!(rendered.contains("没有匹配结果"));
        assert!(!rendered.contains("bili up videos"));
    }

    #[test]
    fn narrow_discovery_keeps_complete_ids_and_suggests_the_correct_next_page() {
        let rendered = render_result(
            &json!({
                "target":{"kind":"up","uid":504018708}, "page":2,"page_size":42,"total":100,"has_more":true,
                "items":[{"title":"很长的中文视频标题需要按照终端宽度截断避免错行", "bvid":"BV17x411w7KC","author":"作者", "duration":"123", "downloadable":true}]
            }),
            32,
        );
        assert!(rendered.contains("43. "));
        assert!(rendered.contains("2:03 · BV17x411w7KC"));
        assert!(rendered.contains("下一页：bili up videos 504018708 --page 3"));
        assert!(rendered.contains("下载：bili download BV17x411w7KC"));
        assert!(
            rendered
                .lines()
                .find(|line| line.starts_with("43. "))
                .is_some_and(|line| display_width(line) <= 32)
        );
        assert!(!rendered.contains("downloadable"));
    }

    #[test]
    fn empty_and_unavailable_discovery_do_not_offer_a_download() {
        for items in [
            json!([]),
            json!([{"title":"已失效视频","bvid":"BV17x411w7KC","downloadable":false,"unavailable_reason":"收藏条目已失效"}]),
        ] {
            let rendered = render_result(
                &json!({"target":{"kind":"favorite","fid":123},"title":"我的收藏", "page":1,"page_size":36,"total":0,"has_more":false,"items":items}),
                80,
            );
            assert!(!rendered.contains("bili download"));
            assert!(rendered.contains("最后一页"));
            assert!(!rendered.contains("unavailable_reason"));
        }
    }

    #[test]
    fn task_detail_exposes_the_resume_id_and_directory_without_raw_machine_fields() {
        let rendered = render_result(
            &json!({"live":false,"task":{
                "task_id":"task-123-complete-copyable-id","title":"测试视频","state":"Paused","output_dir":"/tmp/视频目录", "error":"连接中断", "bvid":"BV17x411w7KC",
                "video":{"selected":true,"bytes_completed":1024,"bytes_total":4096,"quality":"1080P","codec":"AVC","url":"https://cdn.invalid?secret=value"},
                "audio":{"selected":false,"bytes_completed":100,"bytes_total":100}
            }}),
            80,
        );
        assert!(rendered.contains("磁盘快照"));
        assert!(rendered.contains("25.0%"));
        assert!(rendered.contains("目录：/tmp/视频目录"));
        assert!(rendered.contains("恢复：bili resume task-123-complete-copyable-id"));
        assert!(rendered.contains("原因：连接中断"));
        assert!(!rendered.contains("bytes_completed"));
        assert!(!rendered.contains("secret"));

        let finishing = render_result(
            &json!({"task":{
                "state":"Downloading","task_id":"finishing","title":"尚在处理",
                "video":{"selected":true,"bytes_completed":100,"bytes_total":100},
                "error":"错误信息很长也必须保留最后的修复提示：请运行 bili auth login"
            }}),
            24,
        );
        assert!(finishing.contains("[下载中]"));
        assert!(finishing.contains("任务尚未完成"));
        assert!(
            finishing
                .replace('\n', "")
                .contains("请运行 bili auth login")
        );
    }

    #[test]
    fn media_reads_the_real_tagged_metadata_shape_and_converts_episode_milliseconds() {
        let rendered = render_result(
            &json!({"media":{
                "target":{"kind":"bangumi_season","id":123},
                "info":{"kind":"bangumi","info":{"title":"测试番剧","episodes":[{"id":456,"title":"第1话","long_title":"开始","duration":90000}]}}
            }}),
            80,
        );
        assert!(rendered.contains("ep456  1:30  第1话 开始"));
        assert!(rendered.contains("下载：bili download ss123"));
        assert!(!rendered.contains("bangumi_season"));
    }
}
