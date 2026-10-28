# 锐境 Vivid

> 画质常新，安心去试。

## 锐境 Vivid 已停止维护

2026-10-17 起，锐境 Vivid 不再更新，之后的更新和反馈只在 **Vivid-V2** 进行。

**下载 Vivid-V2**：https://github.com/HikAr131/Vivid-V2-releases/releases/latest

- Vivid-V2 能认出旧版锐境 Vivid 装过的游戏，并直接还原；还原之后再用 Vivid-V2 安装。
- 《荒野大镖客：救赎 2》这类装过全局 Vulkan 组件的游戏，还原需要管理员权限：请先下载旧版 [v0.1.3](https://github.com/HikAr131/Vivid/releases/tag/v0.1.3)（[直接下载 Vivid.zip](https://github.com/HikAr131/Vivid/releases/download/v0.1.3/Vivid.zip)），打开后点「一键还原」。
- 旧版历史下载仍在 [Releases](https://github.com/HikAr131/Vivid/releases)。

## 源码与构建

本仓库已公开 Vivid 的完整应用源码，采用 **GPL-3.0-only**，允许按许可证使用、修改和再分发，包括商业用途。
再分发时按 GPL 保留适用的版权、许可与合理署名声明，修改版须明确标记修改；项目名称、作者与源码网址见 [NOTICE](NOTICE)。
源码开放不代表恢复维护；第三方组件、字体与图标仍受各自许可约束。

- [从源码构建与验证](docs/BUILDING.md)：Windows x64、Rust 1.96.0+、MSVC / Windows SDK、Python 3。
- [核心逻辑](crates/vivid-core/) · [egui 介面](crates/vivid-gui/) · [LICENSE](LICENSE) · [第三方声明](THIRD_PARTY_NOTICES.md)。
- [源码快照说明](SOURCE.md)：源码范围、版本与验证结果。

## 免费公开分享

这个软件免费给大家使用。有好的建议或反馈，欢迎在下面这些平台告诉我。

- 抖音：[Hikari（三角洲行动）](https://www.douyin.com/user/MS4wLjABAAAAE1GMl_LBdFUJywgN__Kv1vB9PcgKgwjZ0VbyBSFidfI)
- B站：[Hikari三角洲行动](https://space.bilibili.com/3690980263856410)
- 小红书：[Hikari（三角洲行动）](https://www.xiaohongshu.com/user/profile/67f9175a000000000a03e7b9)

程序「关于与作者」页提供以上渠道及「请我喝杯咖啡」入口。
咖啡完全自愿，不解锁任何功能，也不换取任何服务。

## 免责声明与许可

使用前请阅读程序「关于与作者 → 免责声明」中的完整说明；
最后一节「第三方组件」列出随程序分发的依赖、版本和许可证，并可展开查看许可证全文。

Vivid 本体使用 **GPL-3.0-only**；运行时下载的第三方组件分别受各自许可约束。
程序内嵌 Resource Han Rounded CN（思源圆体），授权为 SIL Open Font License 1.1；
发行包中的 OFL.txt、FONT-LICENSE.md 提供字体许可与说明。

本仓库提供源码、历史下载与更新说明。DLSS 是 NVIDIA Corporation 的商标；
Vivid 与 NVIDIA 无隶属关系，不随包分发 NVIDIA、ReShade 或 RenoDX 的第三方运行时。

平台图标：抖音来自 Streamline Logos（CC BY 4.0），B站来自 Remix Icon 2.5.0（Apache-2.0，经 SVG Repo），小红书来自 Simple Icons（CC0-1.0，经 Streamline）。[Free icons from Streamline](https://streamlinehq.com)；完整来源、修改说明与许可全文见程序「免责声明 → 第三方组件」。
