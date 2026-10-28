//! 备份还原演练：用真实的游戏 DLL 跑一遍完整流程。
//!
//! **不碰用户的游戏。** 自动挑出本机版本差距最大的一对真实 DLL，
//! 复制进临时沙箱后才动手；原游戏目录全程只读。
//!
//! 单元测试用的是几十字节的假文件，验证的是逻辑。这里补的是另一半：
//! 真实的 9MB PE、真实的 SHA-256、真实的版本解析。
//!
//! 用法：cargo run --example backup_drill

use std::path::Path;

use vivid_core::backup::{self, hash_file};
use vivid_core::component::DlssComponent;
use vivid_core::scan::{ScanReport, scan_tree};
use vivid_core::version::DllVersion;
use vivid_core::{BRAND_FULL, steam};

fn short(hash: &str) -> &str {
    &hash[..16]
}

fn describe(path: &Path) -> String {
    let v = DllVersion::from_pe_file(path)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "版本读不出".into());
    let h = hash_file(path)
        .map(|h| short(&h).to_string())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    format!("{v:<12} {:>8.2} MB  sha256:{h}…", size as f64 / 1_048_576.0)
}

fn main() {
    println!("\n{BRAND_FULL} · 备份还原演练（沙箱，不改动任何游戏）");
    println!("{}", "=".repeat(78));

    // ---- 从真机挑一对差距最大的 DLL ----
    let mut all = ScanReport::default();
    for d in steam::game_dirs() {
        all.merge(scan_tree(&d));
    }

    let kind = DlssComponent::SuperResolution;
    let Some(newest) = all.newest_source(kind) else {
        println!("\n本机没有可用的{}来源，演练跳过。", kind.display_name());
        return;
    };
    let Some(oldest) = all
        .found
        .iter()
        .filter(|f| f.kind == kind && f.is_usable_source())
        .min_by_key(|f| f.version)
    else {
        return;
    };

    if oldest.version >= newest.version {
        println!(
            "\n本机所有{}版本一致，没有可演练的升级。",
            kind.display_name()
        );
        return;
    }

    println!("\n挑中的一对（都来自你本机的正版游戏）：");
    println!("  旧：{}", oldest.path.display());
    println!("  新：{}", newest.path.display());

    // ---- 建沙箱，把两个文件复制进来 ----
    let sandbox = std::env::temp_dir().join(format!("vivid-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(sandbox.join("game")).unwrap();
    std::fs::create_dir_all(sandbox.join("source")).unwrap();

    let target = sandbox.join("game").join(kind.file_name());
    let source = sandbox.join("source").join(kind.file_name());
    std::fs::copy(&oldest.path, &target).expect("复制到沙箱失败");
    std::fs::copy(&newest.path, &source).expect("复制到沙箱失败");

    let original_hash = hash_file(&target).unwrap();

    println!("\n{}", "-".repeat(78));
    println!("① 初始状态");
    println!("   游戏内：{}", describe(&target));
    println!("   待装入：{}", describe(&source));

    // ---- 替换 ----
    let rec = match backup::install(&source, &target) {
        Ok(r) => r,
        Err(e) => {
            println!("\n替换失败：{e}");
            let _ = std::fs::remove_dir_all(&sandbox);
            return;
        }
    };

    println!("\n② 替换后");
    println!("   游戏内：{}", describe(&target));
    println!("   备份： {}", describe(&rec.backup_path));
    println!(
        "   记录： {} → {}   于 {}",
        rec.original_version
            .map(|v| v.to_string())
            .unwrap_or_default(),
        rec.installed_version
            .map(|v| v.to_string())
            .unwrap_or_default(),
        rec.created_at
    );

    // ---- 再装一次，验证备份不被刷掉 ----
    let rec2 = backup::install(&source, &target).expect("二次替换应当成功");
    let backup_still_original = hash_file(&rec2.backup_path).unwrap() == original_hash;
    println!("\n③ 再替换一次（检验备份是否被刷掉）");
    println!(
        "   备份仍是最初那份：{}",
        if backup_still_original {
            "是 ✅"
        } else {
            "否 ❌"
        }
    );

    // ---- 还原 ----
    match backup::restore(&target, Some(&rec.original_sha256)) {
        Ok(()) => {}
        Err(e) => {
            println!("\n还原失败：{e}");
            let _ = std::fs::remove_dir_all(&sandbox);
            return;
        }
    }

    println!("\n④ 还原后");
    println!("   游戏内：{}", describe(&target));

    // ---- 判据 ----
    let restored_hash = hash_file(&target).unwrap();
    let byte_identical = restored_hash == original_hash;
    let backup_gone = !backup::has_backup(&target);

    println!("\n{}", "=".repeat(78));
    println!("判据");
    println!(
        "  还原后与最初逐字节一致        {}",
        if byte_identical { "✅" } else { "❌" }
    );
    println!(
        "  二次替换未污染备份            {}",
        if backup_still_original { "✅" } else { "❌" }
    );
    println!(
        "  还原后未残留备份文件          {}",
        if backup_gone { "✅" } else { "❌" }
    );

    let _ = std::fs::remove_dir_all(&sandbox);
    println!("\n沙箱已清理：{}", sandbox.display());

    if !(byte_identical && backup_still_original && backup_gone) {
        std::process::exit(1);
    }
    println!();
}
