//! DLSS DLL 的版本表示、读取与排序。
//!
//! 为什么这件事值得单独成模块：本机实测发现**驱动自带的不一定最新**
//! （GTA V Enhanced 的 `nvngx_dlssg.dll` 是 310.4.0，而驱动 581.08 里的才 310.2.1）。
//! 所以「哪个更新」必须走真实文件版本比较，任何按来源猜测的捷径都是错的。
//! 见 `CLAUDE.md` 第五节的实测数据。

use std::path::{Path, PathBuf};

/// 一个四段式文件版本，形如 `310.4.0.0`。
///
/// 直接对应 PE 资源里 `VS_FIXEDFILEINFO` 的四个 u16，原样保留。
/// 这里**不解释**版本号的含义（比如「310 就是 DLSS 4」），那属于上层展示的事——
/// 映射关系会随 NVIDIA 的发布变化，埋在比较逻辑里迟早出错。
///
/// 派生的 `Ord` 按 major → minor → build → revision 逐段比较，正是我们要的语义。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct DllVersion {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
}

impl DllVersion {
    pub const fn new(major: u16, minor: u16, build: u16, revision: u16) -> Self {
        Self {
            major,
            minor,
            build,
            revision,
        }
    }

    /// 从 `VS_FIXEDFILEINFO` 的两个 DWORD 还原。高 16 位在前，这是 Win32 的既定布局。
    pub const fn from_ms_ls(ms: u32, ls: u32) -> Self {
        Self {
            major: (ms >> 16) as u16,
            minor: (ms & 0xFFFF) as u16,
            build: (ls >> 16) as u16,
            revision: (ls & 0xFFFF) as u16,
        }
    }

    /// 读取一个 PE 文件（.dll/.exe）的文件版本。
    ///
    /// 走 PE 资源解析而不是 Win32 的 `GetFileVersionInfo`：纯 Rust、零 `unsafe`，
    /// 而且能在非 Windows 上跑单元测试。
    pub fn from_pe_file(path: &Path) -> Result<Self, VersionError> {
        use pelite::FileMap;
        use pelite::pe64::{Pe, PeFile};

        let map = FileMap::open(path).map_err(|source| VersionError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        // DLSS 只服务 64 位游戏，32 位 PE 直接当作「不是我们要找的东西」报错，
        // 而不是悄悄放过——放过会让上层以为版本读取成功了。
        let pe = PeFile::from_bytes(map.as_ref()).map_err(|e| VersionError::NotPe {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;

        let resources = pe.resources().map_err(|e| VersionError::NoVersionInfo {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;

        let info = resources
            .version_info()
            .map_err(|e| VersionError::NoVersionInfo {
                path: path.to_path_buf(),
                detail: e.to_string(),
            })?;

        let fixed = info.fixed().ok_or_else(|| VersionError::NoVersionInfo {
            path: path.to_path_buf(),
            detail: "资源里没有 VS_FIXEDFILEINFO".to_string(),
        })?;

        // pelite 的 VS_VERSION 字段命名与 Win32 惯例**是反的**：
        // 它的 Patch 对应 dwFileVersionLS 的高 16 位（Win32 称 build），
        // 它的 Build 对应低 16 位（Win32 称 revision）。
        // 读反了就会把 2.2.10.0 读成 2.2.0.10——下面 kernel32 那条测试守的就是这个。
        //
        // 另：别用 VS_VERSION 自带的 Ord。它的字段顺序是 Minor 排在 Major 前面，
        // derive 出来的比较语义是错的，必须转成 DllVersion 再比。
        let v = fixed.dwFileVersion;
        Ok(Self {
            major: v.Major,
            minor: v.Minor,
            build: v.Patch,
            revision: v.Build,
        })
    }
}

impl std::fmt::Display for DllVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.build, self.revision
        )
    }
}

/// 一句话说清「这个文件是什么」。
///
/// **用途只有一个**：安装前的预演清单里，标着「替换」的那一行要告诉用户
/// *正在被替换掉的是什么*。RHI 有等价的「外来 DLL 识别」，防的是把用户
/// 已经装好的 Special K / ENB / DXVK 顶掉而他自己不知道。
///
/// 名字直接取 PE 版本资源里的 `ProductName`（读不到就退到 `FileDescription`
/// 和 `CompanyName`）——**不做任何映射表**：NVIDIA 的 DLL 自己就写着
/// 「NVIDIA DLSS」，ReShade 自己就写着「ReShade」，我们不需要替它们改名，
/// 也就不会因为映射表过期而说错。
///
/// 读不出版本资源返回 `None`（很多正当文件本来就没有版本信息），
/// 由调用方决定怎么说——**不编一个名字出来**。
pub fn describe_pe_file(path: &Path) -> Option<String> {
    use pelite::FileMap;
    use pelite::pe64::{Pe, PeFile};

    let map = FileMap::open(path).ok()?;
    let pe = PeFile::from_bytes(map.as_ref()).ok()?;
    let info = pe.resources().ok()?.version_info().ok()?;
    let lang = info.translation().first().copied()?;

    let name = ["ProductName", "FileDescription", "CompanyName"]
        .iter()
        .find_map(|key| info.value(lang, key))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;

    match DllVersion::from_pe_file(path) {
        Ok(version) => Some(format!("{name} {version}")),
        Err(_) => Some(name),
    }
}

/// 读取版本时可能出的岔子。每条都要能直接说给玩家听。
#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("读不到文件 {path}：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} 不是有效的 64 位程序文件（{detail}）")]
    NotPe { path: PathBuf, detail: String },

    #[error("{path} 里没有版本信息（{detail}）")]
    NoVersionInfo { path: PathBuf, detail: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预演清单要告诉用户「即将被替换掉的是什么」。
    /// 拿系统自带的 PE 文件做样本：名字和版本都由文件自己说，我们不映射。
    #[test]
    fn a_real_pe_file_describes_itself() {
        let kernel32 = Path::new(r"C:\Windows\System32\kernel32.dll");
        if !kernel32.is_file() {
            return; // 非 Windows 上跳过
        }
        let described = describe_pe_file(kernel32).expect("系统 DLL 一定有版本资源");
        assert!(described.contains("Windows"), "{described}");
        // 版本号跟在名字后面，用户一眼能看出新旧。
        assert!(described.contains('.'), "{described}");
    }

    /// 没有版本资源、或者根本不是 PE 的文件——**不编一个名字出来**。
    #[test]
    fn a_file_without_version_info_is_not_described() {
        let temp = tempfile::tempdir().unwrap();
        let junk = temp.path().join("junk.dll");
        std::fs::write(&junk, b"not a pe at all").unwrap();
        assert_eq!(describe_pe_file(&junk), None);
        assert_eq!(describe_pe_file(&temp.path().join("missing.dll")), None);
    }

    /// 本机 2026-08-31 实测到的真实版本，按从旧到新排列。
    /// 用真实数据当测试向量，比编造的 1.0/2.0 更能暴露分段比较的错误。
    const REAL_WORLD_ASCENDING: &[DllVersion] = &[
        DllVersion::new(2, 2, 10, 0),  // Red Dead Redemption 2，老 CNN
        DllVersion::new(2, 4, 12, 0),  // Call of Duty MW II，老 CNN
        DllVersion::new(310, 1, 0, 0), // 永劫无间测试服，Transformer
        DllVersion::new(310, 2, 1, 0), // 驱动 581.08 自带的 dlssg
        DllVersion::new(310, 4, 0, 0), // GTA V Enhanced，本机最新
    ];

    #[test]
    fn orders_real_world_versions_correctly() {
        for pair in REAL_WORLD_ASCENDING.windows(2) {
            assert!(pair[0] < pair[1], "{} 应当早于 {}", pair[0], pair[1]);
        }
    }

    /// CLAUDE.md 第五节记下的坑：驱动里的 310.2.1 比游戏自带的 310.4.0 旧。
    /// 任何「驱动版本最高」的假设都会在这里翻车。
    #[test]
    fn driver_bundled_version_is_not_assumed_newest() {
        let from_driver = DllVersion::new(310, 2, 1, 0);
        let from_game = DllVersion::new(310, 4, 0, 0);
        assert!(
            from_game > from_driver,
            "游戏自带的可能比驱动新，不能按来源排序"
        );
    }

    /// 分段比较不能退化成字典序或浮点比较：2.4 必须小于 2.10。
    #[test]
    fn compares_numerically_not_lexicographically() {
        assert!(DllVersion::new(2, 4, 0, 0) < DllVersion::new(2, 10, 0, 0));
        assert!(DllVersion::new(2, 4, 12, 0) < DllVersion::new(310, 1, 0, 0));
    }

    #[test]
    fn unpacks_win32_dword_layout() {
        // 310.4.0.0 → MS = 310<<16 | 4，LS = 0
        assert_eq!(
            DllVersion::from_ms_ls((310 << 16) | 4, 0),
            DllVersion::new(310, 4, 0, 0)
        );
        assert_eq!(
            DllVersion::from_ms_ls((2 << 16) | 2, 10 << 16),
            DllVersion::new(2, 2, 10, 0)
        );
    }

    #[test]
    fn displays_as_four_dotted_segments() {
        assert_eq!(DllVersion::new(310, 4, 0, 0).to_string(), "310.4.0.0");
    }

    /// 真机判据：能从一个确定存在的系统 DLL 里读出非零版本。
    /// 这条证明 PE 解析链路是通的，而不只是结构体能构造。
    #[cfg(windows)]
    #[test]
    fn reads_version_from_a_real_system_dll() {
        let path = Path::new(r"C:\Windows\System32\kernel32.dll");
        let v = DllVersion::from_pe_file(path).expect("应能读出 kernel32 的版本");
        // 这条同时验证字段映射是否正确：kernel32 的版本形如 10.0.22621.xxxx。
        // 若 build 与 revision 读反了，build 会掉到四位数，断言立刻红。
        assert_eq!(
            v.major, 10,
            "Windows 10/11 的 kernel32 主版本应为 10，实为 {v}"
        );
        assert!(v.build > 10_000, "build 段疑似与 revision 读反了：{v}");
    }

    #[cfg(windows)]
    #[test]
    fn reports_a_readable_error_for_missing_file() {
        let err = DllVersion::from_pe_file(Path::new(r"C:\definitely\not\here.dll"))
            .expect_err("不存在的文件应当报错");
        let msg = err.to_string();
        assert!(msg.contains("读不到文件"), "错误信息应当是中文人话：{msg}");
    }
}
