//! 受限网络层：只允许 HTTPS、带大小上限与超时的 GET。
//!
//! 这个项目要下载的每一个字节最终都会落进游戏目录，所以网络层必须先把
//! 「明面上安全」的事做掉：
//!
//! 1. 只允许 HTTPS——明文 HTTP 和 file:// 之类的本地 scheme 一律拒绝；
//! 2. 响应大小有上限，超过直接报错，不给恶意服务器塞爆磁盘的机会；
//! 3. 记录最终 URL（重定向之后），因为「下载自哪里」是界面必须展示的事实。
//!
//! 错误永远携带上下文，**不允许用空内容代替失败**——下载半截当成功，
//! 是这类工具最阴险的一类 bug。

use std::io::Read;
use std::time::Duration;

use ureq::ResponseExt;

/// 单次 GET 的总时限。
const TIMEOUT: Duration = Duration::from_secs(30);

/// 一次成功 GET 的完整结果。
#[derive(Debug, Clone)]
pub struct HttpPayload {
    /// 响应体原文。
    pub bytes: Vec<u8>,
    /// 调用方请求的 URL。
    pub requested_url: String,
    /// 重定向之后实际取到内容的 URL。
    pub final_url: String,
}

/// 网络客户端的抽象。生产用 `UreqHttpClient`，测试用假实现。
pub trait HttpClient: Send + Sync {
    /// 下载 `url`，响应超过 `limit` 字节即失败。
    fn get(&self, url: &str, limit: usize) -> Result<HttpPayload, NetError>;
}

/// 基于 ureq 的生产实现。
#[derive(Debug, Clone)]
pub struct UreqHttpClient {
    agent: ureq::Agent,
}

impl Default for UreqHttpClient {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("只允许从 HTTPS 地址下载：{0}")]
    InsecureUrl(String),
    #[error("下载内容超过允许的 {limit} 字节")]
    TooLarge { limit: usize },
    #[error("网络请求失败：{0}")]
    Request(String),
}

/// 要求 `url` 是可解析的 HTTPS 地址。
pub(crate) fn validate_https(url: &str) -> Result<(), NetError> {
    let parsed = url::Url::parse(url).map_err(|_| NetError::InsecureUrl(url.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(NetError::InsecureUrl(url.to_string()));
    }
    Ok(())
}

/// 检查已收到的字节数是否超限。
pub(crate) fn check_size(received: usize, limit: usize) -> Result<(), NetError> {
    if received > limit {
        return Err(NetError::TooLarge { limit });
    }
    Ok(())
}

impl HttpClient for UreqHttpClient {
    fn get(&self, url: &str, limit: usize) -> Result<HttpPayload, NetError> {
        validate_https(url)?;

        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| NetError::Request(e.to_string()))?;

        let final_url = response.get_uri().to_string();

        // 多读一个字节，用来区分「恰好等于上限」和「真的超限」。
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| NetError::Request(e.to_string()))?;
        check_size(bytes.len(), limit)?;

        Ok(HttpPayload {
            bytes,
            requested_url: url.to_string(),
            final_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{NetError, check_size, validate_https};

    #[test]
    fn accepts_only_https_sources() {
        assert!(validate_https("https://raw.githubusercontent.com/a/b/main/x.json").is_ok());
        assert!(validate_https("http://example.com/x").is_err());
        assert!(validate_https("file:///C:/x").is_err());
        assert!(validate_https("not a url").is_err());
    }

    #[test]
    fn size_limit_is_enforced() {
        assert_eq!(check_size(4, 4), Ok(()));
        assert!(matches!(
            check_size(5, 4),
            Err(NetError::TooLarge { limit: 4 })
        ));
    }
}
