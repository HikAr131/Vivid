//! 扫描器自检：在真机上跑一遍完整链路，人工核对结果。
//!
//! 用法：
//!   cargo run --example scan              仅扫 Steam 库
//!   cargo run --example scan -- <目录>    额外扫指定目录（用来验证红线拦截）

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use vivid_core::BRAND_FULL;
use vivid_core::component::DlssComponent;
use vivid_core::scan::{FoundComponent, ScanReport, scan_tree};
use vivid_core::steam;

/// 终端里中文占两列，用字符数做对齐会歪。这里粗略按码位区间估算显示宽度。
fn pad(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = if (c as u32) >= 0x1100 { 2 } else { 1 };
        if used + cw > w.saturating_sub(1) {
            out.push('…');
            used += 1;
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push_str(&" ".repeat(w.saturating_sub(used)));
    out
}

fn main() {
    println!("\n{BRAND_FULL} · 扫描自检");
    println!("{}", "=".repeat(78));

    // ---- 显卡与驱动：决定该对用户说什么话 ----
    let gpus = vivid_core::gpu::detect();
    if gpus.is_empty() {
        println!(
            "
没有检测到 NVIDIA 显卡。"
        );
    } else {
        for g in &gpus {
            println!(
                "
显卡：{}",
                g.summary()
            );
            if let Some(arch) = g.generation {
                let yn = |b: bool| if b { "支持" } else { "不支持" };
                println!(
                    "  transformer 超分：{}    帧生成：{}    RHI 上游兼容：{}",
                    yn(arch.supports_transformer()),
                    yn(arch.supports_frame_generation()),
                    yn(arch.supports_rhi_neural_rendering()),
                );
            }
        }
    }

    let dirs = steam::game_dirs();
    if dirs.is_empty() {
        println!("\n没有找到 Steam 游戏目录。是没装 Steam，还是库放在别处？");
    } else {
        println!("\n找到 {} 个 Steam 库：", dirs.len());
        for d in &dirs {
            println!("  · {}", d.display());
        }
    }

    let t = Instant::now();
    let mut all = ScanReport::default();
    let mut by_game: BTreeMap<String, Vec<FoundComponent>> = BTreeMap::new();

    for d in &dirs {
        let r = scan_tree(d);
        for f in &r.found {
            let name = steam::game_name_of(&f.path, d).unwrap_or_else(|| "（库根目录）".into());
            by_game.entry(name).or_default().push(f.clone());
        }
        all.merge(r);
    }

    // 命令行给的额外目录——用来在真实破解件上验证红线拦截。
    for extra in std::env::args().skip(1).map(PathBuf::from) {
        println!("\n额外扫描：{}", extra.display());
        all.merge(scan_tree(&extra));
    }

    let elapsed = t.elapsed();

    // ---- 本机各组件的最高可用版本：A+ 方案里「能升到哪」的答案 ----
    println!("\n本机可用的最新版本");
    println!("{}", "-".repeat(78));
    for kind in DlssComponent::ALL {
        match all.newest_source(kind) {
            Some(src) => {
                let from = src
                    .path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                println!(
                    "  {}{}来源：{}",
                    pad(kind.display_name(), 10),
                    pad(&src.version.unwrap().to_string(), 14),
                    from
                );
            }
            None => println!("  {}—— 本机没有可用版本", pad(kind.display_name(), 10)),
        }
    }

    // ---- 逐游戏编目 + 升级建议 ----
    println!("\n各游戏当前状况");
    println!("{}", "-".repeat(78));
    println!(
        "{}{}{}建议",
        pad("游戏", 34),
        pad("组件", 10),
        pad("当前版本", 14),
    );
    println!("{}", "-".repeat(78));

    let mut upgradable = 0;
    for (game, comps) in &by_game {
        let mut first = true;
        for c in comps {
            let cur = c
                .version
                .map(|v| v.to_string())
                .unwrap_or_else(|| "读不出".into());
            let advice = match (c.version, all.newest(c.kind)) {
                (Some(v), Some(best)) if v < best => {
                    upgradable += 1;
                    format!("↑ 可升到 {best}")
                }
                (Some(_), Some(_)) => "已是本机最新".to_string(),
                _ => "—".to_string(),
            };
            println!(
                "{}{}{}{}",
                pad(if first { game } else { "" }, 34),
                pad(c.kind.display_name(), 10),
                pad(&cur, 14),
                advice
            );
            first = false;
        }
    }

    // ---- 红线拦截：必须显式报出来 ----
    if !all.blocked.is_empty() {
        println!("\n已拦截（本工具不处理）");
        println!("{}", "-".repeat(78));
        let mut grouped: BTreeMap<&str, Vec<&PathBuf>> = BTreeMap::new();
        for b in &all.blocked {
            grouped.entry(b.reason).or_default().push(&b.path);
        }
        for (reason, paths) in grouped {
            println!("\n  共 {} 个文件：{}", paths.len(), reason);
            for p in paths.iter().take(3) {
                println!("    {}", p.display());
            }
            if paths.len() > 3 {
                println!("    …… 另有 {} 个", paths.len() - 3);
            }
        }
    }

    println!("\n{}", "=".repeat(78));
    println!(
        "扫描完成：{} 个游戏、{} 个组件、{} 个可升级、{} 个文件被拦截，用时 {:.1}s",
        by_game.len(),
        all.found.len(),
        upgradable,
        all.blocked.len(),
        elapsed.as_secs_f32()
    );
    println!();
}
