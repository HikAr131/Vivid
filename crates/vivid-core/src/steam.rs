//! Steam 库的发现。
//!
//! 路径来源有两处，都要试：注册表里的安装位置，以及 `libraryfolders.vdf` 里
//! 用户自己加的其它盘符。本机就有三个库（H:/D:/F:），只读注册表会漏掉两个。

use std::path::{Path, PathBuf};

/// Steam 主安装目录。读不到就返回 `None`——没装 Steam 是正常情况，不是错误。
#[cfg(windows)]
pub fn install_path() -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    // Steam 客户端是 32 位程序，64 位系统上它的键在 WOW6432Node 下。
    // 两个位置都试一遍，别赌系统位数。
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for sub in [r"SOFTWARE\WOW6432Node\Valve\Steam", r"SOFTWARE\Valve\Steam"] {
        if let Ok(key) = hklm.open_subkey(sub)
            && let Ok(p) = key.get_value::<String, _>("InstallPath")
        {
            return Some(PathBuf::from(p));
        }
    }

    // 退路：机器级的键有时缺失（绿色安装、多用户），用户级的 SteamPath 更可靠。
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey(r"Software\Valve\Steam")
        && let Ok(p) = key.get_value::<String, _>("SteamPath")
    {
        return Some(PathBuf::from(p));
    }

    None
}

#[cfg(not(windows))]
pub fn install_path() -> Option<PathBuf> {
    None
}

/// 所有 Steam 库根目录（主库 + 用户添加的其它盘）。
pub fn libraries() -> Vec<PathBuf> {
    let Some(root) = install_path() else {
        return Vec::new();
    };

    let mut out = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf"))
        .map(|text| parse_library_paths(&text))
        .unwrap_or_default();

    // vdf 读不到或格式变了，至少还有主库可用——降级而不是整个功能失效。
    if !out.iter().any(|p| p == &root) {
        out.insert(0, root);
    }

    out.retain(|p| p.is_dir());
    out.dedup();
    out
}

/// 各库里存放游戏的 `steamapps/common` 目录，只返回真实存在的。
pub fn game_dirs() -> Vec<PathBuf> {
    libraries()
        .into_iter()
        .map(|l| l.join("steamapps").join("common"))
        .filter(|p| p.is_dir())
        .collect()
}

/// 从 `libraryfolders.vdf` 文本里抽出库路径。
///
/// 做成纯函数是为了能脱离本机 Steam 测试——VDF 是 Valve 的私有格式，
/// 改过好几版，值得有测试盯着。
pub fn parse_library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("\"path\"")?;
            let start = rest.find('"')? + 1;
            let rest = &rest[start..];
            let end = rest.find('"')?;
            let raw = &rest[..end];
            // VDF 里反斜杠是转义的：`H:\Steam` 表示 `H:\Steam`
            Some(PathBuf::from(raw.replace(r"\\", r"\")))
        })
        .collect()
}

/// 从游戏目录路径反推游戏名——就是 `common/` 下那一层目录名。
pub fn game_name_of(path: &Path, common_dir: &Path) -> Option<String> {
    path.strip_prefix(common_dir)
        .ok()?
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
}

/// 一个 `appmanifest_<id>.acf` 里解析出的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAppManifest {
    pub app_id: u32,
    pub name: String,
    pub install_dir: String,
}

/// 一款已安装的 Steam 游戏：清单字段 + 拼好的磁盘位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamApp {
    pub app_id: u32,
    pub name: String,
    pub install_dir: String,
    /// `steamapps/common/<installdir>`，已验证存在。
    pub game_root: PathBuf,
    /// Steam 主程序，用于 `-applaunch` 启动。
    pub steam_exe: PathBuf,
}

/// 解析 `appmanifest_<id>.acf` 的文本。
///
/// 只读 Vivid 关心的三个字段；缺任何一个或 appid 读不出来就返回 `None`。
/// ACF 是 Valve 的私有格式，历史上改过好几版——解析失败是正常情况，
/// 不该 panic，静默当「这个 app 不存在」处理。
pub fn parse_app_manifest(text: &str) -> Option<ParsedAppManifest> {
    let field = |key: &str| -> Option<String> {
        text.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(&format!("\"{key}\""))?;
            let rest = rest.trim_start();
            let rest = rest.strip_prefix('"')?;
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
    };

    let app_id = field("appid")?.parse().ok()?;
    let name = field("name")?;
    let install_dir = field("installdir")?;
    Some(ParsedAppManifest {
        app_id,
        name,
        install_dir,
    })
}

/// 按 App ID 找一款已安装的 Steam 游戏。
///
/// 只读各库的 `steamapps/appmanifest_<id>.acf`，拼接 `steamapps/common/<installdir>`，
/// 要求游戏目录与 steam.exe 都真实存在——**不递归扫整个磁盘**。
pub fn find_app(app_id: u32) -> Option<SteamApp> {
    let steam_root = install_path()?;
    let steam_exe = steam_root.join("steam.exe");
    if !steam_exe.is_file() {
        return None;
    }

    for library in libraries() {
        let manifest = library
            .join("steamapps")
            .join(format!("appmanifest_{app_id}.acf"));
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let Some(parsed) = parse_app_manifest(&text) else {
            continue;
        };
        if parsed.app_id != app_id {
            continue;
        }
        let game_root = library
            .join("steamapps")
            .join("common")
            .join(&parsed.install_dir);
        if game_root.is_dir() {
            return Some(SteamApp {
                app_id,
                name: parsed.name,
                install_dir: parsed.install_dir,
                game_root,
                steam_exe,
            });
        }
    }
    None
}

/// 本机 Steam 库里所有已安装的游戏。
///
/// 只读各库 `steamapps` 下的 `appmanifest_*.acf`，**不递归扫磁盘**。
/// 没装 Steam、库目录读不动、清单坏了——一律跳过不报错：
/// 用户库里有半装的、正在下载的、被手动删掉目录的游戏，这些都很正常。
pub fn all_apps() -> Vec<SteamApp> {
    let Some(steam_root) = install_path() else {
        return Vec::new();
    };
    let steam_exe = steam_root.join("steam.exe");
    if !steam_exe.is_file() {
        return Vec::new();
    }
    apps_in_libraries(&libraries(), &steam_exe)
}

/// [`all_apps`] 的纯函数版本：库目录和 steam.exe 都由调用方给。
///
/// 拆出来是为了能脱离本机 Steam 测试——自动测试造一个临时目录当假库，
/// 不读注册表、不碰真实 Steam。
pub fn apps_in_libraries(libraries: &[PathBuf], steam_exe: &Path) -> Vec<SteamApp> {
    let mut apps: Vec<SteamApp> = Vec::new();
    for library in libraries {
        let steamapps = library.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Some(parsed) = parse_app_manifest(&text) else {
                continue;
            };
            let game_root = steamapps.join("common").join(&parsed.install_dir);
            if !game_root.is_dir() {
                continue; // 清单还在但目录没了：当作没装
            }
            apps.push(SteamApp {
                app_id: parsed.app_id,
                name: parsed.name,
                install_dir: parsed.install_dir,
                game_root,
                steam_exe: steam_exe.to_path_buf(),
            });
        }
    }
    // 同一个 App ID 在多个库里各有一份清单是可能的（搬过库、残留）。
    // 排序 + 去重让结果稳定，界面每次刷新的顺序才不会跳。
    apps.sort_by(|a, b| a.app_id.cmp(&b.app_id).then_with(|| a.name.cmp(&b.name)));
    apps.dedup_by_key(|app| app.app_id);
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取自本机真实的 libraryfolders.vdf（2026-08-31），只截了相关字段。
    const REAL_VDF: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"H:\Steam"
		"label"		""
		"contentid"		"123456"
	}
	"1"
	{
		"path"		"D:\SteamLibrary"
		"label"		""
	}
	"2"
	{
		"path"		"F:\SteamLibrary"
		"label"		""
	}
}
"#;

    #[test]
    fn parses_all_three_local_libraries() {
        let paths = parse_library_paths(REAL_VDF);
        assert_eq!(
            paths,
            vec![
                PathBuf::from(r"H:\Steam"),
                PathBuf::from(r"D:\SteamLibrary"),
                PathBuf::from(r"F:\SteamLibrary"),
            ]
        );
    }

    /// 反斜杠必须反转义，否则拼出来的路径根本不存在，扫描会静默扫不到东西。
    #[test]
    fn unescapes_backslashes() {
        let v = parse_library_paths(r#"		"path"		"C:\Program Files\Steam""#);
        assert_eq!(v, vec![PathBuf::from(r"C:\Program Files\Steam")]);
    }

    #[test]
    fn ignores_other_keys() {
        let v = parse_library_paths(
            r#"
		"label"		"not a path"
		"contentid"		"999"
"#,
        );
        assert!(v.is_empty(), "只有 path 键该被采纳，实际取到 {v:?}");
    }

    #[test]
    fn tolerates_garbage_without_panicking() {
        for junk in ["", "{{{", "\"path\"", "\"path\"  \"unterminated"] {
            let _ = parse_library_paths(junk);
        }
    }

    #[test]
    fn derives_game_name_from_path() {
        let common = Path::new(r"H:\Steam\steamapps\common");
        let dll = Path::new(r"H:\Steam\steamapps\common\Red Dead Redemption 2\x64\nvngx_dlss.dll");
        assert_eq!(
            game_name_of(dll, common).as_deref(),
            Some("Red Dead Redemption 2")
        );
    }

    #[test]
    fn parses_supported_app_manifest() {
        let text = r#"
    "AppState"
    {
        "appid" "3240220"
        "name" "Grand Theft Auto V Enhanced"
        "installdir" "Grand Theft Auto V Enhanced"
    }"#;
        let app = parse_app_manifest(text).unwrap();
        assert_eq!(app.app_id, 3_240_220);
        assert_eq!(app.install_dir, "Grand Theft Auto V Enhanced");
    }

    #[test]
    fn parses_rdr2_app_manifest() {
        let text = r#"
    "AppState"
    {
        "appid" "1174180"
        "name" "Red Dead Redemption 2"
        "installdir" "Red Dead Redemption 2"
    }"#;
        let app = parse_app_manifest(text).unwrap();
        assert_eq!(app.app_id, 1_174_180);
        assert_eq!(app.install_dir, "Red Dead Redemption 2");
    }

    /// 造一个假 Steam 库：`<root>/steamapps/appmanifest_<id>.acf` +
    /// `<root>/steamapps/common/<installdir>/`。
    fn fake_library(root: &Path, apps: &[(u32, &str, &str)]) {
        let steamapps = root.join("steamapps");
        std::fs::create_dir_all(steamapps.join("common")).unwrap();
        for (app_id, name, install_dir) in apps {
            std::fs::write(
                steamapps.join(format!("appmanifest_{app_id}.acf")),
                format!(
                    "\"AppState\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\
                     \t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{install_dir}\"\n}}"
                ),
            )
            .unwrap();
            std::fs::create_dir_all(steamapps.join("common").join(install_dir)).unwrap();
        }
    }

    /// 多个库目录里的游戏都要被发现，**全程不读真实 Steam**。
    #[test]
    fn every_library_is_scanned() {
        let temp = tempfile::tempdir().unwrap();
        let steam_exe = temp.path().join("steam.exe");
        std::fs::write(&steam_exe, b"MZ").unwrap();
        let one = temp.path().join("Steam");
        let two = temp.path().join("SteamLibrary");
        fake_library(&one, &[(3_240_220, "GTA V Enhanced", "GTAV Enhanced")]);
        fake_library(&two, &[(1_091_500, "Cyberpunk 2077", "Cyberpunk 2077")]);

        let apps = apps_in_libraries(&[one, two], &steam_exe);
        let ids: Vec<u32> = apps.iter().map(|app| app.app_id).collect();
        assert_eq!(ids, vec![1_091_500, 3_240_220]);
        assert!(apps[0].game_root.ends_with("Cyberpunk 2077"));
        assert_eq!(apps[0].steam_exe, steam_exe);
    }

    /// 清单还在但目录没了（手动删过、正在下载）——跳过，不当错误。
    #[test]
    fn a_manifest_without_its_directory_is_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("Steam");
        fake_library(&library, &[(1_091_500, "Cyberpunk 2077", "Cyberpunk 2077")]);
        std::fs::remove_dir(
            library
                .join("steamapps")
                .join("common")
                .join("Cyberpunk 2077"),
        )
        .unwrap();
        assert!(apps_in_libraries(&[library], &temp.path().join("steam.exe")).is_empty());
    }

    /// 坏清单、无关文件、不存在的库目录都不能让整次发现失败。
    #[test]
    fn broken_manifests_do_not_abort_the_scan() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("Steam");
        fake_library(&library, &[(1_091_500, "Cyberpunk 2077", "Cyberpunk 2077")]);
        let steamapps = library.join("steamapps");
        std::fs::write(steamapps.join("appmanifest_999.acf"), b"{ not acf").unwrap();
        std::fs::write(steamapps.join("readme.txt"), b"ignore me").unwrap();

        let missing = temp.path().join("NoSuchLibrary");
        let apps = apps_in_libraries(&[library, missing], &temp.path().join("steam.exe"));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].app_id, 1_091_500);
    }

    /// 同一个 App ID 在两个库里各有一份清单（搬过库的残留）只算一次。
    #[test]
    fn a_duplicated_app_id_is_listed_once() {
        let temp = tempfile::tempdir().unwrap();
        let one = temp.path().join("A");
        let two = temp.path().join("B");
        fake_library(&one, &[(1_091_500, "Cyberpunk 2077", "Cyberpunk 2077")]);
        fake_library(&two, &[(1_091_500, "Cyberpunk 2077", "Cyberpunk 2077")]);
        let apps = apps_in_libraries(&[one, two], &temp.path().join("steam.exe"));
        assert_eq!(apps.len(), 1);
    }

    #[test]
    fn broken_manifests_return_none() {
        for bad in [
            "",
            "not acf at all",
            r#""AppState" { "appid" "abc" }"#,
            r#""AppState" { "appid" "3240220" "name" "X" }"#, // 缺 installdir
        ] {
            assert_eq!(parse_app_manifest(bad), None, "{bad}");
        }
    }
}
