//! 只读列出本机 Steam 库里的每一款游戏和 Vivid 对它的裁决。
//!
//! **这个例子一个字节都不写**：不下载、不碰游戏目录、不碰注册表。
//! 它是真机验收「Step 1 只读检查」用的工具——确认 CS2 / PUBG / Apex 这类
//! 确实没有可点的安装按钮。
//!
//! ```text
//! cargo run -p vivid-core --example survey
//! ```

use vivid_core::catalog::{self, Eligibility};
use vivid_core::paths::AppPaths;
use vivid_core::rhi_manifest;

fn main() {
    let snapshot = AppPaths::detect()
        .ok()
        .and_then(|paths| rhi_manifest::load_cached(&paths.manifest_cache()));
    println!(
        "RHI 清单缓存：{}",
        if snapshot.is_some() {
            "有（用它判图形 API）"
        } else {
            "无（只能靠本机检测）"
        }
    );

    let games = catalog::survey_local(snapshot.as_ref());
    if games.is_empty() {
        println!("没有发现任何已安装的 Steam 游戏。");
        return;
    }

    let mut installable = 0usize;
    for game in &games {
        let verdict = match &game.eligibility {
            Eligibility::Installable(profile) => {
                installable += 1;
                format!(
                    "可安装（{}，{}，{}）",
                    profile.api.label(),
                    game.api_source.label(),
                    match profile.verification {
                        vivid_core::recipe::Verification::Verified => "已验证",
                        vivid_core::recipe::Verification::Untested => "未实测",
                    }
                )
            }
            Eligibility::BlockedByAntiCheat(found) => {
                format!("不提供安装（{}）", found.kind().unwrap_or("反作弊"))
            }
            Eligibility::UnknownApi => "不提供安装（判不出图形 API）".to_string(),
            Eligibility::NotAGame => "不提供安装（目录里没有可执行文件）".to_string(),
        };
        println!("{:<44} {:>8}  {}", game.name(), game.app_id(), verdict);
        if let Some(reason) = game.blocked_reason() {
            println!("{:>44}  └ {reason}", "");
        }
    }
    println!(
        "\n共 {} 款，其中 {} 款可安装、{} 款不提供安装。",
        games.len(),
        installable,
        games.len() - installable
    );
    println!("{}", vivid_core::anticheat::CANNOT_BE_SURE);
}
