//! 把图标与版本信息写进 exe 的 Windows 资源段。
//!
//! 运行时的窗口/任务栏图标是 `main.rs` 用 `IconData` 设的；
//! 这里管的是**另一件事**：资源管理器里 `Vivid.exe` 文件本身的图标，
//! 以及右键属性里的「详细信息」。两者互不替代。

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "锐境 Vivid");
        res.set("FileDescription", "锐境 Vivid —— 显卡画质一键管理器");
        res.set("LegalCopyright", "Copyright (C) 2026 Hikari. GPL-3.0-only.");
        // FileVersion / ProductVersion 由 winresource 自动取 CARGO_PKG_VERSION

        // **失败不中断构建。** 嵌资源要调 Windows SDK 的 rc.exe，
        // 没装 SDK 的机器上会失败——但那只影响 exe 文件的图标，程序功能一点不缺。
        // 为一个装饰性的东西让整个项目编译不过，是本末倒置。
        if let Err(e) = res.compile() {
            println!("cargo:warning=exe 图标未能嵌入（多半是缺 Windows SDK 的 rc.exe）：{e}");
            println!("cargo:warning=程序功能不受影响，只是资源管理器里显示默认图标。");
        }
    }
}
