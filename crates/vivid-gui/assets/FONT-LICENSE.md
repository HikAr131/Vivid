# 内嵌字体的授权

本软件把下面这份字体**内嵌进了可执行文件**，随发行包一并分发。
**这份文件必须随发行包一起分发**——这是 OFL 的义务，不是可选项。

---

## Resource Han Rounded CN Medium（思源圆体）

| 项 | 值 |
|---|---|
| 文件 | `crates/vivid-gui/assets/font.ttf` |
| 版本 | 0.990 |
| 授权 | **SIL Open Font License, Version 1.1** |
| 授权全文 | <http://scripts.sil.org/OFL> |
| 项目主页 | <https://github.com/CyanoHao/Resource-Han-Rounded> |

### 版权声明（取自字体文件的 name table，原文照录）

> Copyright © 2018—2019 Cyano Hao.
> Portions © 2014, 2015, 2018 Adobe (http://www.adobe.com/).

### 授权声明（原文照录）

> This Font Software is licensed under the SIL Open Font License, Version 1.1.
> This Font Software is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
> CONDITIONS OF ANY KIND, either express or implied. See the SIL Open Font
> License for the specific language, permissions and limitations governing your
> use of this Font Software.

### OFL 对本项目的约束

- ✅ 允许内嵌进软件并随之分发，**含商业用途**
- ✅ 允许修改，但衍生字体必须同样以 OFL 发布
- ❌ 不得单独售卖字体本身
- ❌ 不得用保留字体名（Reserved Font Name）命名衍生版本
- ⚠️ **必须**随分发保留上述版权与授权声明

### ✅ 完整许可证副本（已附）

同目录下的 **`OFL.txt`** 是 SIL Open Font License 1.1 的完整文本，
取自官方 <https://openfontlicense.org/documents/OFL.txt>，**许可证正文逐字未改**。

开头的版权行按 SIL 模板本身的用法，填成了本字体 name table 里的真实版权声明
（官方模板那里是 `<Copyright Holder>` 一类的占位符，本就是留给字体作者填的）。

⚠️ **发行时必须把 `OFL.txt` 与本文件一并放进发行包**，这是 OFL 的分发义务。

---

## 为什么内嵌这个、却不内嵌微软雅黑

区别不在于「内不内嵌」，而在于**授权允不允许再分发**：

| 字体 | 能否内嵌进本软件分发 | 原因 |
|---|---|---|
| 思源圆体 / Noto Sans SC | ✅ | SIL OFL 1.1 明确允许 |
| 微软雅黑、黑体、等线 | ❌ | 微软商业字体，授权不含再分发 |
| 苹方 PingFang SC | ❌ | Apple 字体，授权限于 Apple 设备 |

这跟 `CLAUDE.md` 红线 1（不分发 NVIDIA 的闭源 DLL）是同一类问题，
只是对象从显卡驱动组件换成了字体文件。

### 那系统字体怎么用的

`main.rs` 的 `install_chinese_font` 会在内嵌字体之后**追加**系统字体作为缺字兜底。
那是在**用户自己的机器上读取用户自己已有的字体**，属于「引用」而非「分发」，
不涉及再分发授权——就像网页 CSS 里写 `font-family: "PingFang SC"` 一样。

发行包里**不包含**任何系统字体文件。
