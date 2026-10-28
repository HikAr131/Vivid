//! 全局 Vulkan layer 的固定 UAC helper。
//!
//! Vulkan 方案的 layer 要写 `C:\ProgramData\ReShade` 和 HKLM，需要管理员权限。
//! Vivid **不常驻管理员权限**，只为这一次操作请求一次 UAC。
//!
//! 这条链最危险的地方是命令行：提权进程拥有管理员权限，如果它肯照着命令行里的
//! 路径写文件，那就等于给了任何能拼命令行的东西一个提权写任意文件的入口。
//! 所以本模块的形状是刻意的：
//!
//! - 命令行**固定四个 token**：`--elevated-apply <app-id> --request <nonce>`，
//!   app-id 只允许十进制数字、nonce 只允许 ASCII 字母数字和连字符，
//!   除此之外没有任何可变内容；
//! - **提权进程自己再判一次这款游戏是不是真的需要管理员权限。**
//!   命令行里的 App ID 是不可信输入：它最多能指定「用户 Steam 库里的哪一款游戏」，
//!   而那款游戏必须自己解析出 Vulkan 方案，否则提权进程什么都不做就退出；
//! - 提权进程**不接受任何路径**：游戏目录由它自己按 Steam App ID 重新发现，
//!   组件只从已缓存且哈希有效的 artifact 取，写入目标由代码内配方决定；
//! - request / result 文件都在固定的 `%LOCALAPPDATA%\Vivid\pending` 下，
//!   文件名由 nonce 决定，内容只有 schema、nonce 和固定的游戏标识。
//!
//! 备份与还原仍然全部走 `vivid_core::deployment`，这里不另建一套。

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use vivid_core::artifact::{ArtifactCache, ArtifactKind};
use vivid_core::deployment::{
    Deployment, InstallPlan, InstalledArtifact, RealSystemBackend, TargetAllowlist,
};
use vivid_core::manager::{self, VividManager, WindowsEnvironment};
use vivid_core::net::UreqHttpClient;
use vivid_core::paths::AppPaths;
use vivid_core::recipe::{GameId, GameProfile, RECIPE_VERSION, requires_admin, validate_profile};
use vivid_core::{steam, vulkan_layer};

/// 提权入口的固定标志。
pub const ARG_ELEVATED_APPLY: &str = "--elevated-apply";
/// 固定的 request 标志。
pub const ARG_REQUEST: &str = "--request";
/// 命令行与请求文件里的游戏标识：**十进制 Steam App ID**。
///
/// 用 App ID 而不是名字，是为了让这个 token 的合法形状能被一行代码说清：
/// 全是数字，没有分隔符、没有路径、没有大小写问题。
pub fn game_token(game: GameId) -> String {
    game.app_id().to_string()
}

/// 解析游戏标识。只认十进制数字，别的一律拒绝。
pub fn parse_game_token(text: &str) -> Option<GameId> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().map(GameId::from_app_id)
}
/// request / result 文件的格式版本。
pub const ELEVATION_SCHEMA: u32 = 1;

/// 提权进程被允许做的两件事。**闭集**：请求文件里写别的一律拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationAction {
    /// 安装 RDR2 的全局 Vulkan layer。
    Apply,
    /// 还原这款游戏的全部改动（含全局 layer 与注册表）。
    Restore,
}

impl ElevationAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Restore => "restore",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "apply" => Some(Self::Apply),
            "restore" => Some(Self::Restore),
            _ => None,
        }
    }
}

/// nonce 长度上限。它会进命令行也会进文件名，不给它变长的机会。
const MAX_NONCE: usize = 64;

pub const EXIT_OK: i32 = 0;
pub const EXIT_APPLY_FAILED: i32 = 1;
pub const EXIT_NO_DATA_DIR: i32 = 2;
pub const EXIT_BAD_REQUEST: i32 = 3;
pub const EXIT_NOT_ELEVATED: i32 = 4;
pub const EXIT_BAD_ARGS: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevationError {
    /// nonce 不是 ASCII 字母数字/连字符，或者太长。
    BadNonce,
    /// 这款游戏不需要管理员权限（只有 Vulkan 方案需要）。
    UnsupportedGame,
    /// 用户在 UAC 弹窗上点了「否」。
    Cancelled,
    Malformed(String),
    Io(String),
    Failed(String),
}

impl std::fmt::Display for ElevationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadNonce => write!(f, "提权请求编号不合法"),
            Self::UnsupportedGame => write!(f, "这款游戏不需要管理员权限，Vivid 不会为它提权"),
            Self::Cancelled => write!(f, "已取消，未修改文件"),
            Self::Malformed(what) => write!(f, "提权请求内容不对：{what}"),
            Self::Io(what) => write!(f, "读写提权请求失败：{what}"),
            Self::Failed(what) => write!(f, "{what}"),
        }
    }
}

/// 一次提权请求。构造时就把游戏和 nonce 校验掉，
/// 因此 `args()` 永远只可能产出那四个固定 token。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElevationRequest {
    game: GameId,
    nonce: String,
    /// 具体做哪件事。**只在请求文件里传，不进命令行**——命令行形状必须固定。
    action: ElevationAction,
}

impl ElevationRequest {
    pub fn new(game: GameId, nonce: String) -> Result<Self, ElevationError> {
        Self::with_action(game, nonce, ElevationAction::Apply)
    }

    /// 构造一次请求。
    ///
    /// 这里**只校验形状**（游戏标识能不能来回转、nonce 合不合法）。
    /// 「这款游戏是不是真的需要管理员权限」由提权进程自己重新判一次——
    /// 请求文件是不可信输入，光在写它的那一侧检查等于没检查。
    pub fn with_action(
        game: GameId,
        nonce: String,
        action: ElevationAction,
    ) -> Result<Self, ElevationError> {
        if parse_game_token(&game_token(game)) != Some(game) {
            return Err(ElevationError::UnsupportedGame);
        }
        if !is_valid_nonce(&nonce) {
            return Err(ElevationError::BadNonce);
        }
        Ok(Self {
            game,
            nonce,
            action,
        })
    }

    pub fn action(&self) -> ElevationAction {
        self.action
    }

    pub fn game(&self) -> GameId {
        self.game
    }

    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// 提权进程的完整参数。**没有任何路径。**
    pub fn args(&self) -> Vec<String> {
        vec![
            ARG_ELEVATED_APPLY.to_string(),
            game_token(self.game),
            ARG_REQUEST.to_string(),
            self.nonce.clone(),
        ]
    }

    pub fn write(&self, paths: &AppPaths) -> Result<(), ElevationError> {
        let file = RequestFile {
            schema: ELEVATION_SCHEMA,
            nonce: self.nonce.clone(),
            game: game_token(self.game),
            action: self.action.as_str().to_string(),
        };
        write_json(&request_path(paths, &self.nonce), &file)
    }

    /// 读回自己写的 request。schema、游戏、nonce 三样都要对得上，
    /// 否则宁可失败也不照着一份被改过的文件动手。
    pub fn read(paths: &AppPaths, nonce: &str) -> Result<Self, ElevationError> {
        if !is_valid_nonce(nonce) {
            return Err(ElevationError::BadNonce);
        }
        let path = request_path(paths, nonce);
        let bytes = std::fs::read(&path).map_err(|e| ElevationError::Io(e.to_string()))?;
        let file: RequestFile =
            serde_json::from_slice(&bytes).map_err(|e| ElevationError::Malformed(e.to_string()))?;
        if file.schema != ELEVATION_SCHEMA {
            return Err(ElevationError::Malformed(format!("版本 {}", file.schema)));
        }
        let game = parse_game_token(&file.game).ok_or(ElevationError::UnsupportedGame)?;
        if file.nonce != nonce {
            return Err(ElevationError::Malformed("编号对不上".to_string()));
        }
        let action = ElevationAction::parse(&file.action)
            .ok_or_else(|| ElevationError::Malformed(format!("不认识的动作 {}", file.action)))?;
        Self::with_action(game, file.nonce, action)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestFile {
    schema: u32,
    nonce: String,
    /// 十进制 Steam App ID。存字符串而不是枚举，读的时候好当成不可信输入校验。
    game: String,
    /// 只可能是 `apply` 或 `restore`，同样当成不可信输入校验。
    action: String,
}

/// 提权进程留给普通进程看的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElevationOutcome {
    pub schema: u32,
    pub nonce: String,
    pub ok: bool,
    /// 直接显示给用户的中文。
    pub message: String,
}

impl ElevationOutcome {
    fn new(nonce: &str, ok: bool, message: String) -> Self {
        Self {
            schema: ELEVATION_SCHEMA,
            nonce: nonce.to_string(),
            ok,
            message,
        }
    }

    pub fn write(&self, paths: &AppPaths) -> Result<(), ElevationError> {
        write_json(&result_path(paths, &self.nonce), self)
    }

    /// 结果还没出来时返回 `None`——普通进程就是靠它轮询的。
    pub fn read(paths: &AppPaths, nonce: &str) -> Option<Self> {
        if !is_valid_nonce(nonce) {
            return None;
        }
        let bytes = std::fs::read(result_path(paths, nonce)).ok()?;
        let outcome: Self = serde_json::from_slice(&bytes).ok()?;
        (outcome.schema == ELEVATION_SCHEMA && outcome.nonce == nonce).then_some(outcome)
    }
}

/// request 文件位置。文件名只由 nonce 决定。
pub fn request_path(paths: &AppPaths, nonce: &str) -> PathBuf {
    paths.pending_dir().join(format!("elevation-{nonce}.json"))
}

/// result 文件位置。刻意不以 `.journal.json` 结尾，
/// 免得被 `deployment::recover_pending` 的中断恢复扫描当成 journal。
pub fn result_path(paths: &AppPaths, nonce: &str) -> PathBuf {
    paths
        .pending_dir()
        .join(format!("elevation-{nonce}.result.json"))
}

/// nonce 只允许 ASCII 字母数字和连字符：它既进命令行也进文件名，
/// 放行别的字符就等于放行路径穿越和命令行注入。
pub fn is_valid_nonce(nonce: &str) -> bool {
    !nonce.is_empty()
        && nonce.len() <= MAX_NONCE
        && nonce.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 生成一个 nonce。它只用来把本次请求和本次结果对上号，不是安全凭据
/// （pending 目录本来就在用户自己的 LOCALAPPDATA 下）。
pub fn generate_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{:x}", std::process::id())
}

/// 解析提权进程的命令行。形状必须**一字不差**，多一个参数都不认。
pub fn parse_cli(args: &[String]) -> Option<ElevationRequest> {
    if args.len() != 5 {
        return None;
    }
    if args[1] != ARG_ELEVATED_APPLY || args[3] != ARG_REQUEST {
        return None;
    }
    let game = parse_game_token(&args[2])?;
    ElevationRequest::new(game, args[4].clone()).ok()
}

fn write_json<T: Serialize>(path: &PathBuf, value: &T) -> Result<(), ElevationError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ElevationError::Io(e.to_string()))?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| ElevationError::Io(e.to_string()))?;
    std::fs::write(path, bytes).map_err(|e| ElevationError::Io(e.to_string()))
}

// ---------------------------------------------------------------------------
// 普通进程侧
// ---------------------------------------------------------------------------

/// 普通进程侧的完整流程：写 request → 请求一次 UAC → 等结果。
///
/// 只在用户点了「安装」或「还原」之后调用，而且只请求这一次；
/// Vivid 不常驻管理员权限。
pub fn run_with_uac(
    paths: &AppPaths,
    game: GameId,
    action: ElevationAction,
    timeout: Duration,
) -> Result<ElevationOutcome, ElevationError> {
    let request = ElevationRequest::with_action(game, generate_nonce(), action)?;
    request.write(paths)?;
    relaunch_elevated(&request)?;
    wait_for_outcome(paths, request.nonce(), timeout)
        .ok_or_else(|| ElevationError::Failed("等不到安装结果，请稍后重新检测状态".to_string()))
}

/// 轮询提权进程写下的结果文件。超时返回 `None`，不假装成功也不假装失败。
pub fn wait_for_outcome(
    paths: &AppPaths,
    nonce: &str,
    timeout: Duration,
) -> Option<ElevationOutcome> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(outcome) = ElevationOutcome::read(paths, nonce) {
            return Some(outcome);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// 提权进程入口
// ---------------------------------------------------------------------------

/// 提权进程要做的全部事情。返回进程退出码。
///
/// 它**只**装 RDR2 的全局 Vulkan layer——游戏目录里的文件普通权限就能写，
/// 不该被裹进管理员范围。
pub fn run_elevated_apply(request: &ElevationRequest) -> i32 {
    let Ok(paths) = AppPaths::detect() else {
        return EXIT_NO_DATA_DIR;
    };
    let nonce = request.nonce();

    // 命令行只是个信号，真正的请求以 pending 目录里那份为准。
    let on_disk = match ElevationRequest::read(&paths, nonce) {
        Ok(request) => request,
        Err(e) => {
            let _ = ElevationOutcome::new(nonce, false, e.to_string()).write(&paths);
            return EXIT_BAD_REQUEST;
        }
    };
    if !is_process_elevated() {
        let _ = ElevationOutcome::new(
            nonce,
            false,
            "没有拿到管理员权限，未修改任何文件".to_string(),
        )
        .write(&paths);
        return EXIT_NOT_ELEVATED;
    }

    // 游戏和动作都来自那份已校验的请求，不在这里再硬编码一次。
    let outcome = match on_disk.action() {
        ElevationAction::Apply => apply_global_layer(&paths, on_disk.game()),
        ElevationAction::Restore => restore_everything(&paths, on_disk.game()),
    };
    match outcome {
        Ok(message) => {
            let _ = ElevationOutcome::new(nonce, true, message).write(&paths);
            EXIT_OK
        }
        Err(e) => {
            let _ = ElevationOutcome::new(nonce, false, e.to_string()).write(&paths);
            EXIT_APPLY_FAILED
        }
    }
}

/// 重新发现游戏、重新生成计划、执行安装。**不接受任何外部传入的路径。**
fn apply_global_layer(paths: &AppPaths, game: GameId) -> Result<String, ElevationError> {
    let (app, profile) = resolve_vulkan_game(paths, game)?;

    // 只用普通进程刚刚下载校验好的那一版：提权进程不联网，也不自己挑版本，
    // 否则两个进程可能装上不同版本的 ReShade。
    let upstream =
        vivid_core::upstream::load_cached_upstream(&paths.upstream_cache()).ok_or_else(|| {
            ElevationError::Failed("本地没有上游发布记录，请先在主界面点一次安装".to_string())
        })?;
    let cache = ArtifactCache::new(paths.artifacts_dir());
    let reshade = cache
        .cached(ArtifactKind::ReShade, &upstream.reshade.version)
        .ok_or_else(|| {
            ElevationError::Failed(format!(
                "缓存里没有 ReShade {}，请先在主界面点一次安装",
                upstream.reshade.version
            ))
        })?;

    let allowlist = TargetAllowlist::for_profile(&profile);
    let work_dir = paths
        .cache_dir()
        .join("generated")
        .join(game.app_id().to_string());
    let probe = RealSystemBackend::new(allowlist.clone());
    let vulkan = vulkan_layer::build_vulkan_actions(
        &app.game_root,
        &reshade,
        &work_dir,
        &probe,
        manager::ini_template(game),
    )
    .map_err(|e| ElevationError::Failed(e.to_string()))?;

    let plan = InstallPlan {
        game_id: game,
        recipe_version: RECIPE_VERSION,
        components: vec![InstalledArtifact::from(&reshade)],
        actions: vulkan.elevated_actions(),
        requires_admin: true,
    };
    let mut backend = RealSystemBackend::new(allowlist.clone());
    Deployment::with_allowlist(paths, allowlist)
        .apply(&plan, &mut backend)
        .map_err(|e| ElevationError::Failed(e.to_string()))?;

    Ok("全局 Vulkan layer 已安装".to_string())
}

/// **提权进程自己再判一次这款游戏是不是真的需要管理员权限。**
///
/// 命令行和请求文件都是不可信输入：它们最多只能指定「用户 Steam 库里的哪一款游戏」，
/// 而那款游戏必须在本机解析出 Vulkan 方案（也就是真的要写全局层），
/// 否则提权进程什么都不做就退出。这样即使有人拼出一条命令行，
/// 也只能让 Vivid 对一款本来就该走这条路的游戏做它本来就会做的事。
fn resolve_vulkan_game(
    paths: &AppPaths,
    game: GameId,
) -> Result<(steam::SteamApp, GameProfile), ElevationError> {
    let app = steam::find_app(game.app_id())
        .ok_or_else(|| ElevationError::Failed("没找到这款游戏的 Steam 安装目录".to_string()))?;
    let manager = VividManager::new(paths.clone(), UreqHttpClient::default(), WindowsEnvironment);
    let profile = manager
        .profile_for(game)
        .map_err(|e| ElevationError::Failed(e.to_string()))?;
    validate_profile(&profile, &app).map_err(|e| ElevationError::Failed(e.to_string()))?;
    if !requires_admin(profile.api) {
        return Err(ElevationError::UnsupportedGame);
    }
    Ok((app, profile))
}

/// 提权还原：全局 layer 的文件和注册表值都要管理员权限才能改回去，
/// 所以整份 ledger 的还原都在提权进程里做（管理员当然也能写游戏目录）。
fn restore_everything(paths: &AppPaths, game: GameId) -> Result<String, ElevationError> {
    let (_, profile) = resolve_vulkan_game(paths, game)?;
    let allowlist = TargetAllowlist::for_profile(&profile);
    let mut backend = RealSystemBackend::new(allowlist.clone());
    Deployment::with_allowlist(paths, allowlist)
        .restore(&mut backend)
        .map_err(|e| ElevationError::Failed(e.to_string()))?;
    Ok("已还原到 Vivid 接管之前的状态".to_string())
}

// ---------------------------------------------------------------------------
// Windows 调用
//
// 这是 vivid-gui 里唯一用 unsafe 的地方，只有两件事：查当前进程是否已提权、
// 用 `runas` 重启自己。vivid-core 那边除 nvapi 外仍然零 unsafe。
// ---------------------------------------------------------------------------

/// 当前进程是否已经是管理员。
#[cfg(windows)]
pub fn is_process_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            std::ptr::from_mut(&mut elevation).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_process_elevated() -> bool {
    false
}

/// 以管理员身份重启自己，只带那四个固定参数。
#[cfg(windows)]
pub fn relaunch_elevated(request: &ElevationRequest) -> Result<(), ElevationError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    let exe = std::env::current_exe().map_err(|e| ElevationError::Io(e.to_string()))?;
    // nonce 已经限定在 ASCII 字母数字和连字符里，拼空格不会产生额外参数。
    let params = request.args().join(" ");
    let file = wide(exe.as_os_str());
    let verb = wide(std::ffi::OsStr::new("runas"));
    let params = wide(std::ffi::OsStr::new(&params));

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result as isize;
    if code > 32 {
        Ok(())
    } else if code == 5 {
        // SE_ERR_ACCESSDENIED：UAC 弹窗上点了「否」。
        Err(ElevationError::Cancelled)
    } else {
        Err(ElevationError::Failed(format!(
            "请求管理员权限失败（代码 {code}）"
        )))
    }
}

#[cfg(not(windows))]
pub fn relaunch_elevated(_request: &ElevationRequest) -> Result<(), ElevationError> {
    Err(ElevationError::Failed(
        "只有 Windows 支持这个操作".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (tempfile::TempDir, AppPaths) {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(temp.path().join("Vivid"));
        (temp, paths)
    }

    /// 提权进程的命令行里**不能出现任何用户可控的路径**——
    /// 目标路径必须由提权进程自己重新发现，不能从命令行接。
    #[test]
    fn helper_command_has_no_user_controlled_target_path() {
        let request = ElevationRequest::new(GameId::RedDeadRedemption2, "abc123".into()).unwrap();
        let args = request.args();
        // 游戏标识是十进制 App ID：形状能被一行代码说清，不含路径。
        assert_eq!(args, ["--elevated-apply", "1174180", "--request", "abc123"]);
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains(":\\") || arg.starts_with("\\\\"))
        );
    }

    /// nonce 只能是 ASCII 字母数字和连字符。它会进命令行，也会进文件名。
    #[test]
    fn nonce_rejects_anything_but_ascii_alnum_and_hyphen() {
        assert!(is_valid_nonce("abc123"));
        assert!(is_valid_nonce("a-b-c-1-2-3"));
        for bad in [
            "",
            "..",
            "a/b",
            "a\\b",
            "C:\\Windows",
            "a b",
            "a\"b",
            "a&calc",
            "中文",
            "a.json",
            &"x".repeat(65),
        ] {
            assert!(!is_valid_nonce(bad), "{bad:?} 不该被接受");
        }
    }

    #[test]
    fn generated_nonce_is_valid() {
        let nonce = generate_nonce();
        assert!(is_valid_nonce(&nonce), "{nonce}");
    }

    #[test]
    fn bad_nonce_cannot_build_a_request() {
        assert!(ElevationRequest::new(GameId::RedDeadRedemption2, "../evil".into()).is_err());
    }

    /// 游戏标识只能是十进制 App ID，别的形状一律拒绝。
    ///
    /// **「这款游戏该不该提权」不在这里判**——请求文件是不可信输入，
    /// 光在写它的那一侧检查等于没检查。真正的判定在 `resolve_vulkan_game`：
    /// 提权进程自己解析这款游戏，解析不出 Vulkan 方案就什么都不做。
    #[test]
    fn the_game_token_is_a_plain_app_id() {
        assert_eq!(game_token(GameId::RedDeadRedemption2), "1174180");
        assert_eq!(game_token(GameId::App(1_091_500)), "1091500");
        assert_eq!(
            parse_game_token("1174180"),
            Some(GameId::RedDeadRedemption2)
        );
        for bad in ["", "rdr2", "-1", "11a", "1174180 ", "0x10", "１１７"] {
            assert_eq!(parse_game_token(bad), None, "{bad}");
        }
    }

    #[test]
    fn parse_cli_accepts_only_the_fixed_shape() {
        let ok = parse_cli(&args(&[
            "Vivid.exe",
            "--elevated-apply",
            "1174180",
            "--request",
            "abc-123",
        ]))
        .unwrap();
        assert_eq!(ok.nonce(), "abc-123");
        assert_eq!(ok.game(), GameId::RedDeadRedemption2);

        for bad in [
            vec!["Vivid.exe"],
            vec!["Vivid.exe", "--elevated-apply", "1174180"],
            vec!["Vivid.exe", "--elevated-apply", "rdr2", "--request", "abc"],
            vec![
                "Vivid.exe",
                "--elevated-apply",
                r"..\evil",
                "--request",
                "abc",
            ],
            vec![
                "Vivid.exe",
                "--elevated-apply",
                "rdr2",
                "--request",
                "../evil",
            ],
            vec![
                "Vivid.exe",
                "--elevated-apply",
                "rdr2",
                "--request",
                "abc",
                "--extra",
            ],
            vec![
                "Vivid.exe",
                "--elevated-apply",
                "rdr2",
                "--path",
                r"C:\Windows",
            ],
        ] {
            assert!(parse_cli(&args(&bad)).is_none(), "{bad:?} 不该被接受");
        }
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn request_file_round_trips_through_the_pending_dir() {
        let (_temp, paths) = temp_paths();
        let request = ElevationRequest::new(GameId::RedDeadRedemption2, "abc123".into()).unwrap();
        request.write(&paths).unwrap();
        let path = request_path(&paths, "abc123");
        assert!(path.starts_with(paths.pending_dir()));

        let read = ElevationRequest::read(&paths, "abc123").unwrap();
        assert_eq!(read, request);
    }

    /// request 文件是 JSON，可能被改过：只允许 RDR2，别的一律拒绝。
    #[test]
    fn request_file_with_a_foreign_game_is_rejected() {
        let (_temp, paths) = temp_paths();
        let request = ElevationRequest::new(GameId::RedDeadRedemption2, "abc123".into()).unwrap();
        request.write(&paths).unwrap();

        let path = request_path(&paths, "abc123");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("\"1174180\"", "\"gta5-enhanced\"")).unwrap();
        assert!(ElevationRequest::read(&paths, "abc123").is_err());
    }

    /// nonce 对不上的 request 文件不能被当成自己的。
    #[test]
    fn request_file_with_a_mismatched_nonce_is_rejected() {
        let (_temp, paths) = temp_paths();
        let request = ElevationRequest::new(GameId::RedDeadRedemption2, "abc123".into()).unwrap();
        request.write(&paths).unwrap();

        let path = request_path(&paths, "abc123");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("abc123", "def456")).unwrap();
        assert!(ElevationRequest::read(&paths, "abc123").is_err());
    }

    #[test]
    fn result_file_round_trips_and_stays_in_pending() {
        let (_temp, paths) = temp_paths();
        let outcome = ElevationOutcome {
            schema: ELEVATION_SCHEMA,
            nonce: "abc123".to_string(),
            ok: false,
            message: "已取消，未修改文件".to_string(),
        };
        outcome.write(&paths).unwrap();
        let path = result_path(&paths, "abc123");
        assert!(path.starts_with(paths.pending_dir()));
        // 结果文件不能被 deployment 的中断恢复扫描误认成 journal
        assert!(!path.to_string_lossy().ends_with(".journal.json"));

        let read = ElevationOutcome::read(&paths, "abc123").unwrap();
        assert!(!read.ok);
        assert_eq!(read.message, "已取消，未修改文件");
    }

    /// 结果文件出现之前，轮询要如实返回「还没有结果」。
    #[test]
    fn waiting_returns_the_outcome_once_it_appears() {
        let (_temp, paths) = temp_paths();
        assert!(wait_for_outcome(&paths, "abc123", Duration::from_millis(60)).is_none());

        ElevationOutcome {
            schema: ELEVATION_SCHEMA,
            nonce: "abc123".to_string(),
            ok: true,
            message: "全局 Vulkan layer 已安装".to_string(),
        }
        .write(&paths)
        .unwrap();
        let outcome = wait_for_outcome(&paths, "abc123", Duration::from_millis(60)).unwrap();
        assert!(outcome.ok);
    }

    /// 动作只在请求文件里传，命令行形状不变。
    #[test]
    fn the_action_round_trips_through_the_request_file() {
        let (_temp, paths) = temp_paths();
        let request = ElevationRequest::with_action(
            GameId::RedDeadRedemption2,
            "abc123".into(),
            ElevationAction::Restore,
        )
        .unwrap();
        request.write(&paths).unwrap();
        assert_eq!(
            request.args(),
            ["--elevated-apply", "1174180", "--request", "abc123"]
        );

        let read = ElevationRequest::read(&paths, "abc123").unwrap();
        assert_eq!(read.action(), ElevationAction::Restore);
    }

    /// 请求文件里写了别的动作 → 拒绝，不猜它想干什么。
    #[test]
    fn an_unknown_action_is_rejected() {
        let (_temp, paths) = temp_paths();
        ElevationRequest::new(GameId::RedDeadRedemption2, "abc123".into())
            .unwrap()
            .write(&paths)
            .unwrap();
        let path = request_path(&paths, "abc123");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("\"apply\"", "\"format-c\"")).unwrap();
        assert!(ElevationRequest::read(&paths, "abc123").is_err());
    }

    #[test]
    fn missing_result_reads_as_none() {
        let (_temp, paths) = temp_paths();
        assert!(ElevationOutcome::read(&paths, "abc123").is_none());
    }
}
