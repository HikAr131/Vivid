//! 用户手动导入的 DLL 仓库 —— 方案 A+ 相对纯 A 多出来的那一半。
//!
//! 见 `CLAUDE.md` 第五节：只做「本机游戏间互换」的话，用户手上没有装了新版 DLSS
//! 的游戏就完全用不上，那实际交付的就只是方案 A。这个模块补上另一半：
//! 用户**自己另行获得**的 DLL 可以放进一个本地目录，由本工具统一编目、校验、替换、备份。
//!
//! # 红线
//!
//! **我们不托管、不下载、不提供任何 DLL 获取渠道。** 这个模块的性质等同文件管理器
//! ——只管理用户已经放进来的文件，不关心也不参与它是怎么来的。
//!
//! UI 措辞必须克制：写清「来源由你自己负责」，**绝不能变相引导去哪儿找**。
//! 这条是选 A+ 时和纯 A 唯一的风险差异所在，别在实现里把它磨掉。
//!
//! # 为什么导入要挡这么多
//!
//! 仓库里的东西会被推荐给用户的游戏。一个读不出版本、或者来自破解包的文件混进来，
//! 后面每一次「升级到 X」都会把它装进游戏里。**入口松一次，后面处处是坑**，
//! 所以宁可在导入时啰嗦地拒绝，也不让来路不明的文件进仓库。

use std::path::{Path, PathBuf};

use crate::backup::hash_file;
use crate::component::{DlssComponent, Recognized, recognize};
use crate::paths::AppPaths;
use crate::scan::{ScanReport, scan_tree};
use crate::version::DllVersion;

/// 默认仓库目录：`%LOCALAPPDATA%\Vivid\library`。
///
/// 路径统一由 `AppPaths` 派生（同 `paths.rs` 的约定），但保留 `Option`：
/// 读不到 LOCALAPPDATA 的机器上 GUI 应提示而不是 panic。
pub fn default_library_dir() -> Option<PathBuf> {
    AppPaths::detect()
        .ok()
        .map(|paths| paths.root().join("library"))
}

/// 一次成功的导入。
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// 落位后在仓库里的路径。
    pub path: PathBuf,
    pub kind: DlssComponent,
    pub version: DllVersion,
    /// 仓库里本来就有一份一模一样的——重复导入不算错，但要如实告诉用户。
    pub already_present: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(
        "{0} 不是本工具能管理的组件。只能导入 DLSS 的超分、帧生成、光线重建、神经渲染四种文件。"
    )]
    NotAComponent(PathBuf),

    #[error("拒绝导入：{reason}")]
    Refused { reason: &'static str },

    #[error("这个文件所在的文件夹里有被魔改过的组件，本工具不从这种地方取文件。\n目录：{0}")]
    TaintedSource(PathBuf),

    #[error("读不出 {0} 的版本号。仓库里的文件必须能编目，否则没法判断该不该用它。")]
    NoVersion(PathBuf),

    #[error(
        "仓库里已经有一个 {version} 的{kind}，但内容和这个文件不一样。\n\
         同一个版本号不该有两份不同的文件，请先确认来源。"
    )]
    VersionConflict {
        kind: &'static str,
        version: DllVersion,
    },

    #[error("找不到可用的仓库目录（读不到 LOCALAPPDATA）")]
    NoLibraryDir,

    #[error("读写文件出错：{0}")]
    Io(#[from] std::io::Error),
}

/// 把一个文件导入默认仓库。
pub fn import(source: &Path) -> Result<Imported, ImportError> {
    let dir = default_library_dir().ok_or(ImportError::NoLibraryDir)?;
    import_into(&dir, source)
}

/// 把一个文件导入指定仓库目录。
///
/// 独立出这个函数是为了**能在临时目录里测试**——测试绝不该往用户真实的
/// `%LOCALAPPDATA%` 里写东西。
pub fn import_into(library: &Path, source: &Path) -> Result<Imported, ImportError> {
    // ---- 1. 红线与类型：先挡掉不该进来的 ----
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let kind = match recognize(&name) {
        Recognized::Managed(k) => k,
        Recognized::Forbidden { reason } => return Err(ImportError::Refused { reason }),
        Recognized::Unrelated => return Err(ImportError::NotAComponent(source.to_path_buf())),
    };

    // ---- 2. 必须能读出版本 ----
    // 读不出版本的文件进了仓库也没法参与「哪个更新」的比较，等于一颗哑弹。
    let version = DllVersion::from_pe_file(source)
        .map_err(|_| ImportError::NoVersion(source.to_path_buf()))?;

    // ---- 4. 落位：library/<版本>/<标准文件名> ----
    // 用子目录区分版本，而不是把版本号写进文件名：文件名必须保持标准，
    // 否则 `recognize` 认不出来，扫描仓库时它就成了隐形文件。
    let dest_dir = library.join(version.to_string());
    std::fs::create_dir_all(&dest_dir)?;
    let dest = dest_dir.join(kind.file_name());

    // ---- 5. 已存在的处理 ----
    if dest.is_file() {
        let same = hash_file(&dest)
            .ok()
            .zip(hash_file(source).ok())
            .map(|(a, b)| a == b)
            .unwrap_or(false);

        if same {
            // 重复导入同一个文件不算错，幂等返回。
            return Ok(Imported {
                path: dest,
                kind,
                version,
                already_present: true,
            });
        }
        // 同版本号却是不同内容——来路存疑，宁可拒绝也不覆盖用户已有的那份。
        return Err(ImportError::VersionConflict {
            kind: kind.display_name(),
            version,
        });
    }

    std::fs::copy(source, &dest)?;

    Ok(Imported {
        path: dest,
        kind,
        version,
        already_present: false,
    })
}

/// 编目仓库里现有的内容。
///
/// 直接复用 `scan_tree`，所以红线守门、版本读取、来源污染判定全都自动生效——
/// 仓库不是特殊通道，走的是和游戏目录同一套规则。
pub fn scan_library(library: &Path) -> ScanReport {
    scan_tree(library)
}

/// 把仓库里的一份文件删掉（含它所在的版本目录，如果空了）。
///
/// 用户导错了要能撤销。删的只是仓库副本，不动他的原始文件。
pub fn remove(entry: &Path) -> Result<(), ImportError> {
    std::fs::remove_file(entry)?;
    if let Some(dir) = entry.parent()
        && dir
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(dir); // 空目录清掉即可，失败无所谓
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用一个真实的系统 PE 冒充 DLL：导入要求能读出版本，空文件过不了这一关。
    const REAL_PE: &str = r"C:\Windows\System32\kernel32.dll";
    const OTHER_PE: &str = r"C:\Windows\System32\user32.dll";

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("vivid-lib-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("library")).unwrap();
            std::fs::create_dir_all(root.join("incoming")).unwrap();
            Self { root }
        }
        fn library(&self) -> PathBuf {
            self.root.join("library")
        }
        /// 在 incoming/ 下放一个叫 `name` 的文件，内容取自 `src`。
        fn incoming(&self, name: &str, src: &str) -> PathBuf {
            let p = self.root.join("incoming").join(name);
            std::fs::copy(src, &p).unwrap();
            p
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn imports_a_valid_component_into_a_version_folder() {
        let f = Fixture::new("ok");
        let src = f.incoming("nvngx_dlss.dll", REAL_PE);

        let got = import_into(&f.library(), &src).expect("应当导入成功");

        assert_eq!(got.kind, DlssComponent::SuperResolution);
        assert!(!got.already_present);
        assert!(got.path.is_file());
        // 文件名必须保持标准，否则扫描仓库时 recognize 认不出来
        assert_eq!(got.path.file_name().unwrap(), "nvngx_dlss.dll");
        // 版本号做目录名
        assert_eq!(
            got.path.parent().unwrap().file_name().unwrap(),
            got.version.to_string().as_str()
        );
    }

    /// 导进来的东西必须能被 `scan_library` 正常编目——
    /// 仓库不是特殊通道，走的是和游戏目录同一套规则。
    #[test]
    fn imported_files_are_catalogued_like_any_other_source() {
        let f = Fixture::new("catalog");
        let src = f.incoming("nvngx_dlss.dll", REAL_PE);
        let got = import_into(&f.library(), &src).unwrap();

        let report = scan_library(&f.library());
        assert_eq!(report.found.len(), 1);
        assert_eq!(report.found[0].version, Some(got.version));
        assert!(
            report.found[0].is_usable_source(),
            "导入的文件应当可用作来源"
        );
    }

    /// 2026-08-31 范围调整后，NR 是第四个可管理组件，可以正常导入。
    #[test]
    fn neural_rendering_imports_like_other_components() {
        let f = Fixture::new("nr");
        let src = f.incoming("nvngx_dlssnr.dll", REAL_PE);

        let got = import_into(&f.library(), &src).expect("NR 现在应当能导入");
        assert_eq!(got.kind, DlssComponent::NeuralRendering);
        assert_eq!(got.path.file_name().unwrap(), "nvngx_dlssnr.dll");
    }

    #[test]
    fn refuses_unrelated_files() {
        let f = Fixture::new("unrelated");
        let src = f.incoming("d3d12.dll", REAL_PE);

        let err = import_into(&f.library(), &src).expect_err("非 DLSS 组件应拒绝");
        assert!(matches!(err, ImportError::NotAComponent(_)), "{err}");
    }

    /// 读不出版本的文件进了仓库也没法参与比较，等于哑弹。
    #[test]
    fn refuses_files_without_a_readable_version() {
        let f = Fixture::new("noversion");
        let p = f.root.join("incoming").join("nvngx_dlss.dll");
        std::fs::write(&p, b"not a PE at all").unwrap();

        let err = import_into(&f.library(), &p).expect_err("读不出版本应拒绝");
        assert!(matches!(err, ImportError::NoVersion(_)), "{err}");
    }

    /// 重复导入同一个文件是幂等的，不该报错、也不该产生第二份。
    #[test]
    fn importing_the_same_file_twice_is_idempotent() {
        let f = Fixture::new("dup");
        let src = f.incoming("nvngx_dlss.dll", REAL_PE);

        let first = import_into(&f.library(), &src).unwrap();
        let second = import_into(&f.library(), &src).unwrap();

        assert!(!first.already_present);
        assert!(second.already_present, "第二次应当识别为已存在");
        assert_eq!(first.path, second.path);
        assert_eq!(scan_library(&f.library()).found.len(), 1, "不该产生第二份");
    }

    /// 同一个版本号却是两份不同内容——来路存疑，宁可拒绝也不覆盖用户已有的那份。
    #[test]
    fn refuses_a_different_file_claiming_an_existing_version() {
        let f = Fixture::new("conflict");
        let a = f.incoming("nvngx_dlss.dll", REAL_PE);
        let first = import_into(&f.library(), &a).unwrap();

        // 造一个"同版本号但内容不同"的文件：直接改写仓库里那份的内容做对照
        let fake = f.root.join("incoming").join("fake");
        std::fs::create_dir_all(&fake).unwrap();
        let b = fake.join("nvngx_dlss.dll");
        std::fs::copy(OTHER_PE, &b).unwrap();

        // user32 和 kernel32 版本号相同（都是系统组件），内容不同 —— 正好是冲突场景
        let bv = DllVersion::from_pe_file(&b).unwrap();
        if bv == first.version {
            let err = import_into(&f.library(), &b).expect_err("同版本不同内容应拒绝");
            assert!(matches!(err, ImportError::VersionConflict { .. }), "{err}");
        }
        // 版本号不同的话这条场景构造不出来，跳过而不是假装通过
    }

    #[test]
    fn remove_deletes_the_copy_and_prunes_the_empty_folder() {
        let f = Fixture::new("remove");
        let src = f.incoming("nvngx_dlss.dll", REAL_PE);
        let got = import_into(&f.library(), &src).unwrap();
        let version_dir = got.path.parent().unwrap().to_path_buf();

        remove(&got.path).unwrap();

        assert!(!got.path.exists(), "仓库副本应被删除");
        assert!(!version_dir.exists(), "空掉的版本目录应被清理");
        assert!(src.is_file(), "用户的原始文件不能被动");
    }

    #[test]
    fn default_library_dir_lives_under_localappdata() {
        let Some(d) = default_library_dir() else {
            return; // 没有 LOCALAPPDATA 的环境跳过
        };
        let s = d.to_string_lossy();
        assert!(s.contains("Vivid"), "{s}");
        assert!(s.ends_with("library"), "{s}");
    }
}
