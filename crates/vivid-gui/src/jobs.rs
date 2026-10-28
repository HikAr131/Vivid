//! 后台任务状态。
//!
//! 两条规则决定了这个模块的形状：
//!
//! 1. **每款游戏同时最多一个任务。** 用户连点两下「安装」，第二下必须被挡住——
//!    两个安装并发跑同一个游戏目录，回滚就再也说不清了。
//! 2. **失败就是失败。** 这里没有任何把 `Err` 兜成 `Ok` 的路径。
//!    主结论只显示错误的第一行摘要，原文留给折叠详情。

use std::collections::BTreeMap;

use vivid_core::recipe::GameId;

/// 后台任务的种类。界面按它决定进度文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Refresh,
    /// 下载并校验组件、生成安装计划。**到这一步为止游戏目录一个字节都没动。**
    Prepare,
    Install,
    Update,
    Repair,
    Restore,
    Launch,
}

impl JobKind {
    /// 进行中时显示的一句中文。
    pub const fn busy_label(self) -> &'static str {
        match self {
            Self::Refresh => "正在检查",
            Self::Prepare => "正在准备",
            Self::Install => "正在安装",
            Self::Update => "正在更新",
            Self::Repair => "正在修复",
            Self::Restore => "正在还原",
            Self::Launch => "正在启动",
        }
    }
}

/// 工作线程发回来的消息。
#[derive(Debug)]
pub enum JobEvent {
    Progress {
        game: GameId,
        message: String,
        percent: f32,
    },
    /// 体检结果（安装、更新之后重新看一遍状态）。
    Inspected {
        game: GameId,
        inspection: Box<vivid_core::manager::GameInspection>,
    },
    /// 计划已经生成好，等用户在预演里确认。**确认之前不写任何东西。**
    Prepared {
        game: GameId,
        /// 用户点的是安装还是更新，确认之后照原样执行。
        intent: JobKind,
        prepared: Box<vivid_core::manager::PreparedInstall>,
    },
    Finished {
        game: GameId,
        result: Result<String, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobBusy;

impl std::fmt::Display for JobBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "这款游戏已经有一个操作在进行中")
    }
}

/// 每款游戏的任务状态。
#[derive(Default)]
pub struct Jobs {
    running: BTreeMap<GameId, JobKind>,
    progress: BTreeMap<GameId, (String, f32)>,
    last: BTreeMap<GameId, Result<String, String>>,
}

impl Jobs {
    /// 开始一个任务。已有任务在跑时拒绝——**这是防连点的唯一闸口**。
    pub fn start(&mut self, game: GameId, kind: JobKind) -> Result<(), JobBusy> {
        if self.running.contains_key(&game) {
            return Err(JobBusy);
        }
        self.running.insert(game, kind);
        self.progress.remove(&game);
        self.last.remove(&game); // 新操作开始，旧结果不该还挂在旁边
        Ok(())
    }

    pub fn is_busy(&self, game: GameId) -> bool {
        self.running.contains_key(&game)
    }

    pub fn kind(&self, game: GameId) -> Option<JobKind> {
        self.running.get(&game).copied()
    }

    pub fn progress(&self, game: GameId) -> Option<(&str, f32)> {
        self.progress
            .get(&game)
            .map(|(message, percent)| (message.as_str(), *percent))
    }

    pub fn finish(&mut self, game: GameId, result: Result<String, String>) {
        self.running.remove(&game);
        self.progress.remove(&game);
        self.last.insert(game, result);
    }

    /// 成功时的原话；失败时是 `None`（失败要走 `last_summary` / `last_error`）。
    pub fn last_message(&self, game: GameId) -> Option<&str> {
        match self.last.get(&game) {
            Some(Ok(message)) => Some(message.as_str()),
            _ => None,
        }
    }

    /// 失败时给用户看的一行摘要（错误原文的第一行）。
    pub fn last_summary(&self, game: GameId) -> Option<&str> {
        self.last_error(game)
            .and_then(|text| text.lines().next())
            .map(str::trim)
    }

    /// 失败原文，进折叠详情。
    pub fn last_error(&self, game: GameId) -> Option<&str> {
        match self.last.get(&game) {
            Some(Err(text)) => Some(text.as_str()),
            _ => None,
        }
    }

    /// 处理一条来自工作线程的消息。`Inspected` 由调用方自己消费。
    pub fn apply(&mut self, event: JobEvent) {
        match event {
            JobEvent::Progress {
                game,
                message,
                percent,
            } => {
                self.progress.insert(game, (message, percent));
            }
            JobEvent::Finished { game, result } => self.finish(game, result),
            JobEvent::Inspected { .. } | JobEvent::Prepared { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_job_disables_a_second_primary_action() {
        let mut jobs = Jobs::default();
        jobs.start(GameId::Gta5Enhanced, JobKind::Install).unwrap();
        assert!(jobs.start(GameId::Gta5Enhanced, JobKind::Install).is_err());
        assert!(jobs.is_busy(GameId::Gta5Enhanced));
        // 另一款游戏不受影响
        assert!(!jobs.is_busy(GameId::RedDeadRedemption2));
        assert!(
            jobs.start(GameId::RedDeadRedemption2, JobKind::Refresh)
                .is_ok()
        );
    }

    #[test]
    fn completed_job_keeps_the_exact_result() {
        let mut jobs = Jobs::default();
        jobs.start(GameId::Gta5Enhanced, JobKind::Refresh).unwrap();
        jobs.finish(GameId::Gta5Enhanced, Ok("扫描完成".into()));
        assert_eq!(jobs.last_message(GameId::Gta5Enhanced), Some("扫描完成"));
        assert!(!jobs.is_busy(GameId::Gta5Enhanced));
        assert_eq!(jobs.last_error(GameId::Gta5Enhanced), None);
    }

    /// 失败就说失败。**不许用宽泛兜底把错误变成成功状态。**
    #[test]
    fn a_failed_job_never_reads_as_success() {
        let mut jobs = Jobs::default();
        jobs.start(GameId::Gta5Enhanced, JobKind::Install).unwrap();
        jobs.finish(
            GameId::Gta5Enhanced,
            Err("组件文件校验不通过，可能没下完\n详细：sha 对不上".into()),
        );
        assert_eq!(jobs.last_message(GameId::Gta5Enhanced), None);
        // 主结论只显示第一行摘要，原文留给折叠详情。
        assert_eq!(
            jobs.last_summary(GameId::Gta5Enhanced),
            Some("组件文件校验不通过，可能没下完")
        );
        assert!(
            jobs.last_error(GameId::Gta5Enhanced)
                .unwrap()
                .contains("sha 对不上")
        );
    }

    #[test]
    fn progress_is_reported_while_running() {
        let mut jobs = Jobs::default();
        jobs.start(GameId::Gta5Enhanced, JobKind::Install).unwrap();
        jobs.apply(JobEvent::Progress {
            game: GameId::Gta5Enhanced,
            message: "正在下载组件".into(),
            percent: 0.4,
        });
        assert_eq!(
            jobs.progress(GameId::Gta5Enhanced),
            Some(("正在下载组件", 0.4))
        );
        jobs.apply(JobEvent::Finished {
            game: GameId::Gta5Enhanced,
            result: Ok("安装完成".into()),
        });
        assert!(!jobs.is_busy(GameId::Gta5Enhanced));
        assert_eq!(jobs.progress(GameId::Gta5Enhanced), None);
    }

    /// 新任务开始时清掉上一次的结果，免得旧提示挂在新操作旁边。
    #[test]
    fn starting_a_job_clears_the_previous_result() {
        let mut jobs = Jobs::default();
        jobs.start(GameId::Gta5Enhanced, JobKind::Install).unwrap();
        jobs.finish(GameId::Gta5Enhanced, Ok("安装完成".into()));
        jobs.start(GameId::Gta5Enhanced, JobKind::Restore).unwrap();
        assert_eq!(jobs.last_message(GameId::Gta5Enhanced), None);
        assert_eq!(jobs.kind(GameId::Gta5Enhanced), Some(JobKind::Restore));
    }
}
