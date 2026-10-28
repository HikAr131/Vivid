# 源码快照说明

本仓库公开的是 **锐境 Vivid 的 Rust / egui 原始码**，不是 Vivid-V2。
Vivid 自 2026-10-17 起停止维护；后续产品更新与反馈见 [Vivid-V2](https://github.com/HikAr131/Vivid-V2-releases)。

## 版本与范围

- crate 版本保持为 `0.1.3`，源码包含停止维护后的更新渠道指向，以及本次 GPL 授权文案与构建说明整理。
- 原有 Releases、安装包与 `update.json` 不因本次开源而重新构建或替换。
- 公开内容包括两个 crate 的全部源码、沙箱测试与必要 fixture、Cargo 锁档、应用素材、许可证、素材开发工具和历史打包脚本。
- 内部会话规则、工作计划、含私人账号的验收原文与影片原稿不属于公开快照；它们不是构建应用或运行沙箱测试的前置条件。
- 测试夹具内的 ReShade 日志是历史行为样本，不代表当前上游版本或其他游戏的实测结果。

## 授权

Vivid 本体由 Hikari 以 **GPL-3.0-only** 开源，完整文本见 [LICENSE](LICENSE)。
该许可证允许商业使用与收费再分发，但要求遵守对应源码提供、保留声明等义务。
「作者免费公开分享」描述作者自己的发布方式，不限制下游依 GPL 获得的权利。
原作项目、作者与源码网址见 [NOTICE](NOTICE)；按 GPL 保留适用声明并标记修改，不另加非商业或强制广告条款。

Rust 依赖、字体、图标与配置模板保留各自的版权和许可证，见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
NVIDIA、ReShade、RenoDX 的运行时不在仓库或发行包中；程序在用户明确操作后从上游下载，不能视作本项目的 GPL 授权内容。

## 构建与验证

完整工具链、构建和测试命令见 [docs/BUILDING.md](docs/BUILDING.md)。
自动测试的安装、还原、系统与网络边界均使用沙箱，不能代替游戏画面、UAC 或其他电脑的实测。
