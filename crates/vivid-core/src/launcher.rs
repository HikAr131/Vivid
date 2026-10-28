//! 一次性 Steam 启动。
//!
//! **这个模块刻意没有「写 Steam 配置」的 API。** 红线在 CLAUDE.md 第一节第 3 条：
//! GTA5 只提供一次性 `-nobattleye` 单机启动，不修改 Steam 的永久 Launch Options。
//! 一次性参数用完就没了；写进 Steam 配置的参数会一直生效，用户以后从 Steam
//! 直接启动也会带着它——那已经超出「用户点了一次按钮」的授权范围。
//!
//! 底部有一条源码守门测试：本文件的生产代码里不许出现 Steam 配置相关的字样。
//! 它拦的不是笔误，而是「以后有人图省事把参数写进去」这条路。

use std::path::{Path, PathBuf};

use crate::recipe::LaunchMode;

/// 一条即将执行的启动命令。先构造成数据再执行，方便测试和展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("找不到 Steam 主程序：{0}")]
    SteamMissing(PathBuf),

    #[error("启动失败：{0}")]
    Spawn(String),
}

/// 把固定配方里的启动方式翻译成一条命令。**纯函数，不碰系统。**
pub fn build_command(steam_exe: &Path, mode: &LaunchMode) -> LaunchCommand {
    match mode {
        LaunchMode::Steam { app_id, args } => {
            let mut all = vec!["-applaunch".to_string(), app_id.to_string()];
            all.extend(args.iter().cloned());
            LaunchCommand {
                program: steam_exe.to_path_buf(),
                args: all,
            }
        }
    }
}

/// 执行一次启动。只 spawn，不等待，也不改任何配置。
pub fn launch(steam_exe: &Path, mode: &LaunchMode) -> Result<(), LaunchError> {
    if !steam_exe.is_file() {
        return Err(LaunchError::SteamMissing(steam_exe.to_path_buf()));
    }
    let command = build_command(steam_exe, mode);
    std::process::Command::new(&command.program)
        .args(&command.args)
        .spawn()
        .map(|_| ())
        .map_err(|e| LaunchError::Spawn(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{GameId, recipe_for};

    #[test]
    fn gta_launch_is_one_shot_and_contains_no_config_write() {
        let steam = PathBuf::from(r"C:\Program Files (x86)\Steam\steam.exe");
        let command = build_command(
            &steam,
            &LaunchMode::Steam {
                app_id: 3_240_220,
                args: vec!["-nobattleye".into()],
            },
        );
        assert_eq!(command.program, steam);
        assert_eq!(command.args, ["-applaunch", "3240220", "-nobattleye"]);
    }

    /// 命令必须来自固定配方，不是界面拼出来的。
    #[test]
    fn the_command_comes_from_the_fixed_recipe() {
        let steam = PathBuf::from(r"H:\Steam\steam.exe");
        let gta = recipe_for(GameId::Gta5Enhanced, Path::new(r"H:\gta")).unwrap();
        assert_eq!(
            build_command(&steam, &gta.launch).args,
            ["-applaunch", "3240220", "-nobattleye"]
        );

        // RDR2 正常启动，不加任何未经验证的参数。
        let rdr2 = recipe_for(GameId::RedDeadRedemption2, Path::new(r"H:\rdr2")).unwrap();
        assert_eq!(
            build_command(&steam, &rdr2.launch).args,
            ["-applaunch", "1174180"]
        );
    }

    #[test]
    fn missing_steam_executable_is_reported() {
        let missing = PathBuf::from(r"Z:\nope\steam.exe");
        let mode = LaunchMode::Steam {
            app_id: 3_240_220,
            args: Vec::new(),
        };
        assert!(matches!(
            launch(&missing, &mode),
            Err(LaunchError::SteamMissing(_))
        ));
    }

    /// 源码守门：生产代码里不许出现任何写 Steam 配置的路子。
    /// 只搜 `#[cfg(test)]` 之前那一半，免得搜到本测试自己列的关键词。
    #[test]
    fn launcher_source_never_touches_steam_configuration() {
        let source = include_str!("launcher.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("源码里一定有测试模块之前的部分");
        for forbidden in [
            "appmanifest_",
            "LaunchOptions",
            "localconfig.vdf",
            "set_value",
            "create_subkey",
        ] {
            assert!(
                !production.contains(forbidden),
                "launcher 生产代码里不该出现 {forbidden}"
            );
        }
    }
}
