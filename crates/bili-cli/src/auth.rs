use std::{io::Read, path::Path, time::Duration};

use bili_core::{AppContext, types::user_info::UserInfo};
use qrcode::{Color, QrCode};
use serde_json::{Value, json};

use crate::output::{CliError, Outcome};

pub async fn status(context: &AppContext) -> Result<Outcome, CliError> {
    let cookie = context.get_config().read().sessdata.clone();
    if cookie.is_empty() {
        return Ok(Outcome::success(json!({"authenticated": false})));
    }
    let user = context
        .get_bili_client()
        .get_user_info(&cookie)
        .await
        .map_err(|err| CliError::report("AUTH_STATUS_FAILED", err))?;
    Ok(Outcome::success(user_summary(&user)))
}

pub async fn set_cookie(context: &AppContext, data_dir: &Path) -> Result<Outcome, CliError> {
    eprintln!("请从标准输入粘贴 SESSDATA 或 Cookie，按 Ctrl-D 结束。");
    let mut input = String::new();
    std::io::stdin()
        .take(65_537)
        .read_to_string(&mut input)
        .map_err(|_| CliError::new("COOKIE_READ_FAILED", "Could not read cookie from stdin"))?;
    if input.len() > 65_536 {
        return Err(CliError::new("INVALID_COOKIE", "Cookie input is too long"));
    }
    let cookie = parse_sessdata(&input)?;
    let user = context
        .get_bili_client()
        .get_user_info(&cookie)
        .await
        .map_err(|err| CliError::report("AUTH_VALIDATION_FAILED", err))?;
    if !user.is_login {
        return Err(CliError::new(
            "AUTH_VALIDATION_FAILED",
            "Cookie is not authenticated",
        ));
    }
    save_cookie(context, data_dir, cookie)?;
    Ok(Outcome::success(user_summary(&user)))
}

pub async fn login(
    context: &AppContext,
    data_dir: &Path,
    timeout_seconds: u64,
) -> Result<Outcome, CliError> {
    let client = context.get_bili_client();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
    let qr = tokio::time::timeout_at(deadline, client.generate_qrcode())
        .await
        .map_err(|_| CliError::new("AUTH_TIMEOUT", "QR login timed out"))?
        .map_err(|err| CliError::report("AUTH_QR_FAILED", err))?;
    eprintln!("请使用哔哩哔哩 App 扫码并确认登录：");
    render_qr(&qr.url)?;
    eprintln!("登录链接：{}", qr.url);

    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut last_code = None;
    loop {
        let poll = tokio::time::timeout_at(deadline, client.get_qrcode_status(&qr.qrcode_key));
        tokio::pin!(poll);
        let response = tokio::select! {
            result = &mut interrupt => {
                result.map_err(|err| CliError::new("SIGNAL_FAILED", err.to_string()))?;
                return Err(CliError::interrupted());
            }
            result = &mut poll => result
                .map_err(|_| CliError::new("AUTH_TIMEOUT", "QR login timed out"))?
                .map_err(|err| CliError::report("AUTH_POLL_FAILED", err))?,
        };
        match response.code {
            0 => {
                if response.sessdata.is_empty() {
                    return Err(CliError::new(
                        "AUTH_COOKIE_MISSING",
                        "Sign-in returned no session cookie",
                    ));
                }
                let user =
                    tokio::time::timeout_at(deadline, client.get_user_info(&response.sessdata))
                        .await
                        .map_err(|_| {
                            CliError::new("AUTH_TIMEOUT", "QR login validation timed out")
                        })?
                        .map_err(|err| CliError::report("AUTH_VALIDATION_FAILED", err))?;
                if !user.is_login {
                    return Err(CliError::new(
                        "AUTH_VALIDATION_FAILED",
                        "Sign-in could not be validated",
                    ));
                }
                save_cookie(context, data_dir, response.sessdata)?;
                return Ok(Outcome::success(user_summary(&user)));
            }
            86038 => {
                return Err(CliError::new(
                    "AUTH_QR_EXPIRED",
                    "QR code expired; run auth login again",
                ));
            }
            86101 | 86090 => {
                if Some(response.code) != last_code {
                    eprintln!(
                        "{}",
                        if response.code == 86101 {
                            "等待扫码…"
                        } else {
                            "已扫码，等待确认…"
                        }
                    );
                    last_code = Some(response.code);
                }
            }
            _ => {
                return Err(CliError::new(
                    "AUTH_POLL_FAILED",
                    "Unexpected QR login state",
                ));
            }
        }
        tokio::select! {
            result = &mut interrupt => {
                result.map_err(|err| CliError::new("SIGNAL_FAILED", err.to_string()))?;
                return Err(CliError::interrupted());
            }
            () = tokio::time::sleep_until(deadline) => return Err(CliError::new("AUTH_TIMEOUT", "QR login timed out")),
            () = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }
}

fn user_summary(user: &UserInfo) -> Value {
    json!({
        "authenticated": user.is_login,
        "user": { "id": user.mid, "name": user.uname, "vip_status": user.vip_status },
    })
}

fn save_cookie(context: &AppContext, data_dir: &Path, cookie: String) -> Result<(), CliError> {
    let mut config = context.get_config().read().clone();
    config.sessdata = cookie;
    config
        .save_to(data_dir)
        .map_err(|err| CliError::report("AUTH_SAVE_FAILED", err))?;
    *context.get_config().write() = config;
    context.get_bili_client().reload_client();
    Ok(())
}

fn parse_sessdata(input: &str) -> Result<String, CliError> {
    let input = input.trim();
    let input = input
        .strip_prefix("Cookie:")
        .or_else(|| input.strip_prefix("cookie:"))
        .unwrap_or(input)
        .trim();
    let value = if input.starts_with("SESSDATA=") || input.contains(';') {
        input
            .split(';')
            .filter_map(|entry| entry.trim().split_once('='))
            .find_map(|(key, value)| (key == "SESSDATA").then_some(value))
            .ok_or_else(|| CliError::new("INVALID_COOKIE", "Cookie header has no SESSDATA value"))?
    } else {
        input
    };
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control() || byte == b';')
        || !value.is_ascii()
    {
        return Err(CliError::new(
            "INVALID_COOKIE",
            "SESSDATA must be a nonempty single ASCII value",
        ));
    }
    Ok(value.to_owned())
}

fn render_qr(url: &str) -> Result<(), CliError> {
    let qr = QrCode::new(url.as_bytes()).map_err(|_| {
        CliError::new(
            "AUTH_QR_RENDER_FAILED",
            "Could not encode login URL as QR code",
        )
    })?;
    let width = qr.width();
    let colors = qr.to_colors();
    let margin = 4;
    let size = width + margin * 2;
    let is_dark = |x: usize, y: usize| {
        x >= margin
            && x < width + margin
            && y >= margin
            && y < width + margin
            && colors[(y - margin) * width + x - margin] == Color::Dark
    };
    // Two module rows per text row keep the QR code within ordinary terminal widths.
    for y in (0..size).step_by(2) {
        let mut line = String::with_capacity(size * 3);
        for x in 0..size {
            line.push(match (is_dark(x, y), is_dark(x, y + 1)) {
                (true, true) => ' ',
                (false, false) => '█',
                (false, true) => '▀',
                (true, false) => '▄',
            });
        }
        eprintln!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_raw_session_and_full_cookie_header() {
        assert_eq!(
            parse_sessdata("raw%2Csession\n").ok().as_deref(),
            Some("raw%2Csession")
        );
        assert_eq!(
            parse_sessdata("Cookie: buvid3=other; SESSDATA=session=with=equals; bili_jct=other")
                .ok()
                .as_deref(),
            Some("session=with=equals")
        );
    }

    #[test]
    fn cookie_validation_never_echoes_secret() {
        for input in ["", "other=secret; foo=bar", "secret\nheader", "SESSDATA="] {
            let error = match parse_sessdata(input) {
                Ok(_) => panic!("invalid cookie accepted"),
                Err(error) => error,
            };
            assert!(!error.message.contains("secret"));
        }
    }
}
