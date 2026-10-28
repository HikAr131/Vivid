//! RHI 在线清单的解析、验证与 last-known-good 缓存。
//!
//! 行为依据（RHI 项目 `https://github.com/RankFTW/RHI`）：
//! RHI 用两份清单描述「哪些游戏装什么」与「DLSS 组件有哪些版本」。
//! Vivid 只读其中两款游戏需要的部分，其余字段原样保留、不做二次解释。
//!
//! # 信任边界
//!
//! 清单没有签名哈希，信任根就是 RHI 仓库本身。因此这里做两件事：
//!
//! 1. **验证**：Vivid 需要的游戏映射、RDR2 的 Vulkan 标记、非空版本列表、
//!    HTTPS 下载地址——缺一不可，缺了就拒绝整份清单；
//! 2. **回退**：只有两份远端响应都下载、解析、验证成功后，才用同目录临时文件
//!    替换本地缓存；任何失败都继续用 last-known-good，绝不拿半成品覆盖好缓存。
//!
//! 版本标签（如 `310.8.SF-v2`）**原样保存**，不强行转成数字版本——
//! 上游的命名含语义（SF = ShortFuse 系列），转数字会丢信息。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::net::HttpClient;

/// RHI 游戏/图形 API 清单的固定入口。
pub const GAME_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/RankFTW/RHI/main/manifest.json";

/// RHI DLSS 组件清单的固定入口。
pub const DLSS_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/RankFTW/RHI/main/dlss_manifest.json";

/// 清单下载上限。两份清单都是几 KB 的 JSON，4 MiB 已经非常宽。
const MANIFEST_LIMIT: usize = 4 * 1024 * 1024;

/// Vivid 首版支持的两款游戏在清单里的精确名称。
const GTA5_NAME: &str = "Grand Theft Auto V Enhanced";
const RDR2_NAME: &str = "Red Dead Redemption 2";

/// RDR2 配方要求的图形 API 标记，与 RHI 的 `graphicsApiOverrides` 一致。
const RDR2_API_VULKAN: &str = "VLK";

/// 两份清单合并后的快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RhiSnapshot {
    /// 快照生成时间（RFC 3339）。
    pub fetched_at: String,
    pub games: GameManifest,
    pub dlss: DlssManifest,
}

/// `manifest.json` 里 Vivid 关心的字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameManifest {
    pub version: u32,
    #[serde(rename = "wikiNameOverrides")]
    pub wiki_name_overrides: BTreeMap<String, String>,
    #[serde(rename = "graphicsApiOverrides")]
    pub graphics_api_overrides: BTreeMap<String, String>,
}

/// `dlss_manifest.json` 的结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlssManifest {
    pub dlss: Vec<ManifestEntry>,
    pub dlssnr: Vec<ManifestEntry>,
}

/// 一个可下载组件：版本标签 + 下载地址。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub version: String,
    pub url: String,
}

/// 快照来自哪：本次在线刷新成功，还是本地缓存兜底。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOrigin {
    Online,
    Cache,
}

/// 刷新结果：快照本身 + 来源 + （兜底时）刷新失败的原因。
#[derive(Debug, Clone)]
pub struct LoadedSnapshot {
    pub snapshot: RhiSnapshot,
    pub origin: SnapshotOrigin,
    /// 在线刷新失败、回退缓存时，给用户看的失败原因；在线成功时为 `None`。
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("RHI 清单读不懂（上游格式变了？）：{0}")]
    Malformed(String),

    #[error("RHI 清单缺少 Vivid 需要的游戏映射：{0}")]
    MissingMapping(String),

    #[error("RHI 清单里的下载地址不是 HTTPS：{0}")]
    InsecureEntry(String),

    #[error("清单刷新失败，且本地没有可用的缓存：{0}")]
    NoSnapshot(String),
}

impl RhiSnapshot {
    /// 解析并验证两份清单的原始 JSON。
    pub fn parse(
        game_json: &str,
        dlss_json: &str,
        fetched_at: &str,
    ) -> Result<Self, ManifestError> {
        let games: GameManifest = serde_json::from_str(game_json)
            .map_err(|e| ManifestError::Malformed(format!("游戏清单：{e}")))?;
        let dlss: DlssManifest = serde_json::from_str(dlss_json)
            .map_err(|e| ManifestError::Malformed(format!("DLSS 清单：{e}")))?;
        Self::validate(games, dlss, fetched_at)
    }

    /// 校验清单是否包含 Vivid 需要的全部事实。
    fn validate(
        games: GameManifest,
        dlss: DlssManifest,
        fetched_at: &str,
    ) -> Result<Self, ManifestError> {
        if !games.wiki_name_overrides.contains_key(GTA5_NAME) {
            return Err(ManifestError::MissingMapping(GTA5_NAME.to_string()));
        }
        if !games.wiki_name_overrides.contains_key(RDR2_NAME) {
            return Err(ManifestError::MissingMapping(RDR2_NAME.to_string()));
        }
        if games
            .graphics_api_overrides
            .get(RDR2_NAME)
            .map(String::as_str)
            != Some(RDR2_API_VULKAN)
        {
            return Err(ManifestError::MissingMapping(format!(
                "{RDR2_NAME} 的图形 API 覆盖必须是 {RDR2_API_VULKAN}"
            )));
        }
        if dlss.dlss.is_empty() {
            return Err(ManifestError::Malformed("dlss 版本列表为空".to_string()));
        }
        if dlss.dlssnr.is_empty() {
            return Err(ManifestError::Malformed("dlssnr 版本列表为空".to_string()));
        }
        for entry in dlss.dlss.iter().chain(&dlss.dlssnr) {
            crate::net::validate_https(&entry.url)
                .map_err(|_| ManifestError::InsecureEntry(entry.url.clone()))?;
        }
        Ok(Self {
            fetched_at: fetched_at.to_string(),
            games,
            dlss,
        })
    }
}

/// 把快照序列化写入 `cache_path`（同目录临时文件 + rename，避免写一半）。
pub fn save_snapshot(cache_path: &Path, snapshot: &RhiSnapshot) -> Result<(), ManifestError> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ManifestError::Malformed(format!("建缓存目录失败：{e}")))?;
    }
    let json = serde_json::to_vec_pretty(snapshot)
        .map_err(|e| ManifestError::Malformed(format!("序列化快照失败：{e}")))?;
    let tmp = cache_path.with_extension("json.tmp");
    std::fs::write(&tmp, json)
        .map_err(|e| ManifestError::Malformed(format!("写临时缓存失败：{e}")))?;
    std::fs::rename(&tmp, cache_path)
        .map_err(|e| ManifestError::Malformed(format!("替换缓存失败：{e}")))?;
    Ok(())
}

/// 刷新清单：在线成功则替换缓存；失败则回退 last-known-good。
///
/// 两份远端响应**都**成功解析验证后才写缓存。远端与缓存都不可用时返回错误。
pub fn refresh_or_cached(
    http: &dyn HttpClient,
    cache_path: &Path,
) -> Result<LoadedSnapshot, ManifestError> {
    let fetched_at = chrono::Utc::now().to_rfc3339();

    let online = (|| -> Result<RhiSnapshot, String> {
        let game = http
            .get(GAME_MANIFEST_URL, MANIFEST_LIMIT)
            .map_err(|e| e.to_string())?;
        let dlss = http
            .get(DLSS_MANIFEST_URL, MANIFEST_LIMIT)
            .map_err(|e| e.to_string())?;
        RhiSnapshot::parse(
            &String::from_utf8_lossy(&game.bytes),
            &String::from_utf8_lossy(&dlss.bytes),
            &fetched_at,
        )
        .map_err(|e| e.to_string())
    })();

    match online {
        Ok(snapshot) => {
            // 缓存写失败不影响在线结果：这次会话仍用在线快照，旧缓存留作下次兜底。
            let _ = save_snapshot(cache_path, &snapshot);
            Ok(LoadedSnapshot {
                snapshot,
                origin: SnapshotOrigin::Online,
                error: None,
            })
        }
        Err(reason) => match load_cached(cache_path) {
            Some(snapshot) => Ok(LoadedSnapshot {
                snapshot,
                origin: SnapshotOrigin::Cache,
                error: Some(reason),
            }),
            None => Err(ManifestError::NoSnapshot(reason)),
        },
    }
}

/// 读取并重新验证本地缓存；损坏或缺失返回 `None`。
///
/// 公开是给只读体检（`manager::inspect`）用的：它不联网，只看 last-known-good。
pub fn load_cached(cache_path: &Path) -> Option<RhiSnapshot> {
    let bytes = std::fs::read(cache_path).ok()?;
    let snapshot: RhiSnapshot = serde_json::from_slice(&bytes).ok()?;
    RhiSnapshot::validate(snapshot.games, snapshot.dlss, &snapshot.fetched_at).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHttp;

    const GAME_JSON: &str = include_str!("../tests/fixtures/rhi-manifest-min.json");
    const DLSS_JSON: &str = include_str!("../tests/fixtures/rhi-dlss-manifest-min.json");

    fn valid_snapshot() -> RhiSnapshot {
        RhiSnapshot::parse(GAME_JSON, DLSS_JSON, "2026-08-31T16:00:00Z").unwrap()
    }

    #[test]
    fn parses_required_games_and_preserves_non_numeric_version_labels() {
        let snapshot = RhiSnapshot::parse(GAME_JSON, DLSS_JSON, "2026-08-31T16:00:00Z").unwrap();
        assert_eq!(snapshot.dlss.dlssnr[0].version, "310.8.SF-v2");
        assert_eq!(
            snapshot.games.wiki_name_overrides["Grand Theft Auto V Enhanced"],
            "Grand Theft Auto V : Enhanced"
        );
        assert_eq!(
            snapshot.games.graphics_api_overrides["Red Dead Redemption 2"],
            "VLK"
        );
    }

    #[test]
    fn broken_remote_does_not_replace_last_known_good() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("snapshot.json");
        save_snapshot(&cache, &valid_snapshot()).unwrap();
        let before = std::fs::read(&cache).unwrap();
        // 远端游戏清单是坏 JSON：必须回退缓存，且缓存内容一字节都不能变。
        let client = FakeHttp::with_texts([
            (GAME_MANIFEST_URL, "{broken"),
            (DLSS_MANIFEST_URL, DLSS_JSON),
        ]);
        let loaded = refresh_or_cached(&client, &cache).unwrap();
        assert_eq!(loaded.origin, SnapshotOrigin::Cache);
        assert!(loaded.error.is_some());
        assert_eq!(std::fs::read(&cache).unwrap(), before);
    }

    #[test]
    fn missing_gta_mapping_is_rejected() {
        let game = r#"{
            "version": 2,
            "wikiNameOverrides": {"Red Dead Redemption 2": "Red Dead Redemption 2 (Vulkan)"},
            "graphicsApiOverrides": {"Red Dead Redemption 2": "VLK"}
        }"#;
        assert!(matches!(
            RhiSnapshot::parse(game, DLSS_JSON, "t"),
            Err(ManifestError::MissingMapping(_))
        ));
    }

    #[test]
    fn missing_rdr2_vulkan_override_is_rejected() {
        let game = r#"{
            "version": 2,
            "wikiNameOverrides": {
                "Grand Theft Auto V Enhanced": "Grand Theft Auto V : Enhanced",
                "Red Dead Redemption 2": "Red Dead Redemption 2 (Vulkan)"
            },
            "graphicsApiOverrides": {}
        }"#;
        assert!(matches!(
            RhiSnapshot::parse(game, DLSS_JSON, "t"),
            Err(ManifestError::MissingMapping(_))
        ));
    }

    #[test]
    fn empty_version_lists_are_rejected() {
        let empty_dlss = r#"{"dlss": [], "dlssnr": [{"version": "310.8.SF-v2", "url": "https://example.com/a.zip"}]}"#;
        assert!(matches!(
            RhiSnapshot::parse(GAME_JSON, empty_dlss, "t"),
            Err(ManifestError::Malformed(_))
        ));
        let empty_nr = r#"{"dlss": [{"version": "310.8.0", "url": "https://example.com/a.zip"}], "dlssnr": []}"#;
        assert!(matches!(
            RhiSnapshot::parse(GAME_JSON, empty_nr, "t"),
            Err(ManifestError::Malformed(_))
        ));
    }

    #[test]
    fn http_urls_are_rejected() {
        let http_dlss = r#"{
            "dlss": [{"version": "310.8.0", "url": "http://example.com/a.zip"}],
            "dlssnr": [{"version": "310.8.SF-v2", "url": "https://example.com/b.zip"}]
        }"#;
        assert!(matches!(
            RhiSnapshot::parse(GAME_JSON, http_dlss, "t"),
            Err(ManifestError::InsecureEntry(_))
        ));
    }

    #[test]
    fn online_success_replaces_cache_and_reports_origin() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("snapshot.json");
        let client = FakeHttp::with_texts([
            (GAME_MANIFEST_URL, GAME_JSON),
            (DLSS_MANIFEST_URL, DLSS_JSON),
        ]);
        let loaded = refresh_or_cached(&client, &cache).unwrap();
        assert_eq!(loaded.origin, SnapshotOrigin::Online);
        assert!(loaded.error.is_none());
        assert!(cache.is_file(), "在线成功后必须写入缓存");
        assert_eq!(client.call_count(), 2, "两份清单各请求一次");
    }

    #[test]
    fn no_remote_and_no_cache_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("snapshot.json");
        let client = FakeHttp::new();
        assert!(matches!(
            refresh_or_cached(&client, &cache),
            Err(ManifestError::NoSnapshot(_))
        ));
    }
}
