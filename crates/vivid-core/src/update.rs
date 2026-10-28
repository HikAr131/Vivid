//! 云端更新检查。
//!
//! # 只检查，不下载、不自我替换
//!
//! 这是刻意的取舍，不是偷懒：
//!
//! 1. **Windows 上程序无法覆盖正在运行的自己**，必须靠辅助进程或「改名再重启」
//!    的技巧。那套逻辑一旦出错，用户的 exe 就没了。
//! 2. **会自我改写的 exe 极易被杀毒误报。** 这个工具本来就在改游戏目录里的 DLL，
//!    再叠一个自我改写行为，报毒概率大幅上升——而红线 3 明确写着
//!    我们不能引导用户去关杀毒。
//! 3. 只拉一个几百字节的 JSON，失败就静默跳过，零风险；下载渠道还能随时换，
//!    不绑死在任何一家。
//!
//! # 为什么是公开 repo
//!
//! 清单托管在 GitHub 的**公开**仓库上。注意 **public repo ≠ 开源**：
//! 仓库里可以只放 README 和 Releases，源码一行不推。
//!
//! 私有 repo 走不通——它的 Release 资产必须带 token 才能下载，
//! 而把 token 打进客户端等于公开这个 token，任何人反编译一下就能读你的私有仓库。
//!
//! # 为什么读 raw 而不是 Releases API
//!
//! - API 对未认证请求有速率限制（60 次/小时/IP），raw 没有
//! - 返回体小两个数量级，解析简单
//! - 不必为了发个版本号额外上传一个 asset

use std::time::Duration;

use serde::Deserialize;

/// 更新清单的默认地址。
///
/// **换仓库时只改这里。** `main` 分支根目录下的 `update.json`。
pub const DEFAULT_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/HikAr131/Vivid/main/update.json";

/// 网络请求的总时限。
///
/// 给得很短是有意的：更新检查是**锦上添花**，不该让用户在启动后
/// 等一个可能根本连不上的服务器。连不上就当没有更新。
const TIMEOUT: Duration = Duration::from_secs(8);

/// 服务端那份 `update.json` 的结构。
///
/// 除了 `version` 之外全部可缺省——清单是人手写的，
/// 少一个字段不该让整个更新检查失效。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Manifest {
    /// 最新版本号，如 `0.2.0`。
    pub version: String,
    /// 更新说明，给用户看的一两句话。
    #[serde(default)]
    pub notes: String,
    /// 下载页地址。点「去下载」时用系统浏览器打开这个。
    #[serde(default)]
    pub download_url: String,
    /// 发布日期，纯展示用。
    #[serde(default)]
    pub published: String,
}

/// 检查到的可用更新。
#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub manifest: Manifest,
    /// 当前运行的版本，方便 UI 显示「从 x 到 y」。
    pub current: String,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("连不上更新服务器：{0}")]
    Network(String),

    #[error("更新信息读不懂（服务端格式变了？）：{0}")]
    Malformed(String),

    #[error("版本号 {0} 无法解析")]
    BadVersion(String),
}

/// 三段式版本号。
///
/// 派生的 `Ord` 逐段比较，正好是我们要的语义——
/// 这也意味着 `1.10.0` 大于 `1.9.0`，字符串比较可做不到这点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// 宽容地解析版本号：`1.2.3` / `v1.2.3` / `1.2` / `1` / `1.2.3-beta` 都认。
    ///
    /// 刻意不做严格 semver 校验：清单是人手写的，多打一个 `v`、
    /// 或者只写两段，都不该让更新检查整个失效。
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        if s.is_empty() {
            return None;
        }
        let mut parts = s.split(['.', '-', '+', ' ']);
        let major = parts.next()?.parse().ok()?;
        // 后两段缺了当 0：`1` 等价于 `1.0.0`
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 本程序自身的版本，取自 `Cargo.toml`。
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 判断远端是否比本地新。纯函数，便于测试。
pub fn is_newer(remote: &str, local: &str) -> Result<bool, UpdateError> {
    let r = SemVer::parse(remote).ok_or_else(|| UpdateError::BadVersion(remote.to_string()))?;
    let l = SemVer::parse(local).ok_or_else(|| UpdateError::BadVersion(local.to_string()))?;
    Ok(r > l)
}

/// 从清单文本判断有没有更新。把网络和判断分开，判断这部分才好测。
pub fn evaluate(manifest_json: &str, local: &str) -> Result<Option<Available>, UpdateError> {
    let manifest: Manifest =
        serde_json::from_str(manifest_json).map_err(|e| UpdateError::Malformed(e.to_string()))?;

    if is_newer(&manifest.version, local)? {
        Ok(Some(Available {
            manifest,
            current: local.to_string(),
        }))
    } else {
        Ok(None)
    }
}

/// 拉清单并判断有没有更新。**阻塞**，调用方负责放到后台线程里。
pub fn check(url: &str, local: &str) -> Result<Option<Available>, UpdateError> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let body = agent
        .get(url)
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?
        .body_mut()
        .read_to_string()
        .map_err(|e| UpdateError::Network(e.to_string()))?;

    evaluate(&body, local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_version_shapes() {
        let cases = [
            ("1.2.3", SemVer::new(1, 2, 3)),
            ("v1.2.3", SemVer::new(1, 2, 3)),
            ("V0.1.0", SemVer::new(0, 1, 0)),
            ("1.2", SemVer::new(1, 2, 0)),
            ("2", SemVer::new(2, 0, 0)),
            ("1.2.3-beta", SemVer::new(1, 2, 3)),
            ("  0.9.1  ", SemVer::new(0, 9, 1)),
        ];
        for (input, expected) in cases {
            assert_eq!(SemVer::parse(input), Some(expected), "{input}");
        }
    }

    #[test]
    fn rejects_garbage_versions() {
        for bad in ["", "   ", "v", "abc", "x.y.z"] {
            assert_eq!(SemVer::parse(bad), None, "{bad:?} 不该解析出版本");
        }
    }

    /// 字符串比较会说 "1.9" > "1.10"，这条守着别退化成那样。
    #[test]
    fn compares_numerically_not_as_strings() {
        assert!(SemVer::parse("1.10.0") > SemVer::parse("1.9.0"));
        assert!(SemVer::parse("0.2.0") > SemVer::parse("0.1.99"));
        assert!(SemVer::parse("1.0.0") > SemVer::parse("0.99.99"));
    }

    #[test]
    fn only_strictly_newer_counts_as_an_update() {
        assert!(is_newer("0.2.0", "0.1.0").unwrap());
        assert!(!is_newer("0.1.0", "0.1.0").unwrap(), "同版本不是更新");
        assert!(!is_newer("0.1.0", "0.2.0").unwrap(), "降级不是更新");
    }

    #[test]
    fn evaluates_a_realistic_manifest() {
        let json = r#"{
            "version": "0.3.0",
            "notes": "修复了扫描大库时的卡顿",
            "download_url": "https://github.com/owner/repo/releases/latest",
            "published": "2026-09-15"
        }"#;

        let got = evaluate(json, "0.1.0").unwrap().expect("应当有更新");
        assert_eq!(got.manifest.version, "0.3.0");
        assert_eq!(got.current, "0.1.0");
        assert!(got.manifest.notes.contains("卡顿"));

        assert!(
            evaluate(json, "0.3.0").unwrap().is_none(),
            "已经是最新就不该报更新"
        );
        assert!(
            evaluate(json, "1.0.0").unwrap().is_none(),
            "本地更新就不该报更新"
        );
    }

    /// 清单是人手写的，少字段不该让更新检查整个失效。
    #[test]
    fn tolerates_a_minimal_manifest() {
        let got = evaluate(r#"{"version":"9.9.9"}"#, "0.1.0")
            .unwrap()
            .expect("只有版本号也该能判断");
        assert_eq!(got.manifest.version, "9.9.9");
        assert!(got.manifest.notes.is_empty());
        assert!(got.manifest.download_url.is_empty());
    }

    #[test]
    fn reports_readable_errors_for_broken_input() {
        let err = evaluate("not json at all", "0.1.0").expect_err("坏 JSON 应当报错");
        assert!(matches!(err, UpdateError::Malformed(_)), "{err}");
        assert!(err.to_string().contains("读不懂"), "错误要说人话：{err}");

        let err = evaluate(r#"{"version":"???"}"#, "0.1.0").expect_err("坏版本号应当报错");
        assert!(matches!(err, UpdateError::BadVersion(_)), "{err}");
    }

    /// 自己的版本号必须能被自己解析——否则更新检查在发布当天就是坏的。
    #[test]
    fn our_own_version_is_parsable() {
        assert!(
            SemVer::parse(current_version()).is_some(),
            "Cargo.toml 里的版本号 {} 解析不了",
            current_version()
        );
    }

    /// 默认地址必须是 https 且指向 raw，不是网页版 GitHub。
    /// 指错了会拉回一整页 HTML，解析必然失败。
    #[test]
    fn default_url_points_at_raw_over_https() {
        assert!(DEFAULT_MANIFEST_URL.starts_with("https://"));
        assert!(
            DEFAULT_MANIFEST_URL.contains("raw.githubusercontent.com"),
            "要用 raw 域名，网页版会返回 HTML"
        );
        assert!(DEFAULT_MANIFEST_URL.ends_with(".json"));
    }
}
