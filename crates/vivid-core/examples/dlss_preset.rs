//! 读驱动里的 DLSS preset 覆盖状态。**只读，不改任何东西。**
//!
//! ```text
//! cargo run -p vivid-core --example dlss_preset                # 读几个已知游戏
//! cargo run -p vivid-core --example dlss_preset -- RDR2.exe    # 读指定 exe
//! ```
//!
//! 这个工具的第一用途是**验证 FFI 是否正确**：
//! 结构体布局或 setting ID 只要错一点，读出来的值就会是垃圾。
//! 拿它去比对 NVIDIA App 界面上显示的内容，对得上才说明这套 FFI 是对的。
//! 单元测试只能证明结构体大小算得对，证明不了字段偏移对。

#[cfg(not(windows))]
fn main() {
    println!("这个工具只在 Windows 上有意义。");
}

#[cfg(windows)]
fn main() {
    use vivid_core::nvapi;

    println!();
    if !nvapi::is_available() {
        println!("连不上 NVIDIA 驱动接口——这台机器可能没有 N 卡，或驱动没装好。");
        return;
    }
    println!("NVIDIA 驱动接口可用。\n");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let targets: Vec<String> = if args.is_empty() {
        // 本机装着的几个，外加三角洲——那个你在 NVIDIA App 里设过，
        // 正好用来核对读出来的值对不对。
        [
            "RDR2.exe",
            "DeltaForceClient-Win64-Shipping.exe",
            "NarakaBladepoint.exe",
            "ModernWarfare.exe",
            "PlayGTAV.exe",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    } else {
        args
    };

    println!("{:<44} {:<10} 预设", "可执行文件", "覆盖开关");
    println!("{}", "-".repeat(78));

    for exe in &targets {
        match nvapi::read_override(exe) {
            Ok(st) => println!(
                "{:<44} {:<10} {}",
                exe,
                if st.enabled { "已开启" } else { "关闭" },
                st.preset.describe()
            ),
            Err(e) => println!("{:<44} —          {}", exe, first_line(&e.to_string())),
        }
    }

    println!();
    println!("核对方法：打开 NVIDIA App → 图形 → 程序设定，找同一个游戏，");
    println!("看「DLSS 覆盖 - 模型预设」显示的内容是否与上表一致。");
    println!("对得上，说明这套 FFI 的结构体布局与 setting ID 都是对的。");
    println!();
}

#[cfg(windows)]
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}
