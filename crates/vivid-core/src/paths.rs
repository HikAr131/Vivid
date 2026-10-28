//! Vivid 自己的数据目录：缓存、备份、安装状态、待处理请求。
//!
//! 全部落在 `%LOCALAPPDATA%\Vivid` 下，理由与旧版 `library::default_library_dir`
//! 相同：程序目录可能在 Program Files（需要管理员），也可能被绿色版用户随手挪走。
//! 游戏目录之外，Vivid 不该往别处写任何东西。
//!
//! `under` 构造器是给测试与提权 helper 用的——生产代码一律走 `detect`，
//! 保证真实数据只落在本机 LOCALAPPDATA。

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("读不到 LOCALAPPDATA，无法确定 Vivid 数据目录")]
    NoLocalAppData,
}

/// Vivid 数据目录的路径集合。所有可变路径都必须在 `root` 之下。
#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// 按本机环境定位 `%LOCALAPPDATA%\Vivid`。
    pub fn detect() -> Result<Self, PathError> {
        let base = std::env::var_os("LOCALAPPDATA").ok_or(PathError::NoLocalAppData)?;
        Ok(Self::under(PathBuf::from(base).join("Vivid")))
    }

    /// 以指定目录为根构造路径集合（测试与提权 helper 用）。
    pub fn under(root: PathBuf) -> Self {
        Self { root }
    }

    /// 数据根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 下载与解析的缓存目录。
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// RHI 两份清单合并后的 last-known-good 快照。
    pub fn manifest_cache(&self) -> PathBuf {
        self.cache_dir().join("rhi-snapshot.json")
    }

    /// ReShade / RenoDX 上游发布解析结果的 last-known-good。
    pub fn upstream_cache(&self) -> PathBuf {
        self.cache_dir().join("upstream-snapshot.json")
    }

    /// 下载并校验过的组件缓存（`<kind>/<version>/<file>`）。
    pub fn artifacts_dir(&self) -> PathBuf {
        self.cache_dir().join("artifacts")
    }

    /// 首次接管目标时的原始内容（baseline）。
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// 每个游戏的 installation ledger。
    pub fn installations_dir(&self) -> PathBuf {
        self.root.join("installations")
    }

    /// 提权请求与中断恢复 journal 的暂存目录。
    pub fn pending_dir(&self) -> PathBuf {
        self.root.join("pending")
    }

    /// 「首次启动说明」的「不再显示」标记。
    ///
    /// 用一个空文件而不是配置项：这条状态只有两种可能，存在即不再显示。
    /// 用户想让它再出来一次，删掉这个文件就行——不需要懂 JSON。
    pub fn welcome_marker(&self) -> PathBuf {
        self.root.join("welcome-dismissed")
    }
}

#[cfg(test)]
mod tests {
    use super::AppPaths;
    use std::path::PathBuf;

    #[test]
    fn every_mutable_path_stays_under_the_supplied_root() {
        let root = PathBuf::from(r"C:\Users\Test\AppData\Local\Vivid");
        let paths = AppPaths::under(root.clone());
        for path in [
            paths.cache_dir(),
            paths.manifest_cache(),
            paths.upstream_cache(),
            paths.artifacts_dir(),
            paths.backups_dir(),
            paths.installations_dir(),
            paths.pending_dir(),
            paths.welcome_marker(),
        ] {
            assert!(path.starts_with(&root), "{}", path.display());
        }
    }
}
