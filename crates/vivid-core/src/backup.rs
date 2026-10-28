//! 备份与还原。
//!
//! CLAUDE.md 第六节把这件事的优先级定为「等同核心功能」，理由是对中文小白来说，
//! 「敢不敢试」完全取决于「能不能退回去」。所以这里的正确性要求比替换本身还高。
//!
//! 三条性质，每条都有测试守着：
//!
//! 1. **首次备份绝不被后续替换覆盖。** 用户换三次 DLL，备份必须始终是最原始那份
//!    官方文件。第二次替换时若把备份刷成第一次装进去的文件，「还原」就会把用户
//!    带回一个他从没有过的状态——比不还原更糟。
//! 2. **还原前校验备份完整性。** 备份坏了宁可报错，也不能拿坏文件去覆盖游戏。
//! 3. **只作用于安全区的四个组件。** 即使上层传进来任意路径，这一层也要挡住——
//!    不是 DLSS 组件文件一律拒绝，神经渲染（RHI 上游版本）在管理范围内。
//!
//! 另一条设计选择：**备份文件放在原目录，索引只是便利。**
//! 备份自解释——只要 `nvngx_dlss.dll.dlss-backup` 还在，哪怕索引文件丢了、
//! 换了台电脑、甚至本工具卸载了，用户手动改个名就能还原。
//! 把还原能力押在一个中心化索引上，是拿用户的游戏赌我们自己的文件不出事。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::component::{DlssComponent, Recognized, recognize};
use crate::version::DllVersion;

/// 备份文件的后缀。
///
/// 刻意**不带产品名**：项目随时可能改名（见 `lib.rs` 的品牌名称常量），
/// 而改名不该让用户已有的备份失效。
pub const BACKUP_SUFFIX: &str = ".dlss-backup";

/// 一次替换留下的记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupRecord {
    /// 被替换的游戏内文件。
    pub original_path: PathBuf,
    /// 备份文件（就在 `original_path` 旁边）。
    pub backup_path: PathBuf,
    pub kind: DlssComponent,
    /// 备份里那份原始文件的版本。
    pub original_version: Option<DllVersion>,
    /// 替换进去的版本。
    pub installed_version: Option<DllVersion>,
    /// 原始文件的 SHA-256。还原前用它确认备份没被动过。
    pub original_sha256: String,
    /// 本地时间，形如 `2026-08-31 14:05:09`。给人看的，不参与逻辑。
    pub created_at: String,
}

/// 备份 / 还原过程中可能出的岔子。每条都要能直接说给玩家听。
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("找不到要替换的文件：{0}")]
    TargetMissing(PathBuf),

    #[error("找不到作为来源的文件：{0}")]
    SourceMissing(PathBuf),

    #[error("文件正被占用，多半是游戏还开着。请先完全退出游戏（包括启动器）再试一次。\n路径：{0}")]
    Locked(PathBuf),

    #[error("{0} 不是本工具能管理的组件，已拒绝操作")]
    NotAComponent(PathBuf),

    #[error("拒绝操作：{reason}")]
    Refused { reason: &'static str },

    #[error("来源是{source_kind}，目标是{target_kind}，两者不是同一种组件，不能互换")]
    ComponentMismatch {
        source_kind: &'static str,
        target_kind: &'static str,
    },

    #[error("没有找到这个文件的备份，无法还原。\n路径：{0}")]
    NoBackup(PathBuf),

    #[error("备份文件已损坏或被改动过，为安全起见没有还原。\n备份：{0}")]
    BackupCorrupted(PathBuf),

    #[error("读写文件出错：{0}")]
    Io(#[from] std::io::Error),
}

/// Windows 在文件被占用时给的错误码。翻译成人话比抛原始 IO 错误有用得多。
fn to_friendly(e: std::io::Error, path: &Path) -> BackupError {
    // 32 = ERROR_SHARING_VIOLATION，33 = ERROR_LOCK_VIOLATION
    if matches!(e.raw_os_error(), Some(32) | Some(33)) {
        BackupError::Locked(path.to_path_buf())
    } else {
        BackupError::Io(e)
    }
}

/// 目标文件对应的备份路径：在原文件名后直接追加后缀。
///
/// 追加而不是替换扩展名，是为了让备份**不再是 `.dll`**——
/// 游戏和反作弊都不会去加载它，也不会被误认成可用组件。
pub fn backup_path_for(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    target.with_file_name(name)
}

/// 这个文件是否被本工具改过（即存在备份）。
pub fn has_backup(target: &Path) -> bool {
    backup_path_for(target).is_file()
}

/// 计算文件的 SHA-256，十六进制小写。
pub fn hash_file(path: &Path) -> Result<String, BackupError> {
    let bytes = std::fs::read(path).map_err(|e| to_friendly(e, path))?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// 判断一个路径是不是安全区里的组件，顺带把红线挡在最外面。
fn component_of(path: &Path) -> Result<DlssComponent, BackupError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    match recognize(&name) {
        Recognized::Managed(k) => Ok(k),
        Recognized::Forbidden { reason } => Err(BackupError::Refused { reason }),
        Recognized::Unrelated => Err(BackupError::NotAComponent(path.to_path_buf())),
    }
}

/// 把 `source` 装到 `target` 的位置，装之前自动备份 `target`。
///
/// 备份的幂等性是这里最关键的性质：**备份已存在时不覆盖它**。
/// 见模块头注释第 1 条。
pub fn install(source: &Path, target: &Path) -> Result<BackupRecord, BackupError> {
    // ---- 纵深防御：这一层不信任调用方传进来的路径 ----
    let source_kind = component_of(source)?;
    let target_kind = component_of(target)?;
    if source_kind != target_kind {
        return Err(BackupError::ComponentMismatch {
            source_kind: source_kind.display_name(),
            target_kind: target_kind.display_name(),
        });
    }

    if !source.is_file() {
        return Err(BackupError::SourceMissing(source.to_path_buf()));
    }
    if !target.is_file() {
        return Err(BackupError::TargetMissing(target.to_path_buf()));
    }

    let backup = backup_path_for(target);

    // ---- 备份：只在首次替换时创建 ----
    let (original_sha256, original_version) = if backup.is_file() {
        // 已经备份过了。原始文件是备份里那份，不是现在 target 上的那份。
        (hash_file(&backup)?, DllVersion::from_pe_file(&backup).ok())
    } else {
        std::fs::copy(target, &backup).map_err(|e| to_friendly(e, target))?;

        // 立刻校验备份，确认它和原文件逐字节一致。
        // 备份没写对就往下走，等于让用户失去退路而不自知。
        let target_hash = hash_file(target)?;
        let backup_hash = hash_file(&backup)?;
        if target_hash != backup_hash {
            let _ = std::fs::remove_file(&backup); // 坏备份留着比没有更危险
            return Err(BackupError::BackupCorrupted(backup));
        }
        (backup_hash, DllVersion::from_pe_file(&backup).ok())
    };

    // ---- 替换 ----
    std::fs::copy(source, target).map_err(|e| to_friendly(e, target))?;

    Ok(BackupRecord {
        original_path: target.to_path_buf(),
        backup_path: backup,
        kind: target_kind,
        original_version,
        installed_version: DllVersion::from_pe_file(target).ok(),
        original_sha256,
        created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// 把 `target` 还原成备份里的原始文件，并清掉备份。
///
/// `expected_sha256` 来自替换时的记录。传 `None` 表示记录丢了——
/// 这时仍然允许还原（备份文件本身就是权威），但会做一次基本的合理性检查。
pub fn restore(target: &Path, expected_sha256: Option<&str>) -> Result<(), BackupError> {
    let backup = backup_path_for(target);
    if !backup.is_file() {
        return Err(BackupError::NoBackup(target.to_path_buf()));
    }

    let actual = hash_file(&backup)?;

    match expected_sha256 {
        // 有记录：逐字节比对，不符就拒绝。
        Some(expected) if expected != actual => {
            return Err(BackupError::BackupCorrupted(backup));
        }
        // 没记录：至少确认它是个能读出版本的有效 PE，
        // 免得把一个 0 字节的坏文件盖回游戏里。
        None if DllVersion::from_pe_file(&backup).is_err() => {
            return Err(BackupError::BackupCorrupted(backup));
        }
        _ => {}
    }

    std::fs::copy(&backup, target).map_err(|e| to_friendly(e, target))?;
    // 还原成功才删备份。删失败不算错误——文件回去了才是要紧事。
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &[u8] = b"MZ--original-official-dll--";
    const NEWER: &[u8] = b"MZ--newer-version-dll--";
    const EVEN_NEWER: &[u8] = b"MZ--even-newer-dll--";

    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("vivid-backup-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("game")).unwrap();
            std::fs::create_dir_all(dir.join("source")).unwrap();
            std::fs::write(dir.join("game").join("nvngx_dlss.dll"), ORIGINAL).unwrap();
            std::fs::write(dir.join("source").join("nvngx_dlss.dll"), NEWER).unwrap();
            Self { dir }
        }
        fn target(&self) -> PathBuf {
            self.dir.join("game").join("nvngx_dlss.dll")
        }
        fn source(&self) -> PathBuf {
            self.dir.join("source").join("nvngx_dlss.dll")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn install_replaces_target_and_keeps_a_backup() {
        let f = Fixture::new("basic");
        let rec = install(&f.source(), &f.target()).expect("应当成功");

        assert_eq!(std::fs::read(f.target()).unwrap(), NEWER, "目标应已被替换");
        assert_eq!(
            std::fs::read(&rec.backup_path).unwrap(),
            ORIGINAL,
            "备份应是原始文件"
        );
        assert!(has_backup(&f.target()));
    }

    /// 模块头注释第 1 条——本文件最重要的一条性质。
    ///
    /// 连装两次不同的 DLL，备份必须仍然是**最初**那份官方文件。
    /// 如果第二次替换刷新了备份，用户点「还原」会回到 NEWER，
    /// 而那是他从来没有过的状态。
    #[test]
    fn the_first_backup_is_never_overwritten_by_later_installs() {
        let f = Fixture::new("firstwins");

        install(&f.source(), &f.target()).unwrap();
        std::fs::write(f.source(), EVEN_NEWER).unwrap();
        let rec = install(&f.source(), &f.target()).unwrap();

        assert_eq!(std::fs::read(f.target()).unwrap(), EVEN_NEWER);
        assert_eq!(
            std::fs::read(&rec.backup_path).unwrap(),
            ORIGINAL,
            "第二次替换把备份刷掉了——还原会把用户带到一个他从没有过的版本"
        );

        restore(&f.target(), Some(&rec.original_sha256)).unwrap();
        assert_eq!(
            std::fs::read(f.target()).unwrap(),
            ORIGINAL,
            "还原必须回到最初的官方文件"
        );
    }

    #[test]
    fn restore_puts_the_original_back_and_removes_the_backup() {
        let f = Fixture::new("restore");
        let rec = install(&f.source(), &f.target()).unwrap();

        restore(&f.target(), Some(&rec.original_sha256)).unwrap();

        assert_eq!(std::fs::read(f.target()).unwrap(), ORIGINAL);
        assert!(!has_backup(&f.target()), "还原后不该留下备份文件");
    }

    /// 索引丢了也要能还原——备份文件本身就是权威。
    #[test]
    fn restore_works_without_a_record_when_the_backup_is_a_valid_pe() {
        let f = Fixture::new("norecord");
        install(&f.source(), &f.target()).unwrap();

        // 用一个真实的 PE 冒充备份，模拟「索引丢失但备份还在」
        let real_pe = std::fs::read(r"C:\Windows\System32\kernel32.dll").unwrap();
        std::fs::write(backup_path_for(&f.target()), &real_pe).unwrap();

        restore(&f.target(), None).expect("没有记录也该能还原");
        assert_eq!(std::fs::read(f.target()).unwrap(), real_pe);
    }

    /// 模块头注释第 2 条：备份被动过就拒绝还原。
    #[test]
    fn restore_refuses_a_tampered_backup() {
        let f = Fixture::new("tampered");
        let rec = install(&f.source(), &f.target()).unwrap();

        std::fs::write(&rec.backup_path, b"garbage").unwrap();

        let err = restore(&f.target(), Some(&rec.original_sha256)).expect_err("应当拒绝");
        assert!(matches!(err, BackupError::BackupCorrupted(_)), "{err}");
        assert_eq!(
            std::fs::read(f.target()).unwrap(),
            NEWER,
            "拒绝还原时不能动目标文件"
        );
    }

    /// 没有记录时，坏备份也要被拦住，不能盖回游戏里。
    #[test]
    fn restore_refuses_a_junk_backup_even_without_a_record() {
        let f = Fixture::new("junk");
        install(&f.source(), &f.target()).unwrap();
        std::fs::write(backup_path_for(&f.target()), b"").unwrap();

        let err = restore(&f.target(), None).expect_err("空备份应被拒绝");
        assert!(matches!(err, BackupError::BackupCorrupted(_)), "{err}");
    }

    #[test]
    fn restore_without_any_backup_says_so_plainly() {
        let f = Fixture::new("nobackup");
        let err = restore(&f.target(), None).expect_err("没备份就该报错");
        assert!(matches!(err, BackupError::NoBackup(_)), "{err}");
        assert!(err.to_string().contains("没有找到"), "错误要说人话：{err}");
    }

    /// 模块头注释第 3 条：不是组件的文件不能借这条路装进游戏。
    #[test]
    fn install_refuses_unrelated_or_mismatched_components() {
        let f = Fixture::new("redline");
        let bad = f.dir.join("source").join("d3d12.dll");
        std::fs::write(&bad, b"MZ").unwrap();

        let err = install(&bad, &f.target()).expect_err("无关文件不能作为来源");
        assert!(matches!(err, BackupError::NotAComponent(_)), "{err}");

        let g = f.dir.join("source").join("nvngx_dlssg.dll");
        std::fs::write(&g, b"MZ--frame-gen--").unwrap();
        let err = install(&g, &f.target()).expect_err("帧生成不能盖到超分上");
        assert!(
            matches!(err, BackupError::ComponentMismatch { .. }),
            "{err}"
        );
        assert_eq!(std::fs::read(f.target()).unwrap(), ORIGINAL);
    }

    #[test]
    fn backup_path_appends_rather_than_replaces_the_extension() {
        let p = backup_path_for(Path::new(r"C:\game\nvngx_dlss.dll"));
        assert_eq!(p, PathBuf::from(r"C:\game\nvngx_dlss.dll.dlss-backup"));
        assert!(
            p.to_string_lossy().ends_with(BACKUP_SUFFIX),
            "备份不能还是 .dll，否则可能被游戏或反作弊看见"
        );
    }

    #[test]
    fn hashing_is_stable_and_distinguishes_content() {
        let f = Fixture::new("hash");
        let a = hash_file(&f.target()).unwrap();
        let b = hash_file(&f.target()).unwrap();
        assert_eq!(a, b, "同一文件两次哈希必须一致");
        assert_ne!(a, hash_file(&f.source()).unwrap(), "不同内容必须不同哈希");
        assert_eq!(a.len(), 64, "SHA-256 十六进制应为 64 字符");
    }

    #[test]
    fn record_round_trips_through_json() {
        let f = Fixture::new("json");
        let rec = install(&f.source(), &f.target()).unwrap();

        let text = serde_json::to_string_pretty(&rec).unwrap();
        let back: BackupRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(rec, back, "记录必须能原样存取，否则重启后还原信息就丢了");
    }
}
