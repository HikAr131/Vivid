# 锐境 Vivid

> 画质常新，安心去试。

给中文 3A 玩家的 DLSS 画质增强一键管理器。选游戏 → 查看支持情况 → 安装组件；出问题可一键还原。

## 下载与使用

下载 [Vivid.zip](https://github.com/HikAr131/Vivid/releases/latest/download/Vivid.zip)，解压后启动 **Vivid.exe**，不需要安装。
历史版本与更新说明见 [Releases](https://github.com/HikAr131/Vivid/releases)。

适用于 Windows 10 / 11。程序会检测 NVIDIA 显卡和驱动，并显示 RHI 上游兼容声明；
上游声明不等于 NVIDIA 官方支持，也不代表 Vivid 已验证你的设备和游戏。

1. 从扫描到的 Steam 游戏中选择一款，阅读支持情况与风险说明。
2. 完全退出游戏，再点击安装。需要的组件会从上游下载，原文件自动备份。
3. 进入单机游戏自行确认效果；需要恢复时，退出游戏后使用「一键还原」。

## 它能做什么

- 扫描本机 Steam 游戏库，显示可安装、未实测或不提供安装的原因；不支持 Epic、Rockstar 独立版等其他启动器。
- 按 [RHI](https://github.com/RankFTW/RHI) 在线清单管理四个组件：ReShade、RenoDX DLSS5 addon、DLSS-NR、DLSS 超分。
- 提供安装、更新、修复、状态检测与还原。更新需要你主动确认，不会自动替换游戏文件。
- 首次接管文件时保存原始备份，后续更新不覆盖；检测到其他工具改动文件时，提示冲突，不直接覆盖。

Vivid 使用 Rust + egui。第三方运行时不随 Vivid 打包，不由作者托管或镜像；
只有你明确安装或更新时，才按 RHI 清单及上游发布地址下载到本机缓存。
组件来源、版本、下载地址与 SHA-256 可在游戏详情中查看。本地 SHA-256 是文件指纹，不是上游签名验证。

## 哪些情况不提供安装

**判不出图形 API，或检测到反作弊且没有已验证的单机入口，就不提供安装。**
这些游戏仍会列出并说明原因。反作弊检测并不完整，未检测到不代表没有反作弊，请仅用于单机。

GTA5 Enhanced 与 RDR2 有专用方案；其余符合条件的游戏标为「未实测」，
不保证画面效果或兼容性，不承诺安装后一定能正常运行。

- **GTA5 Enhanced 只提供单机入口**：通过 Steam 使用一次性 -nobattleye 参数启动，不修改永久启动选项，不提供 GTA Online 入口。
- **RDR2 为实验支持**：使用全局 Vulkan ReShade layer，会改动 C:\ProgramData\ReShade 与系统注册表，影响这台电脑上所有 Vulkan 游戏。安装前需单独确认，相关操作可能请求管理员权限。不要在游戏设为 DX12 时套用 Vulkan 方案。

不要带着修改后的文件进入联机模式。第三方组件可能违反游戏用户协议，游戏方可能检测并处罚。

## 文件装上了，不代表画面有效

2026-08-31 对 RenoDX DLSS5 Generic v4.6 的实测边界是：**DLSS 5 神经渲染只在 D3D12 路径工作**。
RDR2 的 Vulkan 路径虽能载入 ReShade 和插件，但 NR 未被调用；非 D3D12 游戏不能据此宣称 NR 生效。
这是当时版本的实测结果，不代表已验证当前上游的所有版本。

程序区分文件已安装、检测到组件载入或运行与画面效果待实测。
**文件复制成功、哈希一致或日志显示载入，都不能证明画面改善；效果需要你进游戏确认。**

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

本仓库用于提供下载与更新说明。DLSS 是 NVIDIA Corporation 的商标；
Vivid 与 NVIDIA 无隶属关系，不随包分发 NVIDIA、ReShade 或 RenoDX 的第三方运行时。

平台图标：抖音来自 Streamline Logos（CC BY 4.0），B站来自 Remix Icon 2.5.0（Apache-2.0，经 SVG Repo），小红书来自 Simple Icons（CC0-1.0，经 Streamline）。[Free icons from Streamline](https://streamlinehq.com)；完整来源、修改说明与许可全文见程序「免责声明 → 第三方组件」。
