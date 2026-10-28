//! 列出某个游戏在驱动里已设置的所有项。排查用。
#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    let exe = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "DeltaForceClient-Win64-Shipping.exe".into());
    println!("\n=== {exe} 在驱动里已设置的项 ===\n");
    match vivid_core::nvapi::dump_settings(&exe) {
        Ok((h, v)) if v.is_empty() => println!("  profile handle = 0x{h:X}，枚举成功但一项都没有"),
        Ok((h, v)) => {
            println!("  profile handle = 0x{h:X}");
            for (id, val) in &v {
                let note = match *id {
                    0x10E41E01 => "  ← DLSS-SR 覆盖开关",
                    0x10E41DF3 => "  ← DLSS-SR 预设",
                    0x10E41DF7 => "  ← DLSS-RR 预设",
                    0x10E41DF1 => "  ← DLSS-FG 预设",
                    0x00634291 => "  ← 强制模型预设配置",
                    _ => "",
                };
                println!("  0x{id:08X} = 0x{val:08X} ({val}){note}");
            }
            println!("\n  共 {} 项", v.len());
        }
        Err(e) => println!("  {}", e.to_string().lines().next().unwrap_or("")),
    }
    println!();
}
