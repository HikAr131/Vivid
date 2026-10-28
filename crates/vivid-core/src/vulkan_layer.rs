//! RDR2 的全局 Vulkan ReShade layer。
//!
//! 行为照搬 RHI 的 Vulkan 安装方式（见 `https://github.com/RankFTW/RHI`）：
//! ReShade 的 64 位 DLL 和 layer 清单放进 `C:\ProgramData\ReShade`，
//! 再往 `HKLM\SOFTWARE\Khronos\Vulkan\ImplicitLayers` 写一个值为 `0` 的 DWORD，
//! Vulkan loader 看到它就会给**所有** Vulkan 程序加载这一层。
//!
//! 两件事必须在界面上说清楚，代码里也用结构表达出来：
//!
//! 1. **影响范围是全局的**，不只是 RDR2。所以首次安装前要单独确认一次。
//! 2. **需要管理员权限的只有全局那三样**（两个文件 + 一个注册表值）。
//!    游戏目录里的 `reshade.ini` 普通权限就能写，不该被裹进提权范围——
//!    `elevated_actions` / `user_actions` 就是这条边界的机器表达。
//!
//! 本模块**不自建第二套备份/恢复逻辑**：所有原状态由 `deployment` 统一
//! baseline + ledger，还原时一并回退。这里只负责「要写什么、写到哪」。

use std::path::{Path, PathBuf};

use crate::artifact::{CachedArtifact, sha256_hex};
use crate::deployment::{
    ExpectedCurrent, ManagedTarget, PlannedAction, RegistryLocation, SystemBackend,
};
use crate::recipe::GraphicsApi;

/// 全局 layer 的固定目录。RHI 用的就是这个路径，注册表值名也要和它一致。
pub const LAYER_DIR: &str = r"C:\ProgramData\ReShade";
/// 全局 layer 的 ReShade 主体。
pub const LAYER_DLL: &str = "ReShade64.dll";
/// Vulkan loader 读的 layer 清单。
pub const LAYER_JSON: &str = "ReShade64.json";
/// implicit layer 的注册表位置。
pub const REGISTRY_SUBKEY: &str = r"SOFTWARE\Khronos\Vulkan\ImplicitLayers";
/// RDR2 游戏目录里的 ReShade 配置文件名。
pub const GAME_INI: &str = "reshade.ini";

/// RHI 的 RDR2 ReShade 配置模板。**文本模板，不是二进制**，可以随源码分发。
const RDR2_INI_TEMPLATE: &str = include_str!("../assets/reshade.rdr2.ini");
/// ReShade 的 Vulkan layer 清单模板。
const LAYER_JSON_TEMPLATE: &str = include_str!("../assets/ReShade64.json");

#[derive(Debug, thiserror::Error)]
pub enum VulkanError {
    #[error("缓存里的 ReShade 组件文件不见了，请重新下载：{0}")]
    MissingReShade(PathBuf),

    #[error("缓存里的 ReShade 组件校验不通过，可能没下完或被改过：{path}")]
    ReShadeHashMismatch { path: PathBuf },

    #[error("现有的 {0} 不是文本配置，Vivid 不会覆盖它")]
    NotText(String),

    #[error("准备 ReShade 配置失败：{0}")]
    Io(String),

    #[error("读取当前状态失败：{0}")]
    Probe(String),
}

/// 全局 layer 的 DLL 落点。
pub fn layer_dll_path() -> PathBuf {
    Path::new(LAYER_DIR).join(LAYER_DLL)
}

/// 全局 layer 的清单落点。它同时也是注册表值的名字。
pub fn layer_json_path() -> PathBuf {
    Path::new(LAYER_DIR).join(LAYER_JSON)
}

/// implicit layer 注册表位置。
pub fn registry_location() -> RegistryLocation {
    RegistryLocation::Hklm {
        subkey: REGISTRY_SUBKEY.to_string(),
    }
}

/// 全局 layer 会碰到的所有位置。
///
/// `deployment::TargetAllowlist::for_game` 对 RDR2 追加的就是这一组——
/// 白名单和实际写入用**同一份常量**，不会出现「能写但不在白名单」的缝。
pub fn global_layer_targets() -> Vec<ManagedTarget> {
    vec![
        ManagedTarget::File(layer_dll_path()),
        ManagedTarget::File(layer_json_path()),
        ManagedTarget::RegistryDword {
            location: registry_location(),
            value_name: layer_json_path().display().to_string(),
        },
    ]
}

/// Vivid 为了放全局 layer 自己建出来的目录。
///
/// 还原时这个目录会空掉。空壳留在 `C:\ProgramData` 下不好看，也让用户以为没删干净，
/// 所以还原收尾时把它收走——**只在空的时候**，里面万一有别人的东西就一律不动。
///
/// 只有 Vulkan 方案会建目录；其余注入方式把组件写进已经存在的游戏目录，什么都不建。
pub fn created_dirs(api: GraphicsApi) -> Vec<PathBuf> {
    match api {
        GraphicsApi::Vulkan => vec![PathBuf::from(LAYER_DIR)],
        _ => Vec::new(),
    }
}

/// 一个待写的注册表值，给界面展示与测试断言用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryTarget {
    pub location: RegistryLocation,
    pub value_name: String,
    pub value: u32,
}

/// RDR2 Vulkan layer 的安装动作集合。
#[derive(Debug, Clone)]
pub struct VulkanPlan {
    pub actions: Vec<PlannedAction>,
}

impl VulkanPlan {
    /// 按写入顺序列出文件目标。
    pub fn file_targets(&self) -> Vec<PathBuf> {
        self.actions
            .iter()
            .filter_map(|action| match action {
                PlannedAction::WriteFile { target, .. } => Some(target.clone()),
                PlannedAction::WriteRegistryDword { .. } => None,
            })
            .collect()
    }

    pub fn registry_values(&self) -> Vec<RegistryTarget> {
        self.actions
            .iter()
            .filter_map(|action| match action {
                PlannedAction::WriteRegistryDword {
                    location,
                    value_name,
                    value,
                    ..
                } => Some(RegistryTarget {
                    location: location.clone(),
                    value_name: value_name.clone(),
                    value: *value,
                }),
                PlannedAction::WriteFile { .. } => None,
            })
            .collect()
    }

    /// 需要管理员权限的部分：`C:\ProgramData\ReShade` 下的文件和 HKLM 值。
    pub fn elevated_actions(&self) -> Vec<PlannedAction> {
        self.actions
            .iter()
            .filter(|action| is_global(action))
            .cloned()
            .collect()
    }

    /// 普通权限就能写的部分：游戏目录里的配置。
    pub fn user_actions(&self) -> Vec<PlannedAction> {
        self.actions
            .iter()
            .filter(|action| !is_global(action))
            .cloned()
            .collect()
    }
}

/// 这一步是否落在全局（需要管理员）范围内。
fn is_global(action: &PlannedAction) -> bool {
    match action {
        PlannedAction::WriteRegistryDword { .. } => true,
        PlannedAction::WriteFile { target, .. } => target.starts_with(LAYER_DIR),
    }
}

/// 生成 RDR2 的 Vulkan layer 安装计划。
///
/// - `work_dir`：生成的文本先落到这里（Vivid 自己的缓存），再作为 `source` 进计划；
///   **不在写游戏/系统目录的那一步现算内容**。
/// - `backend`：只用来读当前状态（填 `ExpectedCurrent`）与读现有的游戏 ini。
///   测试传假实现，因此不会碰到真实 `C:\ProgramData` 与 HKLM。
/// - `ini_template`：这款游戏用哪份 ReShade 配置模板。RDR2 那份带它专有的
///   深度缓冲设置，**不能套到别的游戏上**，所以由调用方按游戏选。
pub fn build_vulkan_actions(
    game_root: &Path,
    reshade: &CachedArtifact,
    work_dir: &Path,
    backend: &dyn SystemBackend,
    ini_template: &str,
) -> Result<VulkanPlan, VulkanError> {
    // 缓存里的 ReShade 必须存在且哈希对得上——它要被写进系统目录，来路必须确定。
    let cached = std::fs::read(&reshade.path)
        .map_err(|_| VulkanError::MissingReShade(reshade.path.clone()))?;
    if sha256_hex(&cached) != reshade.sha256 {
        return Err(VulkanError::ReShadeHashMismatch {
            path: reshade.path.clone(),
        });
    }

    std::fs::create_dir_all(work_dir)
        .map_err(|e| VulkanError::Io(format!("建缓存目录失败：{e}")))?;

    // 现有的游戏 ini：有就合并，没有就用模板。
    let existing = read_target_file(backend, &game_root.join(GAME_INI))?;
    let existing_text = match &existing {
        None => None,
        Some(bytes) => Some(
            std::str::from_utf8(bytes).map_err(|_| VulkanError::NotText(GAME_INI.to_string()))?,
        ),
    };
    let merged_ini = merge_ini(existing_text, ini_template)?;

    let layer_json_source = stage(work_dir, LAYER_JSON, LAYER_JSON_TEMPLATE.as_bytes())?;
    let game_ini_source = stage(work_dir, GAME_INI, &merged_ini)?;

    let mut actions = Vec::new();
    actions.push(write_file_action(
        reshade.path.clone(),
        reshade.sha256.clone(),
        layer_dll_path(),
        backend,
    )?);
    actions.push(write_file_action(
        layer_json_source,
        sha256_hex(LAYER_JSON_TEMPLATE.as_bytes()),
        layer_json_path(),
        backend,
    )?);
    actions.push(write_file_action(
        game_ini_source,
        sha256_hex(&merged_ini),
        game_root.join(GAME_INI),
        backend,
    )?);

    // 注册表值最后写：先让 layer 清单落地，再让 loader 看见它。
    let location = registry_location();
    let value_name = layer_json_path().display().to_string();
    let current = backend
        .read_registry_dword(&location, &value_name)
        .map_err(|e| VulkanError::Probe(e.to_string()))?;
    actions.push(PlannedAction::WriteRegistryDword {
        location,
        value_name,
        value: 0,
        expected_current: match current {
            None => ExpectedCurrent::RegistryAbsent,
            Some(value) => ExpectedCurrent::RegistryDword(value),
        },
    });

    Ok(VulkanPlan { actions })
}

/// 把生成的内容落到缓存工作目录，返回来源路径。
fn stage(work_dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, VulkanError> {
    // 按内容哈希分目录：同样的内容永远落同一个文件，重新预演也不会把
    // 还没执行的旧计划的 source 改掉。
    let dir = work_dir.join(&sha256_hex(bytes)[..16]);
    std::fs::create_dir_all(&dir).map_err(|e| VulkanError::Io(format!("建缓存目录失败：{e}")))?;
    let path = dir.join(name);
    std::fs::write(&path, bytes)
        .map_err(|e| VulkanError::Io(format!("写 {} 失败：{e}", path.display())))?;
    Ok(path)
}

fn write_file_action(
    source: PathBuf,
    source_sha256: String,
    target: PathBuf,
    backend: &dyn SystemBackend,
) -> Result<PlannedAction, VulkanError> {
    let current = read_target_file(backend, &target)?;
    Ok(PlannedAction::WriteFile {
        source,
        source_sha256,
        expected_current: match current {
            None => ExpectedCurrent::Absent,
            Some(bytes) => ExpectedCurrent::FileSha256(sha256_hex(&bytes)),
        },
        target,
    })
}

fn read_target_file(
    backend: &dyn SystemBackend,
    path: &Path,
) -> Result<Option<Vec<u8>>, VulkanError> {
    backend
        .read_file(path)
        .map_err(|e| VulkanError::Probe(e.to_string()))
}

// ---------------------------------------------------------------------------
// ini 合并
// ---------------------------------------------------------------------------

/// RDR2 专用的 ReShade 配置模板（带这款游戏的深度缓冲设置）。
pub const fn rdr2_ini_template() -> &'static str {
    RDR2_INI_TEMPLATE
}

/// 把 RHI 的 RDR2 模板合并进现有配置。
///
/// 规则（设计文档 7.3「不覆盖无关键」）：
///
/// - 模板里有的 section/key：值以模板为准；
/// - 模板里没有的 section、key、注释、空行：原样保留，顺序不变；
/// - section 名和 key 名按**大小写不敏感**匹配，避免出现两个 `[GENERAL]`。
///
/// 输出用 CRLF：ReShade 自己写出来的 ini 就是 CRLF，保持一致省得每次启动被改写。
pub fn merge_rdr2_ini(existing: Option<&str>) -> Result<Vec<u8>, VulkanError> {
    merge_ini(existing, RDR2_INI_TEMPLATE)
}

/// 通用的 ReShade ini 合并。两款游戏用的是同一套规则，只是模板不同，
/// 所以合并逻辑放在这里由 manager 一起用，不各写一份。
pub fn merge_ini(existing: Option<&str>, template: &str) -> Result<Vec<u8>, VulkanError> {
    let mut doc = parse_ini(existing.unwrap_or_default());
    let template = parse_ini(template);

    for section in template.sections {
        let index = match doc
            .sections
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(&section.name))
        {
            Some(index) => index,
            None => {
                doc.sections.push(IniSection {
                    name: section.name.clone(),
                    lines: Vec::new(),
                });
                doc.sections.len() - 1
            }
        };
        let target = &mut doc.sections[index];
        for line in section.lines {
            let IniLine::Pair { key, value } = line else {
                continue; // 模板里的注释/空行不往用户配置里塞
            };
            let found = target.lines.iter().position(
                |existing| matches!(existing, IniLine::Pair { key: k, .. } if k.eq_ignore_ascii_case(&key)),
            );
            match found {
                Some(index) => {
                    if let IniLine::Pair { value: slot, .. } = &mut target.lines[index] {
                        *slot = value;
                    }
                }
                None => target.lines.push(IniLine::Pair { key, value }),
            }
        }
    }

    Ok(render_ini(&doc).into_bytes())
}

#[derive(Debug, Clone)]
enum IniLine {
    Pair {
        key: String,
        value: String,
    },
    /// 注释、空行，以及任何解析不成键值对的行——原样留着。
    Raw(String),
}

#[derive(Debug, Clone)]
struct IniSection {
    name: String,
    lines: Vec<IniLine>,
}

#[derive(Debug, Clone, Default)]
struct IniDoc {
    /// 第一个 section 之前的内容。
    preamble: Vec<IniLine>,
    sections: Vec<IniSection>,
}

fn parse_ini(text: &str) -> IniDoc {
    let mut doc = IniDoc::default();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
            doc.sections.push(IniSection {
                name: trimmed[1..trimmed.len() - 1].to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        let parsed = match trimmed.split_once('=') {
            Some((key, value)) if !trimmed.starts_with(';') && !trimmed.starts_with('#') => {
                IniLine::Pair {
                    key: key.trim().to_string(),
                    value: value.trim().to_string(),
                }
            }
            _ => IniLine::Raw(line.to_string()),
        };
        match doc.sections.last_mut() {
            Some(section) => section.lines.push(parsed),
            None => doc.preamble.push(parsed),
        }
    }
    doc
}

fn render_ini(doc: &IniDoc) -> String {
    let mut out = String::new();
    for line in &doc.preamble {
        push_line(&mut out, line);
    }
    for section in &doc.sections {
        out.push_str(&format!("[{}]\r\n", section.name));
        for line in &section.lines {
            push_line(&mut out, line);
        }
    }
    out
}

fn push_line(out: &mut String, line: &IniLine) {
    match line {
        IniLine::Pair { key, value } => out.push_str(&format!("{key}={value}\r\n")),
        IniLine::Raw(text) => {
            out.push_str(text);
            out.push_str("\r\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::ArtifactKind;
    use crate::deployment::TargetAllowlist;
    use crate::testing::FakeSystemBackend;

    /// 临时目录 + 虚拟 `C:\ProgramData` 映射：全程不碰真实 ProgramData 和 HKLM。
    struct LayerFixture {
        _temp: tempfile::TempDir,
        game_root: PathBuf,
        work_dir: PathBuf,
        reshade: CachedArtifact,
        backend: FakeSystemBackend,
    }

    impl LayerFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            let game_root = root.join("Red Dead Redemption 2");
            std::fs::create_dir_all(&game_root).unwrap();
            let work_dir = root.join("cache").join("generated");
            std::fs::create_dir_all(&work_dir).unwrap();

            let cache_dir = root.join("cache").join("reshade");
            std::fs::create_dir_all(&cache_dir).unwrap();
            let dll = cache_dir.join(LAYER_DLL);
            std::fs::write(&dll, b"MZ-reshade").unwrap();
            let reshade = CachedArtifact {
                kind: ArtifactKind::ReShade,
                version: "6.8.1".to_string(),
                source_url: "https://reshade.me/downloads/ReShade_Setup_6.8.1_Addon.exe"
                    .to_string(),
                final_url: "https://reshade.me/downloads/ReShade_Setup_6.8.1_Addon.exe".to_string(),
                file_name: LAYER_DLL.to_string(),
                sha256: sha256_hex(b"MZ-reshade"),
                size: 10,
                path: dll,
            };

            let mut backend = FakeSystemBackend::new(&root);
            // 计划里的目标仍然是真实的 C:\ProgramData\ReShade\...，
            // 只有夹具在读写时把它映射到临时目录。
            backend.map_programdata(Path::new(r"C:\ProgramData"), &root.join("programdata"));

            Self {
                game_root,
                work_dir,
                reshade,
                backend,
                _temp: temp,
            }
        }

        fn build(&self) -> Result<VulkanPlan, VulkanError> {
            build_vulkan_actions(
                &self.game_root,
                &self.reshade,
                &self.work_dir,
                &self.backend,
                RDR2_INI_TEMPLATE,
            )
        }
    }

    #[test]
    fn rdr2_vulkan_plan_contains_only_rhi_layer_targets() {
        let fixture = LayerFixture::new();
        let plan = fixture.build().unwrap();
        assert_eq!(
            plan.file_targets(),
            vec![
                PathBuf::from(r"C:\ProgramData\ReShade\ReShade64.dll"),
                PathBuf::from(r"C:\ProgramData\ReShade\ReShade64.json"),
                fixture.game_root.join("reshade.ini"),
            ]
        );
        assert_eq!(plan.registry_values().len(), 1);
        assert_eq!(
            plan.registry_values()[0].location,
            RegistryLocation::Hklm {
                subkey: REGISTRY_SUBKEY.to_string()
            }
        );
        assert_eq!(
            plan.registry_values()[0].value_name,
            r"C:\ProgramData\ReShade\ReShade64.json"
        );
        assert_eq!(plan.registry_values()[0].value, 0);
    }

    /// 需要管理员的只有全局 layer 那三样（两个文件 + 注册表值），
    /// 游戏目录里的 ini 用普通权限就能写——提权范围要尽可能小。
    #[test]
    fn only_the_global_layer_needs_admin() {
        let fixture = LayerFixture::new();
        let plan = fixture.build().unwrap();
        assert_eq!(plan.elevated_actions().len(), 3);
        assert_eq!(plan.user_actions().len(), 1);
        assert_eq!(
            plan.user_actions()[0].target(),
            ManagedTarget::File(fixture.game_root.join("reshade.ini"))
        );
    }

    /// 计划里每个目标都必须在 RDR2 配方白名单内——这是 deployment 那层的红线，
    /// 这里提前证明本模块生成的目标不会被那层拒掉。
    #[test]
    fn every_planned_target_is_inside_the_rdr2_allowlist() {
        let fixture = LayerFixture::new();
        let plan = fixture.build().unwrap();
        let allowlist = TargetAllowlist::for_verified_game(
            crate::recipe::GameId::RedDeadRedemption2,
            &fixture.game_root,
        );
        for action in &plan.actions {
            assert!(
                allowlist.permits(&action.target()),
                "{} 不在白名单里",
                action.target().describe()
            );
        }
    }

    /// 生成的两个文本先落到缓存工作目录，再作为 source 进计划。
    #[test]
    fn generated_text_lands_in_the_cache_before_the_plan_uses_it() {
        let fixture = LayerFixture::new();
        let plan = fixture.build().unwrap();
        for action in &plan.actions {
            let PlannedAction::WriteFile {
                source,
                source_sha256,
                ..
            } = action
            else {
                continue;
            };
            let bytes = std::fs::read(source).expect("来源文件必须已经落盘");
            assert_eq!(&sha256_hex(&bytes), source_sha256);
        }
        // layer 清单按内容哈希落在 work_dir 下的子目录里
        let json_source = plan
            .actions
            .iter()
            .find_map(|action| match action {
                PlannedAction::WriteFile { source, target, .. } if target.ends_with(LAYER_JSON) => {
                    Some(source.clone())
                }
                _ => None,
            })
            .expect("计划里必须有 layer 清单");
        assert!(json_source.starts_with(&fixture.work_dir));
        let layer_json = std::fs::read_to_string(json_source).unwrap();
        assert!(layer_json.contains("VK_LAYER_reshade"));
    }

    /// 缓存里的 ReShade 文件不在了 → 明确报错，不生成一个装不上的计划。
    #[test]
    fn missing_cached_reshade_is_reported() {
        let mut fixture = LayerFixture::new();
        std::fs::remove_file(&fixture.reshade.path).unwrap();
        fixture.reshade.sha256 = sha256_hex(b"MZ-reshade");
        let err = fixture.build().unwrap_err();
        assert!(matches!(err, VulkanError::MissingReShade(_)), "{err:?}");
    }

    /// 缓存文件被改过（哈希对不上）→ 拒绝，不把来路不明的文件写进系统目录。
    #[test]
    fn tampered_cached_reshade_is_rejected() {
        let mut fixture = LayerFixture::new();
        fixture.reshade.sha256 = "00".repeat(32);
        let err = fixture.build().unwrap_err();
        assert!(
            matches!(err, VulkanError::ReShadeHashMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn merge_writes_the_template_when_there_is_no_existing_ini() {
        let merged = String::from_utf8(merge_rdr2_ini(None).unwrap()).unwrap();
        assert!(merged.contains("[ADDON]"));
        assert!(merged.contains("DisabledAddons=Generic Depth,Effect Runtime Sync"));
        assert!(merged.contains("KeyOverlay=35,0,0,0"));
        assert!(merged.contains("\r\n"), "ReShade 的 ini 用 CRLF");
    }

    /// 模板键覆盖同名旧键；无关 section / key / 注释一律保留。
    #[test]
    fn merge_overrides_template_keys_and_keeps_everything_else() {
        let existing = "\
[GENERAL]
; 用户自己写的注释
PresetPath=.\\my-preset.ini
EffectSearchPaths=.\\old-shaders

[SOMETHING_ELSE]
Foo=Bar
";
        let merged = String::from_utf8(merge_rdr2_ini(Some(existing)).unwrap()).unwrap();

        // 模板键被覆盖
        assert!(merged.contains("EffectSearchPaths=.\\reshade-shaders\\Shaders\\**"));
        assert!(!merged.contains("EffectSearchPaths=.\\old-shaders"));
        // 无关键、无关 section、注释都还在
        assert!(merged.contains("PresetPath=.\\my-preset.ini"));
        assert!(merged.contains("[SOMETHING_ELSE]"));
        assert!(merged.contains("Foo=Bar"));
        assert!(merged.contains("; 用户自己写的注释"));
        // 模板里新增的 section 也在
        assert!(merged.contains("[ADDON]"));
        assert!(merged.contains("[OVERLAY]"));
        // 同一个键不能出现两次
        assert_eq!(merged.matches("EffectSearchPaths=").count(), 1);
        assert_eq!(merged.matches("[GENERAL]").count(), 1);
    }

    /// section 名大小写不同不能被当成两个 section。
    #[test]
    fn merge_matches_section_and_key_case_insensitively() {
        let existing = "[general]\r\neffectsearchpaths=.\\old\r\n";
        let merged = String::from_utf8(merge_rdr2_ini(Some(existing)).unwrap()).unwrap();
        assert_eq!(merged.to_lowercase().matches("[general]").count(), 1);
        assert_eq!(
            merged.to_lowercase().matches("effectsearchpaths=").count(),
            1
        );
        assert!(merged.contains(".\\reshade-shaders\\Shaders\\**"));
    }
}
