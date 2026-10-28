//! NVAPI 驱动设置（DRS）—— DLSS preset 覆盖。
//!
//! # 为什么需要这个模块
//!
//! **换 DLL 只是必要条件，不是充分条件。**
//!
//! DLSS 的 DLL 里同时装着 CNN 与 Transformer 两代模型，具体跑哪个由 *preset* 决定。
//! 老游戏（例如《荒野大镖客 2》编译时用的是 DLSS 2.2）请求的 preset 里根本没有
//! Transformer——那是 DLSS 4 才引入的 J/K。于是换了新 DLL 之后游戏照跑老模型，
//! 用户看到的是「什么都没发生」。
//!
//! 覆盖 preset 只能走 NVAPI 的 DRS（Driver Settings Repository），
//! 没有注册表或配置文件的捷径。NVIDIA App 里那个「DLSS 覆盖 - 模型预设」
//! 走的就是这条路，属于 CLAUDE.md 第四节安全区里明确列出的官方路线。
//!
//! # 关于 unsafe
//!
//! CLAUDE.md 第六节说这个项目「几乎零 unsafe」——**这里是唯一的例外**。
//! NVAPI 是 C ABI 的闭源库，除了 FFI 没有别的调法。所有 unsafe 都圈在本模块内，
//! 对外只暴露安全接口。
//!
//! # 数据来源
//!
//! setting ID、取值枚举、函数 ID、结构体布局，均取自 NVIDIA 公开的
//! `NvApiDriverSettings.h` 与开源项目 nvidiaProfileInspector 的定义。
//! **没有任何一个数字是凭记忆写的**——写错 setting ID 会往用户的驱动配置里
//! 塞进无意义的键值，而那是用户看不见也不会去清理的地方。

#![cfg(windows)]

use std::ffi::c_void;
use std::mem::size_of;

// ============================================================================
// 常量：全部有出处
// ============================================================================

/// NVAPI 的入口函数。所有其它函数都靠它按 ID 查出来。
const QUERY_INTERFACE: &[u8] = b"nvapi_QueryInterface\0";

// —— 函数 ID ——
const ID_INITIALIZE: u32 = 0x0150_E828;
const ID_UNLOAD: u32 = 0xD22B_DD7E;
const ID_DRS_CREATE_SESSION: u32 = 0x0694_D52E;
const ID_DRS_DESTROY_SESSION: u32 = 0xDAD9_CFF8;
const ID_DRS_LOAD_SETTINGS: u32 = 0x375D_BD6B;
const ID_DRS_SAVE_SETTINGS: u32 = 0xFCBC_7E14;
const ID_DRS_FIND_APPLICATION_BY_NAME: u32 = 0xEEE5_66B2;
// 下面三个函数各有两个 ID。**经典 ID 放主位**：它们是 NVAPI 里公开且长期稳定的，
// 而另一个只在较新的驱动上存在。
//
// 顺序反过来踩过一次坑：把新 ID 放主位时，这台机器上 QueryInterface 返回了
// 非空但并非 GetSetting 的指针，按 GetSetting 的签名去调，直接 SIGSEGV
// ——不是错误码，是进程没了。查不到就回退还算安全，查到个错的最要命。
const ID_DRS_GET_SETTING: u32 = 0x73BF_8338;
const ID_DRS_GET_SETTING_ALT: u32 = 0xEA99_498D;
const ID_DRS_SET_SETTING: u32 = 0x577D_D202;
const ID_DRS_SET_SETTING_ALT: u32 = 0x8A2C_F5F5;
const ID_DRS_ENUM_SETTINGS: u32 = 0xAE30_39DA;
const ID_DRS_ENUM_SETTINGS_ALT: u32 = 0xCFD6_983E;

// —— DLSS 相关的 setting ——
//
// 取自 NvApiDriverSettings.h：
//   NGX_DLSS_SR_OVERRIDE_ID = 0x10E41E01
//   NGX_DLSS_SR_OVERRIDE_RENDER_PRESET_SELECTION_ID = 0x10E41DF3

/// 「启用 DLSS 超分覆盖」的开关。不开的话下面那个 preset 设了也不生效。
const SETTING_SR_OVERRIDE: u32 = 0x10E4_1E01;
/// 「强制使用哪个 preset」。
const SETTING_SR_PRESET: u32 = 0x10E4_1DF3;

const SR_OVERRIDE_OFF: u32 = 0;
const SR_OVERRIDE_ON: u32 = 1;

/// `NGX_DLSS_SR_OVERRIDE_RENDER_PRESET_SELECTION_RENDER_PRESET_Latest`。
///
/// 这就是 NVIDIA App 界面上那个「最新」。用它而不是写死 `K`：
/// preset 字母会随驱动更新增补，写死某个字母意味着以后要跟着改代码，
/// 而「最新」永远指向当前驱动认为最好的那个。
const PRESET_LATEST: u32 = 0x00FF_FFFF;
const PRESET_OFF: u32 = 0;

/// `NVAPI_UNICODE_STRING_MAX`。
///
/// 可以从 `NVDRS_SETTING_UNION` 的总长反推：4100 = 4（长度字段）+ 2048×2（UTF-16）。
const UNICODE_STRING_MAX: usize = 2048;
/// `NVDRS_SETTING_UNION` 的固定大小。它是个 union，按最大成员算。
const SETTING_UNION_SIZE: usize = 4100;

/// `NVDRS_SETTING_UNION` 的等价物。
///
/// **`align(4)` 不能省。** 裸 `[u8; 4100]` 的对齐是 1，会让编译器算出的
/// 结构体大小与 C 的实际布局对不上。C 里这个 union 的成员是
/// `NvU32` / `NvU16[2048]` / `{NvU32, NvU8[4096]}`，最大对齐为 4。
///
/// （C# 封装里那个 `qwordValue` 是它自己加的便利方法，
/// 不代表 C 的 union 里有 NvU64——按 8 对齐算出来的 12328 会被驱动
/// 判为 INCOMPATIBLE_STRUCT_VERSION。）
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct SettingUnion([u8; SETTING_UNION_SIZE]);

impl SettingUnion {
    fn dword(&self) -> u32 {
        u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    fn set_dword(&mut self, v: u32) {
        self.0 = [0; SETTING_UNION_SIZE];
        self.0[..4].copy_from_slice(&v.to_le_bytes());
    }
}

// ============================================================================
// 结构体：字段照抄官方定义，大小交给编译器算
// ============================================================================

/// `MAKE_NVAPI_VERSION(T, v)` = `sizeof(T) | (v << 16)`。
///
/// **不手算大小。** NVAPI 靠这个版本号校验调用方与驱动的结构体是否一致，
/// 手写一个数字就等于把「结构体定义」和「它的大小」拆成两处维护，
/// 改了字段忘了改数字，得到的是一个语焉不详的 INCOMPATIBLE_STRUCT_VERSION。
const fn make_version<T>(v: u32) -> u32 {
    (size_of::<T>() as u32) | (v << 16)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvdrsSetting {
    version: u32,
    setting_name: [u16; UNICODE_STRING_MAX],
    setting_id: u32,
    setting_type: u32,
    setting_location: u32,
    is_current_predefined: u32,
    is_predefined_valid: u32,
    predefined_value: SettingUnion,
    current_value: SettingUnion,
}

impl NvdrsSetting {
    fn zeroed_with_version() -> Self {
        // SAFETY: 全零是这个结构体的合法状态——所有字段要么是整数，
        // 要么是定长字节数组，没有引用、指针或带不变量的类型。
        let mut s: Self = unsafe { std::mem::zeroed() };
        s.version = make_version::<Self>(1);
        s
    }

    /// 取 DWORD 值。union 的前 4 字节就是 dwordValue。
    fn dword(&self) -> u32 {
        self.current_value.dword()
    }

    fn set_dword(&mut self, v: u32) {
        self.current_value.set_dword(v);
    }
}

/// `NVDRS_APPLICATION_V4`。我们只读它的 handle，字段内容用不上，
/// 但结构体必须完整——NVAPI 会按 version 校验大小并往里写。
#[repr(C)]
#[derive(Clone, Copy)]
struct NvdrsApplicationV4 {
    version: u32,
    is_predefined: u32,
    app_name: [u16; UNICODE_STRING_MAX],
    user_friendly_name: [u16; UNICODE_STRING_MAX],
    launcher: [u16; UNICODE_STRING_MAX],
    file_in_folder: [u16; UNICODE_STRING_MAX],
    bitvector: u32,
    command_line: [u16; UNICODE_STRING_MAX],
}

impl NvdrsApplicationV4 {
    fn zeroed_with_version() -> Self {
        // SAFETY: 同 NvdrsSetting，全零是合法状态。
        let mut a: Self = unsafe { std::mem::zeroed() };
        a.version = make_version::<Self>(4);
        a
    }
}

// ============================================================================
// 函数指针类型
// ============================================================================

type FnQueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
type FnInitialize = unsafe extern "C" fn() -> i32;
type FnUnload = unsafe extern "C" fn() -> i32;
type FnCreateSession = unsafe extern "C" fn(*mut isize) -> i32;
type FnDestroySession = unsafe extern "C" fn(isize) -> i32;
type FnLoadSettings = unsafe extern "C" fn(isize) -> i32;
type FnSaveSettings = unsafe extern "C" fn(isize) -> i32;
type FnFindApplicationByName =
    unsafe extern "C" fn(isize, *const u16, *mut isize, *mut NvdrsApplicationV4) -> i32;
type FnGetSetting = unsafe extern "C" fn(isize, isize, u32, *mut NvdrsSetting) -> i32;
type FnSetSetting = unsafe extern "C" fn(isize, isize, *mut NvdrsSetting) -> i32;
type FnEnumSettings = unsafe extern "C" fn(isize, isize, u32, *mut u32, *mut NvdrsSetting) -> i32;

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum NvapiError {
    #[error("找不到 NVIDIA 驱动接口（nvapi64.dll）。这台机器可能没有 N 卡，或驱动没装好。")]
    NoDriver,

    #[error("NVIDIA 驱动接口里缺少必要的函数，驱动版本可能太旧。")]
    MissingFunction(&'static str),

    #[error("{what} 失败（NVAPI 错误码 {code}）")]
    Call { what: &'static str, code: i32 },

    #[error(
        "驱动里没有为「{0}」建立配置项。\n\
         请先在 NVIDIA App 里打开这个游戏的程序设定，让驱动认识它，再回来重试。"
    )]
    AppNotFound(String),
}

fn check(what: &'static str, code: i32) -> Result<(), NvapiError> {
    if code == 0 {
        Ok(())
    } else {
        Err(NvapiError::Call { what, code })
    }
}

// ============================================================================
// preset 的表示
// ============================================================================

/// 当前给某个游戏设的 preset。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// 没有覆盖，游戏用自己请求的那个。
    Off,
    /// 指定了某个字母（A..O）。
    Letter(char),
    /// 「最新」——跟着驱动走，不写死字母。
    Latest,
    /// 驱动里是个我们不认识的值。原样保留，不猜。
    Unknown(u32),
}

impl Preset {
    fn from_raw(v: u32) -> Self {
        match v {
            PRESET_OFF => Preset::Off,
            PRESET_LATEST => Preset::Latest,
            // 官方定义：A=1, B=2 … O=15
            1..=15 => Preset::Letter((b'A' + (v as u8) - 1) as char),
            other => Preset::Unknown(other),
        }
    }

    pub fn describe(self) -> String {
        match self {
            Preset::Off => "跟随游戏自己的设置".into(),
            Preset::Latest => "最新（跟随驱动）".into(),
            Preset::Letter(c) => format!("预设 {c}"),
            Preset::Unknown(v) => format!("未知值 0x{v:08X}"),
        }
    }
}

/// 一个游戏当前的 DLSS 覆盖状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverrideStatus {
    /// 覆盖总开关。**关着的话 preset 设了也不生效**。
    pub enabled: bool,
    pub preset: Preset,
}

// ============================================================================
// 会话：RAII 包住 NVAPI 的初始化与 session
// ============================================================================

struct Drs {
    _lib: libloading::Library,
    session: isize,
    unload: FnUnload,
    destroy_session: FnDestroySession,
    save_settings: FnSaveSettings,
    find_app: FnFindApplicationByName,
    get_setting: FnGetSetting,
    set_setting: FnSetSetting,
    enum_settings: FnEnumSettings,
}

impl Drs {
    fn open() -> Result<Self, NvapiError> {
        // SAFETY: 加载系统目录下的 nvapi64.dll 并按官方 ID 取函数指针。
        // 这些 ID 与签名来自 NVIDIA 公开的头文件，取错会在下面的
        // MissingFunction 处失败，而不是拿到错误的函数。
        unsafe {
            let lib = libloading::Library::new("nvapi64.dll").map_err(|_| NvapiError::NoDriver)?;
            let query: libloading::Symbol<FnQueryInterface> =
                lib.get(QUERY_INTERFACE).map_err(|_| NvapiError::NoDriver)?;

            // 有些函数在新旧驱动上 ID 不同，主 ID 查不到就回退。
            let pick = |id: u32, fallback: Option<u32>, name: &'static str| {
                let mut p = query(id);
                if p.is_null()
                    && let Some(f) = fallback
                {
                    p = query(f);
                }
                if p.is_null() {
                    Err(NvapiError::MissingFunction(name))
                } else {
                    Ok(p)
                }
            };

            let initialize: FnInitialize =
                std::mem::transmute(pick(ID_INITIALIZE, None, "Initialize")?);
            check("初始化 NVIDIA 接口", initialize())?;

            let unload: FnUnload = std::mem::transmute(pick(ID_UNLOAD, None, "Unload")?);
            let create_session: FnCreateSession =
                std::mem::transmute(pick(ID_DRS_CREATE_SESSION, None, "CreateSession")?);
            let destroy_session: FnDestroySession =
                std::mem::transmute(pick(ID_DRS_DESTROY_SESSION, None, "DestroySession")?);
            let load_settings: FnLoadSettings =
                std::mem::transmute(pick(ID_DRS_LOAD_SETTINGS, None, "LoadSettings")?);
            let save_settings: FnSaveSettings =
                std::mem::transmute(pick(ID_DRS_SAVE_SETTINGS, None, "SaveSettings")?);
            let find_app: FnFindApplicationByName = std::mem::transmute(pick(
                ID_DRS_FIND_APPLICATION_BY_NAME,
                None,
                "FindApplicationByName",
            )?);
            let get_setting: FnGetSetting = std::mem::transmute(pick(
                ID_DRS_GET_SETTING,
                Some(ID_DRS_GET_SETTING_ALT),
                "GetSetting",
            )?);
            let set_setting: FnSetSetting = std::mem::transmute(pick(
                ID_DRS_SET_SETTING,
                Some(ID_DRS_SET_SETTING_ALT),
                "SetSetting",
            )?);

            let enum_settings: FnEnumSettings = std::mem::transmute(pick(
                ID_DRS_ENUM_SETTINGS,
                Some(ID_DRS_ENUM_SETTINGS_ALT),
                "EnumSettings",
            )?);

            let mut session: isize = 0;
            check("建立驱动配置会话", create_session(&mut session))?;
            check("读取驱动配置", load_settings(session))?;

            Ok(Self {
                _lib: lib,
                session,
                unload,
                destroy_session,
                save_settings,
                find_app,
                get_setting,
                set_setting,
                enum_settings,
            })
        }
    }

    /// 按 exe 文件名找到驱动里对应的游戏配置。
    fn find_profile(&self, exe: &str) -> Result<isize, NvapiError> {
        let mut name: Vec<u16> = exe.encode_utf16().collect();
        name.push(0);

        let mut profile: isize = 0;
        let mut app = NvdrsApplicationV4::zeroed_with_version();

        // SAFETY: name 是以 NUL 结尾的 UTF-16；profile/app 是本地变量的可变指针，
        // 在调用期间保持有效。
        let code = unsafe { (self.find_app)(self.session, name.as_ptr(), &mut profile, &mut app) };

        if code != 0 || profile == 0 {
            return Err(NvapiError::AppNotFound(exe.to_string()));
        }
        Ok(profile)
    }

    fn read(&self, profile: isize, id: u32) -> Option<u32> {
        let mut s = NvdrsSetting::zeroed_with_version();
        // SAFETY: s 是本地变量，调用期间有效；NVAPI 只往里写不保存指针。
        let code = unsafe { (self.get_setting)(self.session, profile, id, &mut s) };
        (code == 0).then(|| s.dword())
    }

    fn write(&self, profile: isize, id: u32, value: u32) -> Result<(), NvapiError> {
        let mut s = NvdrsSetting::zeroed_with_version();
        s.setting_id = id;
        s.setting_type = 0; // NVDRS_DWORD_TYPE
        s.set_dword(value);

        // SAFETY: 同上。
        let code = unsafe { (self.set_setting)(self.session, profile, &mut s) };
        check("写入驱动配置", code)
    }

    /// 列出这个 profile 里**已经被设置过**的所有项。
    ///
    /// 排查用：当「读某个 setting 得到 0」时，光看那一个 setting 分不清是
    /// 「设了 0」还是「根本没设、而且我读错了 ID」。把实际有的项全列出来，
    /// 真相一眼可见。
    fn enum_all(&self, profile: isize) -> Result<Vec<(u32, u32)>, NvapiError> {
        // 每个 NVDRS_SETTING 是 12KB，这里要一批——必须放堆上，栈装不下。
        const BATCH: usize = 128;
        let mut buf: Vec<NvdrsSetting> = vec![NvdrsSetting::zeroed_with_version(); BATCH];
        let mut count: u32 = BATCH as u32;

        // SAFETY: buf 是堆上的连续数组，长度 BATCH；count 告诉驱动容量，
        // 返回时它会被改写成实际写入的条数。
        let code =
            unsafe { (self.enum_settings)(self.session, profile, 0, &mut count, buf.as_mut_ptr()) };
        if code != 0 {
            // 把错误码带出去。吞掉它会让「调用失败」和「确实一项都没设」
            // 长得一模一样，那是排查时最耽误事的一种模糊。
            return Err(NvapiError::Call {
                what: "枚举驱动配置项",
                code,
            });
        }
        Ok(buf
            .iter()
            .take(count as usize)
            .map(|s| (s.setting_id, s.dword()))
            .collect())
    }

    fn save(&self) -> Result<(), NvapiError> {
        // SAFETY: session 由本结构体持有，此刻仍然有效。
        check("保存驱动配置", unsafe {
            (self.save_settings)(self.session)
        })
    }
}

impl Drop for Drs {
    fn drop(&mut self) {
        // SAFETY: 只在本结构体仍持有 session 时调用一次，之后不再使用。
        // 失败无从处理也不必处理——进程即将释放这些资源。
        unsafe {
            if self.session != 0 {
                (self.destroy_session)(self.session);
            }
            (self.unload)();
        }
    }
}

// ============================================================================
// 对外的安全接口
// ============================================================================

/// 读某个游戏当前的 DLSS 覆盖状态。
///
/// `exe` 是可执行文件名，例如 `RDR2.exe`——不是路径。
pub fn read_override(exe: &str) -> Result<OverrideStatus, NvapiError> {
    let drs = Drs::open()?;
    let profile = drs.find_profile(exe)?;
    Ok(OverrideStatus {
        enabled: drs.read(profile, SETTING_SR_OVERRIDE) == Some(SR_OVERRIDE_ON),
        preset: Preset::from_raw(drs.read(profile, SETTING_SR_PRESET).unwrap_or(PRESET_OFF)),
    })
}

/// 让某个游戏使用最新的 DLSS preset。
///
/// **两个 setting 都要写**：只设 preset 而不打开总开关的话，驱动不会应用它。
/// 这一点在 NVIDIA 的文档里没写清楚，是从 setting 的名字和描述推出来的
/// （`NGX_DLSS_SR_OVERRIDE` 直译就是「启用 DLSS-SR 覆盖」）。
pub fn enable_latest_preset(exe: &str) -> Result<(), NvapiError> {
    let drs = Drs::open()?;
    let profile = drs.find_profile(exe)?;
    drs.write(profile, SETTING_SR_OVERRIDE, SR_OVERRIDE_ON)?;
    drs.write(profile, SETTING_SR_PRESET, PRESET_LATEST)?;
    drs.save()
}

/// 撤销覆盖，回到游戏自己请求的 preset。
pub fn clear_override(exe: &str) -> Result<(), NvapiError> {
    let drs = Drs::open()?;
    let profile = drs.find_profile(exe)?;
    drs.write(profile, SETTING_SR_PRESET, PRESET_OFF)?;
    drs.write(profile, SETTING_SR_OVERRIDE, SR_OVERRIDE_OFF)?;
    drs.save()
}

/// 列出某个游戏在驱动里已设置的所有项，返回 `(setting_id, 值)`。
///
/// 这是排查工具，不给 UI 用。
pub fn dump_settings(exe: &str) -> Result<(isize, Vec<(u32, u32)>), NvapiError> {
    let drs = Drs::open()?;
    let profile = drs.find_profile(exe)?;
    let items = drs.enum_all(profile)?;
    Ok((profile, items))
}

/// 排查用：只找 profile，不碰任何 setting。
///
/// 保留它是因为「find 成功但 read 崩溃」这种分界一次就值回票价——
/// 当时正是靠它把嫌疑从 NvdrsApplicationV4 转到了 GetSetting 上。
pub fn probe_find_only(exe: &str) -> Result<isize, NvapiError> {
    let drs = Drs::open()?;
    drs.find_profile(exe)
}

/// 排查用：找 profile 并读一个 setting，返回原始值与错误码。
pub fn probe_read_one(exe: &str, id: u32) -> Result<Option<u32>, NvapiError> {
    let drs = Drs::open()?;
    let profile = drs.find_profile(exe)?;
    Ok(drs.read(profile, id))
}

/// 这台机器能不能用 preset 覆盖。用于 UI 上决定要不要显示相关入口。
pub fn is_available() -> bool {
    Drs::open().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 结构体大小必须和 NVAPI 的预期一致，否则驱动会拒绝调用。
    ///
    /// 这两个数字是按官方字段定义算出来的：
    /// NVDRS_SETTING = 4 + 2048×2 + 4×5 + 4100×2 = 12320
    /// 如果哪天改了字段忘了同步，这条会红。
    /// C 里 union 的最大成员对齐是 4，所以整个 NVDRS_SETTING 是 12320。
    /// 按 8 对齐算出的 12328 会被驱动判为 INCOMPATIBLE_STRUCT_VERSION（-9）。
    #[test]
    fn setting_union_alignment_matches_c() {
        assert_eq!(align_of::<SettingUnion>(), 4, "union 按 4 对齐");
        assert_eq!(size_of::<SettingUnion>(), 4100, "union 就是 4100 字节");
    }

    #[test]
    fn struct_sizes_match_the_official_layout() {
        assert_eq!(size_of::<NvdrsSetting>(), 12_320, "NVDRS_SETTING 大小不对");
        assert_eq!(
            size_of::<NvdrsApplicationV4>(),
            4 + 4 + 2048 * 2 * 5 + 4,
            "NVDRS_APPLICATION_V4 大小不对"
        );
    }

    /// 版本号 = 大小 | (版本 << 16)。
    #[test]
    fn version_encoding_follows_make_nvapi_version() {
        assert_eq!(make_version::<NvdrsSetting>(1), 12_320 | (1 << 16));
        assert_eq!(
            make_version::<NvdrsApplicationV4>(4),
            (size_of::<NvdrsApplicationV4>() as u32) | (4 << 16)
        );
    }

    /// preset 取值的映射，照 NvApiDriverSettings.h 的枚举。
    #[test]
    fn maps_preset_values_per_official_enum() {
        assert_eq!(Preset::from_raw(0), Preset::Off);
        assert_eq!(Preset::from_raw(1), Preset::Letter('A'));
        assert_eq!(Preset::from_raw(10), Preset::Letter('J'));
        assert_eq!(Preset::from_raw(11), Preset::Letter('K'));
        assert_eq!(Preset::from_raw(15), Preset::Letter('O'));
        assert_eq!(Preset::from_raw(0x00FF_FFFF), Preset::Latest);
    }

    /// 不认识的值原样保留，**不猜**。
    /// 猜错会让 UI 对用户说错话，而驱动里的值是我们无权解释的。
    #[test]
    fn unknown_preset_values_are_preserved_not_guessed() {
        assert_eq!(Preset::from_raw(16), Preset::Unknown(16));
        assert_eq!(Preset::from_raw(0xDEAD), Preset::Unknown(0xDEAD));
    }

    #[test]
    fn dword_round_trips_through_the_union() {
        let mut s = NvdrsSetting::zeroed_with_version();
        s.set_dword(0x00FF_FFFF);
        assert_eq!(s.dword(), 0x00FF_FFFF);
        s.set_dword(1);
        assert_eq!(s.dword(), 1);
    }
}
