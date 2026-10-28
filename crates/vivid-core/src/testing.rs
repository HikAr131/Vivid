//! 单元测试共用的基础夹具。
//!
//! 契约（见执行计划「测试夹具约定」）：
//!
//! - `FakeHttp` 按**完整 URL** 返回排队响应并记录调用次数；没有预设响应时返回
//!   `NetError::Request`。每个 URL 一个队列，同 URL 多次请求按 FIFO 依次弹出。
//! - `FakeSystemBackend` 的文件操作只接受测试临时目录（Task 7 的全局 layer 会再加
//!   虚拟 ProgramData 映射），注册表只写内存 map，并能在第 N 次写入时注入失败。
//! - 后续任务加入 `FakeEnvironment` 时同样放在这里；`ManagerFixture` /
//!   `EndToEndFixture` 只组合这些基础夹具，不另建语义不同的假实现。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::Digest;

use crate::deployment::{DeployError, RegistryLocation, SystemBackend};
use crate::gpu::{Generation, GpuInfo};
use crate::manager::EnvironmentProbe;
use crate::net::{HttpClient, HttpPayload, NetError};
use crate::recipe::GameId;
use crate::steam::SteamApp;

/// 按 URL 排队响应的假网络客户端。
pub struct FakeHttp {
    responses: Mutex<BTreeMap<String, VecDeque<Result<HttpPayload, NetError>>>>,
    calls: AtomicUsize,
    /// `single` 构造的一次性兜底响应；被消费后置 `None`。
    wildcard: Mutex<Option<Vec<u8>>>,
}

impl FakeHttp {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(BTreeMap::new()),
            calls: AtomicUsize::new(0),
            wildcard: Mutex::new(None),
        }
    }

    /// 用 `(完整 URL, 文本内容)` 对构造。内容以 200 成功响应返回。
    pub fn with_texts<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let fake = Self::new();
        for (url, text) in entries {
            fake.push(url, Ok(fake_payload(url, text.as_bytes().to_vec())));
        }
        fake
    }

    /// 给指定 URL 追加一个排队响应（成功或失败）。
    pub fn push(&self, url: &str, payload: Result<HttpPayload, NetError>) {
        self.responses
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default()
            .push_back(payload);
    }

    /// 该客户端总共被调用的次数。
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// 单响应客户端：无论请求哪个 URL，都用这份内容应答一次（再请求就报错）。
    /// 供「只发一个请求」的测试使用；多 URL 场景请用 `with_texts` / `push`。
    pub fn single(bytes: Vec<u8>) -> Self {
        let fake = Self::new();
        *fake.wildcard.lock().unwrap() = Some(bytes);
        fake
    }
}

impl Default for FakeHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for FakeHttp {
    fn get(&self, url: &str, limit: usize) -> Result<HttpPayload, NetError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(queued) = self
            .responses
            .lock()
            .unwrap()
            .get_mut(url)
            .and_then(VecDeque::pop_front)
        {
            return match queued {
                Ok(payload) => {
                    if payload.bytes.len() > limit {
                        Err(NetError::TooLarge { limit })
                    } else {
                        Ok(payload)
                    }
                }
                Err(e) => Err(e),
            };
        }
        if let Some(bytes) = self.wildcard.lock().unwrap().take() {
            return Ok(fake_payload(url, bytes));
        }
        Err(NetError::Request(format!("no queued response for {url}")))
    }
}

/// 假系统边界：文件只落在测试临时目录，注册表只写内存 map。
///
/// 越界访问一律 **panic** 而不是返回错误：能走到这里说明 `deployment` 的白名单
/// 守门没拦住，那是生产代码的缺陷，必须让测试炸得无法忽略（这也是白名单
/// 变异测试的判据——把守门去掉，越界测试会在这里炸）。
pub struct FakeSystemBackend {
    /// 允许真实落盘的根目录。
    roots: Vec<PathBuf>,
    /// 虚拟系统目录映射（如 C:\ProgramData → 临时目录）。
    /// 计划里的目标仍是真实路径，只有读写时被换成临时目录。
    redirects: Vec<(PathBuf, PathBuf)>,
    registry: BTreeMap<(RegistryLocation, String), u32>,
    apply_count: usize,
    /// 在第 N 次写入（1 起算）返回失败；触发一次后解除，好让回滚能跑完。
    fail_on: BTreeSet<usize>,
}

impl FakeSystemBackend {
    pub fn new(root: &Path) -> Self {
        Self {
            roots: vec![root.to_path_buf()],
            redirects: Vec::new(),
            registry: BTreeMap::new(),
            apply_count: 0,
            fail_on: BTreeSet::new(),
        }
    }

    /// 把 `virtual_root` 下的读写重定向到 `real_root`（测试用的虚拟 ProgramData）。
    pub fn map_programdata(&mut self, virtual_root: &Path, real_root: &Path) {
        self.redirects
            .push((virtual_root.to_path_buf(), real_root.to_path_buf()));
    }

    /// 让第 `ns` 次写入失败（1 起算）。每个编号只触发一次。
    pub fn fail_on_applies(&mut self, ns: impl IntoIterator<Item = usize>) {
        self.fail_on.extend(ns);
    }

    /// 至今为止的写入次数（`replace_file` + `write_registry_dword`）。
    pub fn apply_count(&self) -> usize {
        self.apply_count
    }

    pub fn registry_value(&self, location: &RegistryLocation, name: &str) -> Option<u32> {
        self.registry
            .get(&(location.clone(), name.to_string()))
            .copied()
    }

    /// 内存注册表的全部内容，测试用来断言「什么都没被写过」。
    pub fn registry_dump(&self) -> Vec<String> {
        self.registry
            .iter()
            .map(|((location, name), value)| format!("{location:?}|{name}={value}"))
            .collect()
    }

    /// 映射到真实临时路径；越界即 panic。
    fn resolve(&self, path: &Path) -> PathBuf {
        let mut resolved = path.to_path_buf();
        let key = normalize(path);
        for (virtual_root, real_root) in &self.redirects {
            let prefix = normalize(virtual_root);
            if let Some(rest) = key.strip_prefix(&prefix) {
                resolved = real_root.join(rest.trim_start_matches(['\\', '/']));
                break;
            }
        }
        let resolved_key = normalize(&resolved);
        let inside = self
            .roots
            .iter()
            .any(|root| resolved_key.starts_with(&normalize(root)));
        assert!(
            inside,
            "测试夹具拒绝越界访问：{}（不在测试临时目录内）",
            path.display()
        );
        resolved
    }

    /// 记一次写入；命中注入编号则返回失败并解除该编号。
    fn count_apply(&mut self, what: &str) -> Result<(), DeployError> {
        self.apply_count += 1;
        if self.fail_on.remove(&self.apply_count) {
            return Err(DeployError::Io(format!(
                "注入的第 {} 次写入失败：{what}",
                self.apply_count
            )));
        }
        Ok(())
    }
}

impl SystemBackend for FakeSystemBackend {
    fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, DeployError> {
        let path = &self.resolve(path);
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DeployError::Io(format!("读 {} 失败：{e}", path.display()))),
        }
    }

    fn replace_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), DeployError> {
        let path = &self.resolve(path);
        self.count_apply(&path.display().to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| DeployError::Io(format!("建目录失败：{e}")))?;
        }
        std::fs::write(path, bytes).map_err(|e| DeployError::Io(format!("写文件失败：{e}")))
    }

    fn remove_file(&mut self, path: &Path) -> Result<(), DeployError> {
        let path = &self.resolve(path);
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(DeployError::Io(format!("删 {} 失败：{e}", path.display()))),
        }
    }

    fn remove_empty_dir(&mut self, path: &Path) -> Result<(), DeployError> {
        let path = self.resolve(path);
        let _ = std::fs::remove_dir(path);
        Ok(())
    }

    fn read_registry_dword(
        &self,
        location: &RegistryLocation,
        name: &str,
    ) -> Result<Option<u32>, DeployError> {
        Ok(self.registry_value(location, name))
    }

    fn write_registry_dword(
        &mut self,
        location: &RegistryLocation,
        name: &str,
        value: u32,
    ) -> Result<(), DeployError> {
        self.count_apply(name)?;
        self.registry
            .insert((location.clone(), name.to_string()), value);
        Ok(())
    }

    fn remove_registry_value(
        &mut self,
        location: &RegistryLocation,
        name: &str,
    ) -> Result<(), DeployError> {
        self.registry.remove(&(location.clone(), name.to_string()));
        Ok(())
    }
}

/// 假环境探测：固定的游戏目录与显卡。
///
/// 自动测试不该依赖本机真的装了 Steam、真的插着 RTX 卡——那样的测试
/// 换台机器就红，红了也说明不了产品有问题。
pub struct FakeEnvironment {
    games: BTreeMap<GameId, SteamApp>,
    gpu: Option<GpuInfo>,
}

impl FakeEnvironment {
    pub fn new(gpu: Option<GpuInfo>) -> Self {
        Self {
            games: BTreeMap::new(),
            gpu,
        }
    }

    pub fn with_game(mut self, id: GameId, app: SteamApp) -> Self {
        self.games.insert(id, app);
        self
    }
}

impl EnvironmentProbe for FakeEnvironment {
    fn find_game(&self, game_id: GameId) -> Option<SteamApp> {
        self.games.get(&game_id).cloned()
    }

    fn all_games(&self) -> Vec<SteamApp> {
        self.games.values().cloned().collect()
    }

    fn primary_gpu(&self) -> Option<GpuInfo> {
        self.gpu.clone()
    }
}

/// 执行计划里指定的固定测试显卡。
pub fn rtx_4070_ti_super() -> GpuInfo {
    GpuInfo {
        model: "NVIDIA GeForce RTX 4070 Ti SUPER".to_string(),
        driver_windows: "32.0.15.8108".to_string(),
        driver_nvidia: Some("581.08".to_string()),
        generation: Some(Generation::Ada),
    }
}

/// 路径比较键：分隔符统一成反斜杠、大小写忽略。
fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// 构造一个成功响应载荷，`final_url` 与请求地址一致。
pub fn fake_payload(url: &str, bytes: Vec<u8>) -> HttpPayload {
    HttpPayload {
        bytes,
        requested_url: url.to_string(),
        final_url: url.to_string(),
    }
}

/// 把 `(路径, 内容)` 列表打包成一个内存 ZIP。
pub fn zip_with_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in files {
        writer
            .start_file(name, options)
            .expect("写测试 ZIP 条目失败");
        writer.write_all(bytes).expect("写测试 ZIP 内容失败");
    }
    writer.finish().expect("收尾测试 ZIP 失败").into_inner()
}

/// 字节的 SHA-256 十六进制小写串。
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
