//! 扫描目录树，编目可管理的 DLSS 组件，并把可疑的逆向产物**单独报出来**。
//!
//! 两条刻意的设计选择，都有真机依据：
//!
//! 1. **单个文件出问题不中断整次扫描。** 游戏目录里什么都可能有——占位文件、
//!    下载了一半的文件、权限受限的文件。为一个坏文件让用户的整次扫描失败，
//!    是把工具自己的洁癖变成用户的麻烦。
//! 2. **可疑文件进 `blocked` 而不是被丢弃。** 用户机器上真有逆向产物时，
//!    工具装作没看见，他只会转头自己去手动装。
//!
//! 2026-08-31 范围调整：`nvngx_dlssnr.dll` 成为普通可管理组件（来源由 RHI
//! 清单指向、界面标注「RHI 上游版本」），因此旧的「NR DLL 污染整个目录」规则
//! 撤销；`.ptx` 落在 `dlssnr-*` / `*-patched` 目录里的诊断展示保留。

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::component::{DlssComponent, Recognized, recognize, tamper_evidence};
use crate::version::DllVersion;

/// 扫到的一个可管理组件。
#[derive(Debug, Clone)]
pub struct FoundComponent {
    pub path: PathBuf,
    pub kind: DlssComponent,
    /// 版本读不出来时为 `None`（文件损坏、被占用、不是有效 PE）。
    /// 保留条目而不是丢弃，这样 UI 能显示「这里有个文件但读不出版本」。
    pub version: Option<DllVersion>,
}

impl FoundComponent {
    /// 能否作为替换的来源。读不出版本的不行——没法参与「哪个更新」的比较。
    pub fn is_usable_source(&self) -> bool {
        self.version.is_some()
    }
}

/// 命中红线、本工具拒绝处理的文件。
#[derive(Debug, Clone)]
pub struct BlockedFile {
    pub path: PathBuf,
    /// 直接给玩家看的中文说明。
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub found: Vec<FoundComponent>,
    pub blocked: Vec<BlockedFile>,
}

impl ScanReport {
    /// 本机某个组件的最高**可用**版本——「能升到哪」的答案。
    ///
    /// 靠实际文件版本取 `max()`（真机踩过的坑：驱动自带的 310.2.1 比
    /// 游戏里的 310.4.0 旧，不能按来源假定谁新）。
    pub fn newest(&self, kind: DlssComponent) -> Option<DllVersion> {
        self.found
            .iter()
            .filter(|f| f.kind == kind && f.is_usable_source())
            .filter_map(|f| f.version)
            .max()
    }

    /// 提供该组件最高版本的那个文件，替换时的源就是它。
    pub fn newest_source(&self, kind: DlssComponent) -> Option<&FoundComponent> {
        self.found
            .iter()
            .filter(|f| f.kind == kind && f.is_usable_source())
            .max_by_key(|f| f.version)
    }

    pub fn merge(&mut self, other: ScanReport) {
        self.found.extend(other.found);
        self.blocked.extend(other.blocked);
    }
}

/// 扫一棵目录树，编目其中的 DLSS 组件。
///
/// 不跟随符号链接：游戏目录里的链接可能指向库外甚至成环。
pub fn scan_tree(root: &Path) -> ScanReport {
    let mut report = ScanReport::default();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok); // 读不了的目录跳过，不让它毁掉整次扫描

    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();

        // 可疑逆向产物优先报出来。判定用**完整路径**而非文件名：`.ptx` 要看上下文。
        if let Some(reason) = tamper_evidence(path) {
            report.blocked.push(BlockedFile {
                path: path.to_path_buf(),
                reason,
            });
            continue;
        }

        let name = entry.file_name().to_string_lossy();
        match recognize(&name) {
            Recognized::Managed(kind) => {
                let path = path.to_path_buf();
                let version = DllVersion::from_pe_file(&path).ok();
                report.found.push(FoundComponent {
                    path,
                    kind,
                    version,
                });
            }
            // 兜底：`tamper_evidence` 理论上已经捕获，但识别层再挡一道。
            Recognized::Forbidden { reason } => report.blocked.push(BlockedFile {
                path: path.to_path_buf(),
                reason,
            }),
            Recognized::Unrelated => {}
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sr() -> DlssComponent {
        DlssComponent::SuperResolution
    }

    /// 造一棵一次性目录树，内容照着用户素材目录的真实构成来。
    fn make_tree(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vivid-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Some Game").join("x64")).unwrap();
        std::fs::create_dir_all(dir.join("Clean Game")).unwrap();
        std::fs::create_dir_all(dir.join("dlssnr-sm86-patched")).unwrap();

        for rel in [
            "Some Game/x64/nvngx_dlss.dll",
            "Some Game/x64/nvngx_dlssg.dll",
            "Some Game/x64/nvngx_dlssnr.dll",
            "Some Game/readme.txt",
            "Clean Game/nvngx_dlss.dll",
            "dlssnr-sm86-patched/mod00.ptx",
            "dlssnr-sm86-patched/mod04.fast.ptx",
        ] {
            std::fs::write(dir.join(rel), b"").unwrap();
        }
        dir
    }

    #[test]
    fn catalogs_managed_components_and_ignores_unrelated() {
        let dir = make_tree("catalog");
        let r = scan_tree(&dir);

        assert_eq!(r.found.len(), 4, "应收 4 个 DLL，实际 {:?}", r.found);
        assert!(
            r.found
                .iter()
                .any(|f| f.kind == DlssComponent::NeuralRendering),
            "NR DLL 现在必须被编目为可管理组件"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 逆向产物的诊断展示：ptx 在可疑目录里必须单独报出来。
    #[test]
    fn blocks_reversed_ptx_on_a_real_filesystem() {
        let dir = make_tree("redline");
        let r = scan_tree(&dir);

        assert_eq!(r.blocked.len(), 2, "2 个 ptx：{:?}", r.blocked);
        for b in &r.blocked {
            assert!(b.path.to_string_lossy().ends_with(".ptx"));
            assert!(!b.reason.is_empty(), "拦截必须带中文理由");
            assert!(
                !r.found.iter().any(|f| f.path == b.path),
                "{} 同时进了两个清单",
                b.path.display()
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn newest_picks_the_highest_version_not_the_last_seen() {
        let mut r = ScanReport::default();
        for v in [(310, 1, 0), (310, 4, 0), (2, 4, 12), (310, 2, 1)] {
            r.found.push(FoundComponent {
                path: PathBuf::from(format!("{}.dll", v.0)),
                kind: sr(),
                version: Some(DllVersion::new(v.0, v.1, v.2, 0)),
            });
        }
        assert_eq!(
            r.newest(sr()),
            Some(DllVersion::new(310, 4, 0, 0)),
            "必须取最大值，不是最后一个或第一个"
        );
    }

    #[test]
    fn newest_is_per_component_not_global() {
        let mut r = ScanReport::default();
        r.found.push(FoundComponent {
            path: PathBuf::from("a"),
            kind: sr(),
            version: Some(DllVersion::new(310, 4, 0, 0)),
        });
        r.found.push(FoundComponent {
            path: PathBuf::from("b"),
            kind: DlssComponent::FrameGeneration,
            version: Some(DllVersion::new(310, 2, 1, 0)),
        });
        assert_eq!(
            r.newest(DlssComponent::FrameGeneration),
            Some(DllVersion::new(310, 2, 1, 0))
        );
        assert_eq!(r.newest(DlssComponent::RayReconstruction), None);
    }

    #[test]
    fn keeps_entries_whose_version_cannot_be_read() {
        let dir = make_tree("badpe");
        let r = scan_tree(&dir);

        assert!(!r.found.is_empty());
        assert!(
            r.found.iter().all(|f| f.version.is_none()),
            "空文件不该解析出版本"
        );
        assert_eq!(r.newest(sr()), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scanning_a_missing_directory_is_empty_not_a_panic() {
        let r = scan_tree(Path::new(r"C:\definitely\not\here"));
        assert!(r.found.is_empty() && r.blocked.is_empty());
    }
}
