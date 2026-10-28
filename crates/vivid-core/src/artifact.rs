//! 组件下载缓存：下载 → 按容器类型取出目标文件 → 校验 → 落盘 + 元数据。
//!
//! 这是「文件从网络到游戏目录」这条链的第一站。缓存目录结构固定为
//! `<root>/<kind>/<version>/<expected_file>`，旁边写 `artifact.json` 记录
//! 来源 URL、最终 URL、SHA-256、大小。**缓存先于任何游戏写入**：
//! 下载、提取、校验失败时游戏目录一个字节都不会被碰。
//!
//! 校验三层：
//!
//! 1. 下载响应有大小上限（256 MiB，`net` 层强制）；
//! 2. 提取出的文件必须是 PE（`MZ` 头）——压缩包里放个文本文件冒充 DLL
//!    是常见事故，这里直接拒绝；
//! 3. SHA-256 与大小记入元数据；之后每次 `ensure` / `latest` 都重算实际哈希
//!    与元数据比对，不一致就当缓存不存在（重新下载或跳过）。
//!
//! 提取规则：只按**大小写不敏感的精确 basename** 从包里找一个条目，
//! 其余条目一律不落盘。ReShade setup 是 PE 自解压壳 + 尾部 ZIP，
//! 从 `PK\x03\x04` 处切开尝试打开 ZIP，直到找到目标文件。

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::net::{HttpClient, NetError};
use crate::upstream::ContainerKind;

/// 单个组件下载的硬上限：256 MiB。
pub const DOWNLOAD_LIMIT: usize = 256 * 1024 * 1024;

/// 组件类别，同时决定缓存子目录名与界面展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    /// ReShade addon build（从 setup 里取出 `ReShade64.dll`）。
    ReShade,
    /// RenoDX DLSS5 的 ReShade addon。
    RenoDxDlss5,
    /// NVIDIA DLSS 超分。
    Dlss,
    /// NVIDIA DLSS 神经渲染。
    DlssNr,
}

impl ArtifactKind {
    /// 缓存子目录名，固定英文小写。
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::ReShade => "reshade",
            Self::RenoDxDlss5 => "renodx-dlss5",
            Self::Dlss => "dlss",
            Self::DlssNr => "dlssnr",
        }
    }
}

/// 一次缓存请求：要什么、从哪下载、怎么取出来。
#[derive(Debug, Clone)]
pub struct ArtifactSpec {
    pub kind: ArtifactKind,
    pub version: String,
    pub url: String,
    /// 从下载物里取出的目标文件名（精确 basename，大小写不敏感匹配）。
    pub expected_file: String,
    pub container: ContainerKind,
}

/// 一份已缓存、已校验的组件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedArtifact {
    pub kind: ArtifactKind,
    pub version: String,
    pub source_url: String,
    /// 重定向之后的实际地址，界面展示用。
    pub final_url: String,
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
    /// 缓存文件路径。**不写进 `artifact.json`**——它由根目录 + kind + version + 文件名
    /// 唯一决定，反序列化后再拼接回来。
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("下载 {url} 失败：{reason}")]
    Download { url: String, reason: String },

    #[error("包里找不到需要的文件 {0}")]
    ExpectedFileMissing(String),

    #[error("包里同名文件有多个，无法确定取哪个：{0}")]
    AmbiguousEntry(String),

    #[error("{0} 不是有效的程序文件（缺 PE 头）")]
    NotPe(String),

    #[error("缓存元数据读不懂：{0}")]
    BadMetadata(String),

    #[error("缓存文件与记录的 SHA-256 不一致：{0}")]
    HashMismatch(String),

    #[error("写缓存失败：{0}")]
    Io(String),
}

/// 组件缓存仓库。
pub struct ArtifactCache {
    root: PathBuf,
}

impl ArtifactCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 取回 `spec` 对应组件的缓存；缓存有效时**零网络请求**，否则下载、提取、校验并落盘。
    pub fn ensure(
        &self,
        http: &dyn HttpClient,
        spec: &ArtifactSpec,
    ) -> Result<CachedArtifact, ArtifactError> {
        let dir = self.kind_dir(spec.kind, &spec.version);
        let file_path = dir.join(&spec.expected_file);

        if let Some(cached) = self.load_meta(&dir)
            && cached.file_name.eq_ignore_ascii_case(&spec.expected_file)
            && cached.version == spec.version
            && self.hash_matches(&cached, &file_path)
        {
            return Ok(cached);
        }

        let payload = http.get(&spec.url, DOWNLOAD_LIMIT).map_err(|e| match e {
            NetError::InsecureUrl(u) | NetError::Request(u) => ArtifactError::Download {
                url: spec.url.clone(),
                reason: u,
            },
            NetError::TooLarge { limit } => ArtifactError::Download {
                url: spec.url.clone(),
                reason: format!("超过允许的 {limit} 字节"),
            },
        })?;

        let bytes = extract_expected(&payload.bytes, spec)?;
        validate_pe(&spec.expected_file, &bytes)?;

        let sha256 = sha256_hex(&bytes);
        let cached = CachedArtifact {
            kind: spec.kind,
            version: spec.version.clone(),
            source_url: spec.url.clone(),
            final_url: payload.final_url,
            file_name: spec.expected_file.clone(),
            sha256,
            size: bytes.len() as u64,
            path: file_path.clone(),
        };

        // 同目录临时文件写文件与元数据，全部就绪后再替换。
        std::fs::create_dir_all(&dir)
            .map_err(|e| ArtifactError::Io(format!("建缓存目录失败：{e}")))?;
        let tmp_file = dir.join(format!("{}.tmp", spec.expected_file));
        std::fs::write(&tmp_file, &bytes)
            .map_err(|e| ArtifactError::Io(format!("写缓存临时文件失败：{e}")))?;
        let meta_json = serde_json::to_vec_pretty(&cached)
            .map_err(|e| ArtifactError::BadMetadata(e.to_string()))?;
        let tmp_meta = dir.join("artifact.json.tmp");
        std::fs::write(&tmp_meta, meta_json)
            .map_err(|e| ArtifactError::Io(format!("写缓存元数据失败：{e}")))?;

        std::fs::rename(&tmp_file, &file_path)
            .map_err(|e| ArtifactError::Io(format!("落位缓存文件失败：{e}")))?;
        std::fs::rename(&tmp_meta, dir.join("artifact.json"))
            .map_err(|e| ArtifactError::Io(format!("落位缓存元数据失败：{e}")))?;

        Ok(cached)
    }

    /// 按 kind + version 精确取缓存，**不联网**。
    ///
    /// 提权进程用它：普通进程 `prepare` 时已经把这一版下载校验好了，
    /// 提权进程只需要拿同一版，不能自己去挑「最新的那个」——
    /// 那样两个进程可能装上不同版本。
    pub fn cached(&self, kind: ArtifactKind, version: &str) -> Option<CachedArtifact> {
        let dir = self.kind_dir(kind, version);
        let cached = self.load_meta(&dir)?;
        let file_path = dir.join(&cached.file_name);
        self.hash_matches(&cached, &file_path).then_some(cached)
    }

    /// 找 `kind` 的最高版本缓存；只返回元数据与实际哈希一致的那份。
    ///
    /// 版本排序：能用 `SemVer` 解析的按数字比较；解析不了的按字符串比较兜底
    /// （离线安装只用 ReShade / RenoDX，版本都是数字三段式，兜底极少触发）。
    pub fn latest(&self, kind: ArtifactKind) -> Result<Option<CachedArtifact>, ArtifactError> {
        let kind_root = self.root.join(kind.dir_name());
        let Ok(entries) = std::fs::read_dir(&kind_root) else {
            return Ok(None);
        };
        let mut best: Option<CachedArtifact> = None;
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(cached) = self.load_meta(&dir) else {
                continue;
            };
            let file_path = dir.join(&cached.file_name);
            if !self.hash_matches(&cached, &file_path) {
                continue; // 元数据与实际哈希不一致：不当缓存用
            }
            let better = match &best {
                None => true,
                Some(current) => {
                    let a = version_key(&cached.version);
                    let b = version_key(&current.version);
                    a > b
                }
            };
            if better {
                best = Some(cached);
            }
        }
        Ok(best)
    }

    fn kind_dir(&self, kind: ArtifactKind, version: &str) -> PathBuf {
        self.root.join(kind.dir_name()).join(version)
    }

    /// 读 `artifact.json` 并把 `path` 拼回目录位置；缺失或损坏返回 `None`。
    fn load_meta(&self, dir: &Path) -> Option<CachedArtifact> {
        let bytes = std::fs::read(dir.join("artifact.json")).ok()?;
        let mut cached: CachedArtifact = serde_json::from_slice(&bytes).ok()?;
        cached.path = dir.join(&cached.file_name);
        Some(cached)
    }

    /// 校验缓存文件的实际哈希与元数据一致。
    fn hash_matches(&self, cached: &CachedArtifact, file_path: &Path) -> bool {
        match std::fs::read(file_path) {
            Ok(bytes) => bytes.len() as u64 == cached.size && sha256_hex(&bytes) == cached.sha256,
            Err(_) => false,
        }
    }
}

/// 按容器类型从下载物里取出 `expected_file`。
fn extract_expected(download: &[u8], spec: &ArtifactSpec) -> Result<Vec<u8>, ArtifactError> {
    match spec.container {
        ContainerKind::Direct => Ok(download.to_vec()),
        ContainerKind::Zip => extract_from_zip(download, &spec.expected_file, 0),
        ContainerKind::SelfExtractingZip => {
            // ReShade setup：PE 壳在前、ZIP 数据在后。从每个 PK 标记处尝试打开，
            // 第一个能取出目标文件的归档就是它。
            let mut offset = 0;
            while let Some(pos) = find_bytes(download, b"PK\x03\x04", offset) {
                match extract_from_zip(&download[pos..], &spec.expected_file, pos) {
                    Ok(bytes) => return Ok(bytes),
                    Err(ArtifactError::ExpectedFileMissing(_)) => offset = pos + 1,
                    Err(e) => return Err(e),
                }
            }
            Err(ArtifactError::ExpectedFileMissing(
                spec.expected_file.clone(),
            ))
        }
    }
}

/// 在 `data` 里从 `start` 起找 `needle`，返回偏移或 `None`。
fn find_bytes(data: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    data.get(start..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|p| p + start)
}

/// 打开 ZIP 并只取出 basename 精确匹配（大小写不敏感）的那一个条目。
/// `base` 是 zip 数据在下载物里的偏移，只用于错误信息。
fn extract_from_zip(
    zip_bytes: &[u8],
    expected_file: &str,
    base: usize,
) -> Result<Vec<u8>, ArtifactError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|e| ArtifactError::BadMetadata(format!("不是有效的 ZIP（偏移 {base}）：{e}")))?;

    let mut found: Option<usize> = None;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| ArtifactError::BadMetadata(format!("读 ZIP 条目失败：{e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let matches = Path::new(&name)
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(expected_file));
        if matches {
            if found.is_some() {
                return Err(ArtifactError::AmbiguousEntry(expected_file.to_string()));
            }
            found = Some(i);
        }
    }

    let Some(index) = found else {
        return Err(ArtifactError::ExpectedFileMissing(
            expected_file.to_string(),
        ));
    };

    let mut entry = archive
        .by_index(index)
        .map_err(|e| ArtifactError::BadMetadata(format!("读 ZIP 条目失败：{e}")))?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| ArtifactError::BadMetadata(format!("解压失败：{e}")))?;
    Ok(bytes)
}

/// 要求提取出的文件带 PE 头。
fn validate_pe(name: &str, bytes: &[u8]) -> Result<(), ArtifactError> {
    if bytes.len() >= 2 && &bytes[..2] == b"MZ" {
        Ok(())
    } else {
        Err(ArtifactError::NotPe(name.to_string()))
    }
}

/// 字节的 SHA-256 十六进制小写串，与 `backup::hash_file` 的格式一致。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 版本排序键：数字三段式优先，解析不了退回字符串。
fn version_key(version: &str) -> (u8, crate::update::SemVer, String) {
    match crate::update::SemVer::parse(version) {
        Some(v) => (0, v, String::new()),
        None => (1, crate::update::SemVer::new(0, 0, 0), version.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHttp, sha256_bytes, zip_with_files};

    fn dlss_spec() -> ArtifactSpec {
        ArtifactSpec {
            kind: ArtifactKind::Dlss,
            version: "310.8.0".to_string(),
            url: "https://example.com/dlss.zip".to_string(),
            expected_file: "nvngx_dlss.dll".to_string(),
            container: ContainerKind::Zip,
        }
    }

    /// 持有 TempDir 的缓存夹具：目录随夹具一起释放，不泄漏临时文件。
    struct CacheFixture {
        _temp: tempfile::TempDir,
        cache: ArtifactCache,
    }

    fn cache() -> CacheFixture {
        let temp = tempfile::tempdir().unwrap();
        let cache = ArtifactCache::new(temp.path().to_path_buf());
        CacheFixture { _temp: temp, cache }
    }

    #[test]
    fn zip_extraction_uses_exact_basename_and_records_hash() {
        let temp = tempfile::tempdir().unwrap();
        let payload = zip_with_files([
            ("nested/nvngx_dlss.dll", b"MZ-valid-dlss".as_slice()),
            ("nested/not-this.dll", b"MZ-other".as_slice()),
        ]);
        let client = FakeHttp::single(payload);
        let cached = ArtifactCache::new(temp.path().to_path_buf())
            .ensure(&client, &dlss_spec())
            .unwrap();
        assert_eq!(cached.path.file_name().unwrap(), "nvngx_dlss.dll");
        assert_eq!(cached.sha256, sha256_bytes(b"MZ-valid-dlss"));
        assert_eq!(std::fs::read(cached.path).unwrap(), b"MZ-valid-dlss");
    }

    #[test]
    fn rejects_archive_without_expected_file() {
        let client = FakeHttp::single(zip_with_files([("wrong.dll", b"MZ-wrong".as_slice())]));
        let fixture = cache();
        let err = fixture.cache.ensure(&client, &dlss_spec()).unwrap_err();
        assert!(matches!(err, ArtifactError::ExpectedFileMissing { .. }));
    }

    #[test]
    fn direct_addon64_is_cached_as_is() {
        let spec = ArtifactSpec {
            kind: ArtifactKind::RenoDxDlss5,
            version: "1.10.0".to_string(),
            url: "https://example.com/renodx-dlss5.addon64".to_string(),
            expected_file: "renodx-dlss5.addon64".to_string(),
            container: ContainerKind::Direct,
        };
        let client = FakeHttp::single(b"MZ-addon-bytes".to_vec());
        let fixture = cache();
        let cached = fixture.cache.ensure(&client, &spec).unwrap();
        assert_eq!(std::fs::read(cached.path).unwrap(), b"MZ-addon-bytes");
    }

    #[test]
    fn self_extracting_reshade_setup_yields_reshade64_dll() {
        let zip = zip_with_files([("ReShade64.dll", b"MZ-reshade".as_slice())]);
        let mut payload = b"MZ-this-is-the-pe-stub-".to_vec();
        payload.extend_from_slice(&zip);
        let spec = ArtifactSpec {
            kind: ArtifactKind::ReShade,
            version: "6.8.1".to_string(),
            url: "https://reshade.me/downloads/ReShade_Setup_6.8.1_Addon.exe".to_string(),
            expected_file: "ReShade64.dll".to_string(),
            container: ContainerKind::SelfExtractingZip,
        };
        let client = FakeHttp::single(payload);
        let fixture = cache();
        let cached = fixture.cache.ensure(&client, &spec).unwrap();
        assert_eq!(std::fs::read(cached.path).unwrap(), b"MZ-reshade");
    }

    #[test]
    fn non_pe_bytes_are_rejected() {
        // 用 Direct 容器直通字节，专测「提取出的文件必须带 MZ 头」这一层。
        let spec = ArtifactSpec {
            kind: ArtifactKind::Dlss,
            version: "310.8.0".to_string(),
            url: "https://example.com/nvngx_dlss.dll".to_string(),
            expected_file: "nvngx_dlss.dll".to_string(),
            container: ContainerKind::Direct,
        };
        let client = FakeHttp::single(b"just some text".to_vec());
        let fixture = cache();
        let err = fixture.cache.ensure(&client, &spec).unwrap_err();
        assert!(matches!(err, ArtifactError::NotPe(_)));
    }

    #[test]
    fn mismatched_metadata_triggers_redownload() {
        let temp = tempfile::tempdir().unwrap();
        let cache = ArtifactCache::new(temp.path().to_path_buf());
        let payload = zip_with_files([("nvngx_dlss.dll", b"MZ-good".as_slice())]);
        let client = FakeHttp::new();
        client.push(
            "https://example.com/dlss.zip",
            Ok(crate::testing::fake_payload(
                "https://example.com/dlss.zip",
                payload,
            )),
        );
        let first = cache.ensure(&client, &dlss_spec()).unwrap();

        // 破坏元数据：把记录的哈希改掉，实际文件不动。
        let meta_path = first.path.parent().unwrap().join("artifact.json");
        let mut meta: CachedArtifact =
            serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta.sha256 = "0".repeat(64);
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();

        // 第二次 ensure 应无视坏元数据重新下载；FakeHttp 队列已空会失败——
        // 再推一个响应，确认它真的走了网络。
        client.push(
            "https://example.com/dlss.zip",
            Ok(crate::testing::fake_payload(
                "https://example.com/dlss.zip",
                zip_with_files([("nvngx_dlss.dll", b"MZ-good".as_slice())]),
            )),
        );
        let second = cache.ensure(&client, &dlss_spec()).unwrap();
        assert_eq!(second.sha256, first.sha256);
    }

    #[test]
    fn valid_cache_causes_zero_network_requests() {
        let payload = zip_with_files([("nvngx_dlss.dll", b"MZ-good".as_slice())]);
        let client = FakeHttp::new();
        client.push(
            "https://example.com/dlss.zip",
            Ok(crate::testing::fake_payload(
                "https://example.com/dlss.zip",
                payload,
            )),
        );
        let fixture = cache();
        let cache = &fixture.cache;
        cache.ensure(&client, &dlss_spec()).unwrap();
        let calls = client.call_count();
        cache.ensure(&client, &dlss_spec()).unwrap();
        assert_eq!(client.call_count(), calls, "第二次必须走缓存，零网络");
    }

    #[test]
    fn cached_finds_the_exact_version_without_network() {
        let fixture = cache();
        let spec = ArtifactSpec {
            kind: ArtifactKind::ReShade,
            version: "6.8.1".to_string(),
            url: "https://example.com/reshade.dll".to_string(),
            expected_file: "ReShade64.dll".to_string(),
            container: ContainerKind::Direct,
        };
        let client = FakeHttp::single(b"MZ-reshade".to_vec());
        fixture.cache.ensure(&client, &spec).unwrap();

        let got = fixture
            .cache
            .cached(ArtifactKind::ReShade, "6.8.1")
            .expect("这一版应该在缓存里");
        assert_eq!(got.sha256, sha256_bytes(b"MZ-reshade"));
        // 版本对不上就当没有，不退而求其次拿别的版本。
        assert!(
            fixture
                .cache
                .cached(ArtifactKind::ReShade, "6.9.0")
                .is_none()
        );

        // 文件被改坏 → 也当没有。
        std::fs::write(&got.path, b"tampered").unwrap();
        assert!(
            fixture
                .cache
                .cached(ArtifactKind::ReShade, "6.8.1")
                .is_none()
        );
    }

    #[test]
    fn latest_picks_highest_version_and_skips_corrupt() {
        let fixture = cache();
        let cache = &fixture.cache;
        for version in ["1.9.0", "1.10.0", "2.0.0"] {
            let spec = ArtifactSpec {
                kind: ArtifactKind::RenoDxDlss5,
                version: version.to_string(),
                url: format!("https://example.com/{version}.addon64"),
                expected_file: "renodx-dlss5.addon64".to_string(),
                container: ContainerKind::Direct,
            };
            let client = FakeHttp::new();
            client.push(
                &spec.url,
                Ok(crate::testing::fake_payload(
                    &spec.url,
                    format!("MZ-{version}").into_bytes(),
                )),
            );
            cache.ensure(&client, &spec).unwrap();
        }
        let latest = cache.latest(ArtifactKind::RenoDxDlss5).unwrap().unwrap();
        assert_eq!(latest.version, "2.0.0");

        // 把最高版本的文件改坏：latest 应跳过它、退回 1.10.0。
        let corrupt_path = latest.path.clone();
        std::fs::write(&corrupt_path, b"tampered").unwrap();
        let next = cache.latest(ArtifactKind::RenoDxDlss5).unwrap().unwrap();
        assert_eq!(next.version, "1.10.0");
    }
}
