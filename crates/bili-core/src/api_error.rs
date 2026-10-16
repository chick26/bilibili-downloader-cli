use std::fmt;

use crate::bili_client::BiliResp;

/// A Bilibili API failure without response bodies or session credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BiliApiError {
    Http { endpoint: &'static str, status: u16 },
    Api { endpoint: &'static str, code: i64 },
    InvalidResponse { endpoint: &'static str },
}

impl BiliApiError {
    pub fn http(endpoint: &'static str, status: u16) -> Self {
        Self::Http { endpoint, status }
    }

    pub fn api(endpoint: &'static str, code: i64) -> Self {
        Self::Api { endpoint, code }
    }

    pub fn is_risk_control(&self) -> bool {
        matches!(
            self,
            Self::Http { status: 412, .. }
                | Self::Api {
                    code: -352 | -412,
                    ..
                }
        )
    }
}

impl fmt::Display for BiliApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { endpoint, status } => write!(f, "B站接口 {endpoint} 返回 HTTP {status}"),
            Self::Api { endpoint, code } => {
                write!(f, "B站接口 {endpoint} 返回业务错误码 {code}")?;
                match code {
                    -101 => write!(f, "（未登录或登录已过期，请重新登录）"),
                    -10403 => write!(f, "（当前地区无法访问该内容）"),
                    -403 => write!(f, "（当前账号没有观看权限）"),
                    _ => Ok(()),
                }
            }
            Self::InvalidResponse { endpoint } => write!(f, "B站接口 {endpoint} 返回了无效数据"),
        }
    }
}

impl std::error::Error for BiliApiError {}

pub(crate) async fn read_response(
    response: reqwest::Response,
    endpoint: &'static str,
) -> eyre::Result<BiliResp> {
    read_json(response, endpoint).await
}

pub(crate) fn check_http_status(
    status: reqwest::StatusCode,
    endpoint: &'static str,
) -> eyre::Result<()> {
    if status != reqwest::StatusCode::OK {
        // In particular, do not read/echo the HTML returned by security challenges.
        return Err(BiliApiError::http(endpoint, status.as_u16()).into());
    }
    Ok(())
}

pub(crate) async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    endpoint: &'static str,
) -> eyre::Result<T> {
    check_http_status(response.status(), endpoint)?;
    let body = response.bytes().await?;
    serde_json::from_slice(&body).map_err(|_| BiliApiError::InvalidResponse { endpoint }.into())
}

pub(crate) fn check_code(response: &BiliResp, endpoint: &'static str) -> eyre::Result<()> {
    if response.code != 0 {
        return Err(BiliApiError::api(endpoint, response.code).into());
    }
    Ok(())
}
