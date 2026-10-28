//! ReShade 与 RenoDX 上游发布解析 + last-known-good。
//!
//! 行为依据（RHI 项目 `https://github.com/RankFTW/RHI`）：
//! RHI 的 ReShade 取 reshade.me 的 addon 稳定版 setup，RenoDX DLSS5 addon 取
//! `rhi-repo` 最新 `renodx-dlss5-` 标签。Vivid 用同样的入口和同样的筛选规则，
//! 解析结果缓存到 `upstream-snapshot.json`，刷新失败时回退 last-known-good。
//!
//! 版本排序复用 `update::SemVer`：`1.10.0` 必须大于 `1.9.0`，
//! 字符串比较做不到这一点。

use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::net::HttpClient;
use crate::update::SemVer;

/// RenoDX DLSS5 addon 的 GitHub 发布列表入口。
pub const RENODX_RELEASES_URL: &str =
    "https://api.github.com/repos/RankFTW/rhi-repo/releases?per_page=100";

/// ReShade 官网首页，addon 版 setup 的下载链接从页面里解析。
pub const RESHADE_HOME_URL: &str = "https://reshade.me/";

/// 发布列表可能到几 MB，给 8 MiB 上限。
const RELEASES_LIMIT: usize = 8 * 1024 * 1024;

/// 官网首页是普通 HTML，2 MiB 足够。
const RESHADE_LIMIT: usize = 2 * 1024 * 1024;

/// RenoDX 组件在游戏目录里的固定文件名。
const RENODX_ADDON_NAME: &str = "renodx-dlss5.addon64";

/// RenoDX 发布标签的前缀。
const RENODX_TAG_PREFIX: &str = "renodx-dlss5-";

/// 资产名的公共前缀。**刻意不带分隔符**：上游的 zip 资产用的是下划线
/// （`renodx-dlss5_4.60.zip`），老的直给资产用的是点（`renodx-dlss5.addon64`）。
/// 早先这里写成带连字符的标签前缀，真实发布一个都匹配不上。
const RENODX_ASSET_STEM: &str = "renodx-dlss5";

/// 一个已解析的上游发布：版本、下载地址、最终文件名与容器类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRelease {
    pub version: String,
    pub url: String,
    /// 组件落盘后的文件名（与下载资产名可能不同：ZIP 解包后取这个名）。
    pub file_name: String,
    pub container: ContainerKind,
}

/// 下载物的容器类型，决定 `artifact` 怎么取出需要的文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerKind {
    /// 下载物本身就是目标文件。
    Direct,
    /// 普通 ZIP，按 basename 取一个条目。
    Zip,
    /// ReShade setup：PE 自解压壳 + 尾部 ZIP 数据。
    SelfExtractingZip,
}

/// 两个上游发布的 last-known-good 快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamSnapshot {
    pub fetched_at: String,
    pub reshade: ResolvedRelease,
    pub renodx_dlss5: ResolvedRelease,
}

/// 刷新结果：快照本身 + （兜底时）在线失败的原因。
#[derive(Debug, Clone)]
pub struct LoadedUpstreamSnapshot {
    pub snapshot: UpstreamSnapshot,
    /// 在线刷新失败、回退缓存时给用户看的失败原因；成功时为 `None`。
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("上游发布信息读不懂（页面或 API 变了？）：{0}")]
    Malformed(String),

    #[error("上游发布里的下载地址不是 HTTPS：{0}")]
    InsecureUrl(String),

    #[error("上游发布刷新失败，且本地没有可用的缓存：{0}")]
    NoSnapshot(String),
}

/// GitHub Releases API 响应里用到的字段。
#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// 从 GitHub Releases JSON 里选出数字版本最新的 RenoDX DLSS5 发布。
///
/// 只接受直接 `renodx-dlss5.addon64` 或名称以 `renodx-dlss5` 开头的 ZIP 资产；
/// 版本号必须能被 `SemVer` 解析，标签与版本都无法解析的发布一律跳过。
pub fn resolve_renodx_release(releases_json: &str) -> Result<ResolvedRelease, UpstreamError> {
    let releases: Vec<GhRelease> = serde_json::from_str(releases_json)
        .map_err(|e| UpstreamError::Malformed(format!("GitHub 发布列表：{e}")))?;

    let mut best: Option<(SemVer, ResolvedRelease)> = None;
    for release in releases {
        let Some(version) = release.tag_name.strip_prefix(RENODX_TAG_PREFIX) else {
            continue;
        };
        let Some(semver) = SemVer::parse(version) else {
            continue;
        };

        // 一个发布里挑一个资产：优先直给的 addon64，否则第一个 renodx-dlss5 开头的 zip。
        // **不看上游列举顺序**，同一份发布每次都解析出同一个结果。
        let mut chosen: Option<(ContainerKind, &GhAsset)> = None;
        for asset in &release.assets {
            let container = if asset.name == RENODX_ADDON_NAME {
                ContainerKind::Direct
            } else if asset.name.starts_with(RENODX_ASSET_STEM) && asset.name.ends_with(".zip") {
                ContainerKind::Zip
            } else {
                continue;
            };
            if crate::net::validate_https(&asset.browser_download_url).is_err() {
                return Err(UpstreamError::InsecureUrl(
                    asset.browser_download_url.clone(),
                ));
            }
            if container == ContainerKind::Direct || chosen.is_none() {
                chosen = Some((container, asset));
            }
        }

        let Some((container, asset)) = chosen else {
            continue;
        };
        let resolved = ResolvedRelease {
            version: version.to_string(),
            url: asset.browser_download_url.clone(),
            file_name: RENODX_ADDON_NAME.to_string(),
            container,
        };
        if best.as_ref().is_none_or(|(best_ver, _)| semver > *best_ver) {
            best = Some((semver, resolved));
        }
    }

    best.map(|(_, release)| release)
        .ok_or_else(|| UpstreamError::Malformed("没有可用的 renodx-dlss5 发布".to_string()))
}

/// 从 ReShade 首页 HTML 解析 addon 版稳定 setup 的版本与下载地址。
pub fn resolve_reshade(html: &str) -> Result<ResolvedRelease, UpstreamError> {
    // 与 RHI 相同的匹配口径：只认 addon 版 setup，链接路径以 /downloads/ 开头。
    let pattern = Regex::new(r#"href="(/downloads/ReShade_Setup_([\d.]+)_Addon\.exe)""#)
        .expect("内置正则必须合法");
    let captures = pattern.captures(html).ok_or_else(|| {
        UpstreamError::Malformed("页面上找不到 ReShade addon 版下载链接".to_string())
    })?;
    let path = &captures[1];
    let version = captures[2].to_string();
    let url = format!("https://reshade.me{path}");
    Ok(ResolvedRelease {
        file_name: format!("ReShade_Setup_{version}_Addon.exe"),
        version,
        url,
        container: ContainerKind::SelfExtractingZip,
    })
}

/// 刷新两个上游发布：都成功才替换缓存；任何失败回退 last-known-good。
pub fn refresh_or_cached(
    http: &dyn HttpClient,
    cache_path: &Path,
) -> Result<LoadedUpstreamSnapshot, UpstreamError> {
    let fetched_at = chrono::Utc::now().to_rfc3339();

    let online = (|| -> Result<UpstreamSnapshot, String> {
        let releases = http
            .get(RENODX_RELEASES_URL, RELEASES_LIMIT)
            .map_err(|e| e.to_string())?;
        let reshade_page = http
            .get(RESHADE_HOME_URL, RESHADE_LIMIT)
            .map_err(|e| e.to_string())?;
        let renodx_dlss5 = resolve_renodx_release(&String::from_utf8_lossy(&releases.bytes))
            .map_err(|e| e.to_string())?;
        let reshade = resolve_reshade(&String::from_utf8_lossy(&reshade_page.bytes))
            .map_err(|e| e.to_string())?;
        Ok(UpstreamSnapshot {
            fetched_at,
            reshade,
            renodx_dlss5,
        })
    })();

    match online {
        Ok(snapshot) => {
            // 缓存写失败不影响本次在线结果。
            let _ = save_upstream(cache_path, &snapshot);
            Ok(LoadedUpstreamSnapshot {
                snapshot,
                error: None,
            })
        }
        Err(reason) => match load_cached_upstream(cache_path) {
            Some(snapshot) => Ok(LoadedUpstreamSnapshot {
                snapshot,
                error: Some(reason),
            }),
            None => Err(UpstreamError::NoSnapshot(reason)),
        },
    }
}

/// 序列化写入上游快照缓存（同目录临时文件 + rename）。
pub fn save_upstream(cache_path: &Path, snapshot: &UpstreamSnapshot) -> Result<(), UpstreamError> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| UpstreamError::Malformed(format!("建缓存目录失败：{e}")))?;
    }
    let json = serde_json::to_vec_pretty(snapshot)
        .map_err(|e| UpstreamError::Malformed(format!("序列化快照失败：{e}")))?;
    let tmp = cache_path.with_extension("json.tmp");
    std::fs::write(&tmp, json)
        .map_err(|e| UpstreamError::Malformed(format!("写临时缓存失败：{e}")))?;
    std::fs::rename(&tmp, cache_path)
        .map_err(|e| UpstreamError::Malformed(format!("替换缓存失败：{e}")))?;
    Ok(())
}

/// 读取本地缓存；损坏或 URL 不再安全时返回 `None`。
pub fn load_cached_upstream(cache_path: &Path) -> Option<UpstreamSnapshot> {
    let bytes = std::fs::read(cache_path).ok()?;
    let snapshot: UpstreamSnapshot = serde_json::from_slice(&bytes).ok()?;
    for url in [&snapshot.reshade.url, &snapshot.renodx_dlss5.url] {
        if crate::net::validate_https(url).is_err() {
            return None;
        }
    }
    Some(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASES_JSON: &str = include_str!("../tests/fixtures/rhi-releases-min.json");
    const RESHADE_HTML: &str = include_str!("../tests/fixtures/reshade-download-min.html");

    /// 样本就是 2026-08-31 从上游抓下来的真实形状（两段版本 + 下划线 zip）。
    #[test]
    fn selects_numeric_latest_renodx_release() {
        let got = resolve_renodx_release(RELEASES_JSON).unwrap();
        assert_eq!(got.version, "4.60");
        assert_eq!(got.file_name, "renodx-dlss5.addon64");
        assert_eq!(got.container, ContainerKind::Zip);
    }

    #[test]
    fn resolves_only_the_addon_reshade_build() {
        let got = resolve_reshade(RESHADE_HTML).unwrap();
        assert_eq!(got.version, "6.8.1");
        assert_eq!(
            got.url,
            "https://reshade.me/downloads/ReShade_Setup_6.8.1_Addon.exe"
        );
    }

    /// 真实上游（2026-08-31 实测）用的是**下划线**资产名和两段版本号：
    /// 标签 `renodx-dlss5-4.60`，资产 `renodx-dlss5_4.60.zip`。
    /// 早先只认 `renodx-dlss5-` 前缀的资产名，结果一个发布都匹配不上，
    /// 安装在真机上直接卡在「清单不可用」——这条测试就是钉死那个回归。
    #[test]
    fn accepts_the_real_underscore_asset_naming() {
        let json = r#"[
          {"tag_name":"renodx-dlss5-4.5","assets":[{"name":"renodx-dlss5_4.5.zip",
            "browser_download_url":"https://github.com/RankFTW/rhi-repo/releases/download/renodx-dlss5-4.5/renodx-dlss5_4.5.zip"}]},
          {"tag_name":"renodx-dlss5-4.60","assets":[{"name":"renodx-dlss5_4.60.zip",
            "browser_download_url":"https://github.com/RankFTW/rhi-repo/releases/download/renodx-dlss5-4.60/renodx-dlss5_4.60.zip"}]},
          {"tag_name":"renodx-dlss5-4.55","assets":[{"name":"renodx-dlss5_4.55.zip",
            "browser_download_url":"https://github.com/RankFTW/rhi-repo/releases/download/renodx-dlss5-4.55/renodx-dlss5_4.55.zip"}]}
        ]"#;
        let got = resolve_renodx_release(json).unwrap();
        assert_eq!(got.version, "4.60", "4.60 > 4.55 > 4.5");
        assert_eq!(got.container, ContainerKind::Zip);
        assert_eq!(got.file_name, "renodx-dlss5.addon64");
        assert!(got.url.ends_with("renodx-dlss5_4.60.zip"), "{}", got.url);
    }

    /// 同一个发布里既有直给的 addon64 又有 zip 时，固定取直给的那个。
    #[test]
    fn a_direct_addon_wins_over_a_zip_in_the_same_release() {
        let json = r#"[{"tag_name":"renodx-dlss5-4.60","assets":[
          {"name":"renodx-dlss5_4.60.zip",
           "browser_download_url":"https://example.com/renodx-dlss5_4.60.zip"},
          {"name":"renodx-dlss5.addon64",
           "browser_download_url":"https://example.com/renodx-dlss5.addon64"}]}]"#;
        let got = resolve_renodx_release(json).unwrap();
        assert_eq!(got.container, ContainerKind::Direct);
    }

    #[test]
    fn releases_without_renodx_tags_are_rejected() {
        let json = r#"[{"tag_name": "something-else-1.0.0", "assets": []}]"#;
        assert!(matches!(
            resolve_renodx_release(json),
            Err(UpstreamError::Malformed(_))
        ));
    }

    #[test]
    fn reshade_page_without_addon_link_is_rejected() {
        let html = r#"<a href="/downloads/ReShade_Setup_6.8.1.exe">Download ReShade</a>"#;
        assert!(matches!(
            resolve_reshade(html),
            Err(UpstreamError::Malformed(_))
        ));
    }

    #[test]
    fn renodx_asset_with_insecure_url_is_rejected() {
        let json = r#"[{
            "tag_name": "renodx-dlss5-1.9.0",
            "assets": [{"name": "renodx-dlss5.addon64",
                        "browser_download_url": "http://example.com/renodx-dlss5.addon64"}]
        }]"#;
        assert!(matches!(
            resolve_renodx_release(json),
            Err(UpstreamError::InsecureUrl(_))
        ));
    }

    #[test]
    fn broken_remote_keeps_last_known_good_upstream() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("upstream.json");
        let good = UpstreamSnapshot {
            fetched_at: "2026-08-31T16:00:00Z".to_string(),
            reshade: resolve_reshade(RESHADE_HTML).unwrap(),
            renodx_dlss5: resolve_renodx_release(RELEASES_JSON).unwrap(),
        };
        save_upstream(&cache, &good).unwrap();
        let before = std::fs::read(&cache).unwrap();

        let client = crate::testing::FakeHttp::with_texts([
            (RENODX_RELEASES_URL, "{broken"),
            (RESHADE_HOME_URL, RESHADE_HTML),
        ]);
        let loaded = refresh_or_cached(&client, &cache).unwrap();
        assert!(loaded.error.is_some());
        assert_eq!(std::fs::read(&cache).unwrap(), before);
    }
}
