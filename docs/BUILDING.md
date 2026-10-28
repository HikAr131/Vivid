# 从源码构建 Vivid

Vivid 是已停止维护的 Windows Rust / egui 桌面工具。本仓库公开原始码供学习、修改与再分发，后续产品更新和反馈在 [Vivid-V2](https://github.com/HikAr131/Vivid-V2-releases) 进行。

## 环境

- Windows 10 / 11，x86-64。
- Rust 1.96.0 或更新版本，使用 `x86_64-pc-windows-msvc` 工具链。
- Visual Studio 2022 Build Tools 的「使用 C++ 的桌面开发」工作负载与 Windows SDK。
- Python 3，用于检查随源码提交的第三方许可证清单。
- Git。初次获取 Rust crate 需要联网；不需要安装 Node.js 来编译应用。

建议通过 rustup 安装 Rust，并在 Visual Studio 的 x64 Native Tools PowerShell 或已配置好 MSVC 的 PowerShell 中操作。

```powershell
git clone https://github.com/HikAr131/Vivid.git
Set-Location Vivid
rustup toolchain install 1.96.0 --profile minimal --component rustfmt --component clippy
rustup override set 1.96.0
cargo fetch --locked
python scripts/generate-licenses.py --check
cargo build --locked -p vivid-gui
```

默认输出为 `target/debug/Vivid.exe`。需要优化构建时运行 `cargo build --locked --release -p vivid-gui`，输出为 `target/release/Vivid.exe`。
`cargo run --locked -p vivid-gui` 会启动真实应用，请只在理解其游戏文件与全局 Vulkan 改动范围后使用安装功能。

可通过当前终端的 `CARGO_TARGET_DIR` 将构建输出放到其他磁盘；不需要特定盘符。许可证脚本会沿用该变量，未设置时使用仓库自己的 `target/`。

## 验证

```powershell
cargo fmt --all --check
python scripts/generate-licenses.py --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

自动测试使用临时目录与假网络、假系统边界，不会安装到真实游戏目录，也不会修改真实 ProgramData 或 HKLM。
这些检查证明源码与沙箱逻辑可构建、可测试，不能代替游戏画面、UAC 或其他电脑的实测。

许可证脚本使用 `cargo metadata --locked --offline` 和 `cargo tree --locked --offline`，因此先执行 `cargo fetch --locked` 获取锁定的 crate 源码。
修改依赖或素材后，运行 `python scripts/generate-licenses.py` 重建清单，再运行 `--check`；保留所有原始版权、许可证与署名。

## 项目结构

- `crates/vivid-core/`：Steam 发现、图形 API 与反作弊判定、上游清单、备份、安装、还原与测试。
- `crates/vivid-gui/`：egui 介面、应用图标、内嵌字体、素材与第三方许可证清单。
- `scripts/`：许可证清单生成与素材开发工具；图标重建依赖和命令见 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。
- `release/package.ps1`：历史打包脚本；分发构建时必须随附脚本列出的 GPL、字体与第三方许可文件。
- [ui-style-guide.md](ui-style-guide.md)：既有介面规范。

## 授权与第三方边界

Vivid 本体为 [GPL-3.0-only](../LICENSE)，允许商业使用与收费再分发，但必须履行许可证规定的对应源码等义务。
原作信息见 [NOTICE](../NOTICE)；再分发时保留适用的版权、许可与合理署名声明，并明确标记修改。
「作者免费公开分享」描述作者自己的发布方式，不是额外的非商业限制。

字体、平台图标、Rust 依赖与配置模板保留各自的许可，完整来源见 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。
仓库不包含 NVIDIA、ReShade、RenoDX 的第三方运行时；只有用户明确安装或更新时，程序才按 RHI 在线清单从上游下载。
不要把第三方 DLL、addon、PTX、压缩包或下载缓存加入源码或发行包。
