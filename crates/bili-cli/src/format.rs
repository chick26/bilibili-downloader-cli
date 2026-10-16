use std::io::IsTerminal;

use unicode_width::UnicodeWidthStr;

pub fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width > 0)
        .or_else(|| terminal_size::terminal_size().map(|(width, _)| usize::from(width.0)))
        .unwrap_or(80)
        .clamp(20, 200)
}

pub fn color_enabled(terminal: bool) -> bool {
    terminal
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}

pub fn stdout_color() -> bool {
    color_enabled(std::io::stdout().is_terminal())
}

pub fn stderr_color() -> bool {
    color_enabled(std::io::stderr().is_terminal())
}

/// Keep remote titles and errors from adding lines or terminal escape commands.
pub fn sanitize(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for ch in chars.by_ref() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    let mut escape = false;
                    for ch in chars.by_ref() {
                        if ch == '\u{7}' || (escape && ch == '\\') {
                            break;
                        }
                        escape = ch == '\u{1b}';
                    }
                }
                _ => {}
            }
        } else if ch.is_whitespace() {
            if !clean.ends_with(' ') {
                clean.push(' ');
            }
        } else if !ch.is_control() {
            clean.push(ch);
        }
    }
    clean.trim().to_owned()
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub fn truncate(text: &str, width: usize) -> String {
    let clean = sanitize(text);
    if display_width(&clean) <= width {
        return clean;
    }
    if width == 0 {
        return String::new();
    }
    let budget = width - 1;
    let mut end = 0;
    for (index, ch) in clean.char_indices() {
        let next = index + ch.len_utf8();
        if display_width(&clean[..next]) > budget {
            break;
        }
        end = next;
    }
    format!("{}…", clean[..end].trim_end())
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn wrap(text: &str, width: usize) -> String {
    let text = sanitize(text);
    let width = width.max(1);
    let mut lines = String::new();
    let mut line = String::new();
    for ch in text.chars() {
        let mut candidate = line.clone();
        candidate.push(ch);
        if !line.is_empty() && display_width(&candidate) > width {
            lines.push_str(line.trim_end());
            lines.push('\n');
            line.clear();
        }
        line.push(ch);
    }
    lines.push_str(line.trim_end());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_combining_text_and_truncation_fit_terminal_columns() {
        assert_eq!(display_width("中文 BV"), 7);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(truncate("中文视频标题", 7), "中文视…");
        assert_eq!(truncate("中文", 0), "");
        assert!(display_width(&truncate("👨‍👩‍👦 视频", 5)) <= 5);
        for line in wrap("这是一段很长的中文错误信息", 10).lines() {
            assert!(display_width(line) <= 10);
        }
    }

    #[test]
    fn remote_text_cannot_inject_terminal_lines_or_escape_sequences() {
        assert_eq!(
            sanitize("标题\n\t第二行\r\u{1b}[31m红色\u{1b}[0m"),
            "标题 第二行 红色"
        );
        assert_eq!(
            sanitize("\u{1b}]8;;https://example.com\u{7}文字\u{1b}]8;;\u{7}"),
            "文字"
        );
        assert_eq!(format_bytes(1_048_576), "1.0 MiB");
    }
}
