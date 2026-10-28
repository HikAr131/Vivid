# 銳境 Vivid

[繁體中文](README.md) · [English](README.en.md)

> 畫質常新，安心嘗試。

為中文玩家打造的顯示卡畫質管理工具，使用 Rust 與 egui 開發。

## 銳境 Vivid 已停止維護

自 2026-10-17 起，銳境 Vivid 不再更新；後續更新與問題回報請至 **Vivid-V2**。

**[下載 Vivid-V2](https://github.com/HikAr131/Vivid-V2-releases/releases/latest)**

- Vivid-V2 可辨識舊版銳境 Vivid 安裝過的遊戲元件，並將其還原；還原後再使用 Vivid-V2 安裝。
- 《Red Dead Redemption 2》這類安裝過全域 Vulkan 元件的遊戲，還原時需要系統管理員權限：請先下載舊版 [v0.1.3](https://github.com/HikAr131/Vivid/releases/tag/v0.1.3)（[直接下載 Vivid.zip](https://github.com/HikAr131/Vivid/releases/download/v0.1.3/Vivid.zip)），開啟後使用還原功能。
- 舊版下載仍保留於 [Releases](https://github.com/HikAr131/Vivid/releases)。

## 原始碼與建置

本儲存庫已公開 Vivid 的完整應用程式原始碼，採用 **GPL-3.0-only**，允許依授權條款使用、修改及重新散布，包括商業用途。
重新散布時，應依 GPL 保留適用的著作權、授權與合理署名聲明，修改版本須明確標示變更；專案名稱、作者與原始碼網址請見 [NOTICE](NOTICE)。
公開原始碼不代表恢復維護；第三方元件、字型與圖示仍適用各自的授權條款。

- [從原始碼建置與驗證](docs/BUILDING.md)：Windows x64、Rust 1.96.0+、MSVC / Windows SDK、Python 3。
- [核心邏輯](crates/vivid-core/) · [egui 介面](crates/vivid-gui/) · [授權條款](LICENSE) · [第三方聲明](THIRD_PARTY_NOTICES.md)。
- [原始碼快照說明](SOURCE.md)：原始碼範圍、版本與驗證結果。

## 免費公開分享

這個軟體免費提供給大家使用。若有建議或想回報問題，歡迎透過以下平台聯絡作者。

- 抖音：[Hikari](https://www.douyin.com/user/MS4wLjABAAAAE1GMl_LBdFUJywgN__Kv1vB9PcgKgwjZ0VbyBSFidfI)
- Bilibili：[Hikari](https://space.bilibili.com/3690980263856410)
- 小紅書：[Hikari](https://www.xiaohongshu.com/user/profile/67f9175a000000000a03e7b9)

程式的「關於與作者」頁面也提供上述連結，以及「請我喝杯咖啡」入口。
贊助完全自願，不會解鎖功能，也不換取任何服務。

## 免責聲明與授權

使用前，請閱讀程式「關於與作者」頁面中的完整免責聲明。
最後的「第三方元件」區段列出隨程式散布的相依套件、版本與授權條款，並可展開檢視條款全文。

Vivid 本體採用 **GPL-3.0-only**；執行期間下載的第三方元件適用各自的授權條款。
程式內嵌 Resource Han Rounded CN 字型，採用 SIL Open Font License 1.1；發行套件中的 `OFL.txt` 與 `FONT-LICENSE.md` 提供字型授權與相關說明。

本儲存庫提供原始碼、舊版下載與更新說明。DLSS 為 NVIDIA Corporation 的商標；Vivid 與 NVIDIA 並無隸屬關係，也不隨套件散布 NVIDIA、ReShade 或 RenoDX 的第三方執行階段元件。

平台圖示來源：抖音來自 Streamline Logos（CC BY 4.0），Bilibili 來自 Remix Icon 2.5.0（Apache-2.0，經由 SVG Repo 取得），小紅書來自 Simple Icons（CC0-1.0，經由 Streamline 取得）。[Free icons from Streamline](https://streamlinehq.com)；完整來源、修改說明與授權條款全文，請見程式免責聲明中的「第三方元件」區段。
