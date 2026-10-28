# Vivid

[繁體中文](README.md) · [English](README.en.md)

> Keep your visuals fresh. Explore with confidence.

A graphics quality management tool for Chinese-speaking PC players, built with Rust and egui.

## Vivid is no longer maintained

Vivid has been discontinued since October 17, 2026. Future updates and issue reports are handled through **Vivid-V2**.

**[Download Vivid-V2](https://github.com/HikAr131/Vivid-V2-releases/releases/latest)**

- Vivid-V2 can detect components installed in games by the original Vivid and restore those games. Restore them before installing components with Vivid-V2.
- Restoring games with global Vulkan components, such as *Red Dead Redemption 2*, requires administrator privileges. Download the original [v0.1.3](https://github.com/HikAr131/Vivid/releases/tag/v0.1.3) ([direct Vivid.zip download](https://github.com/HikAr131/Vivid/releases/download/v0.1.3/Vivid.zip)), open it, and use its restore feature.
- Previous downloads remain available under [Releases](https://github.com/HikAr131/Vivid/releases).

## Source code and building

The complete Vivid application source is available in this repository under **GPL-3.0-only**, which permits use, modification, and redistribution, including commercial use, subject to its terms.
When redistributing, preserve applicable copyright, license, and reasonable attribution notices as required by the GPL, and clearly identify modifications. See [NOTICE](NOTICE) for the original project name, author, and source URL.
Publishing the source does not resume maintenance. Third-party components, fonts, and icons remain subject to their respective licenses.

- [Build and verification instructions](docs/BUILDING.md): Windows x64, Rust 1.96.0+, MSVC / Windows SDK, and Python 3.
- [Core logic](crates/vivid-core/) · [egui interface](crates/vivid-gui/) · [License](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md).
- [Source snapshot notes](SOURCE.md): source scope, version, and verification results.

## Free to use

This software is shared free of charge. Suggestions and issue reports are welcome through the author's accounts below.

- Douyin: [Hikari](https://www.douyin.com/user/MS4wLjABAAAAE1GMl_LBdFUJywgN__Kv1vB9PcgKgwjZ0VbyBSFidfI)
- Bilibili: [Hikari](https://space.bilibili.com/3690980263856410)
- Xiaohongshu: [Hikari](https://www.xiaohongshu.com/user/profile/67f9175a000000000a03e7b9)

The application's About & Author page also provides these links and a “Buy me a coffee” option.
Donations are entirely optional and do not unlock features or purchase any service.

## Disclaimer and licensing

Read the full disclaimer on the application's About & Author page before use.
Its final Third-party Components section lists bundled dependencies, versions, and licenses, with expandable full license texts.

Vivid itself is licensed under **GPL-3.0-only**. Third-party components downloaded at runtime remain subject to their own licenses.
The application embeds the Resource Han Rounded CN font under the SIL Open Font License 1.1. The distribution's `OFL.txt` and `FONT-LICENSE.md` contain the font license and related notices.

This repository provides source code, previous downloads, and update information. DLSS is a trademark of NVIDIA Corporation. Vivid is not affiliated with NVIDIA and does not bundle NVIDIA, ReShade, or RenoDX runtime components.

Social icons: Douyin is from Streamline Logos (CC BY 4.0); Bilibili is from Remix Icon 2.5.0 (Apache-2.0, obtained through SVG Repo); Xiaohongshu is from Simple Icons (CC0-1.0, obtained through Streamline). [Free icons from Streamline](https://streamlinehq.com). Complete sources, modification notices, and license texts are available in the disclaimer's Third-party Components section.
