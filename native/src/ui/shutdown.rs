//! 退出时先结束守护和旧保存，再串行持久化最终草稿，避免旧 ACK 覆盖最新修改。

use crate::{
    config::{GuiOverrides, PrimaryPatch, PrimarySaveOutcome, PrimarySettings},
    daemon::Worker,
    platform,
};
use anyhow::{Result, bail};
use std::{path::PathBuf, sync::mpsc, thread};

#[derive(Clone)]
pub(super) struct Plan {
    pub(super) primary: Option<(PrimaryPatch, PrimarySettings)>,
    pub(super) overrides: Option<GuiOverrides>,
    // 仅由用户明确切换自启产生；generation 防止迟到结果覆盖新选择。
    pub(super) autostart: Option<(bool, u64)>,
}

pub(super) struct Outcome {
    pub(super) primary_saved: bool,
    pub(super) settings_saved: bool,
    pub(super) autostart: Option<(bool, u64)>,
    pub(super) error: Option<String>,
}

pub(super) fn run_with(
    plan: &Plan,
    preview: bool,
    finish_worker: impl FnOnce(),
    mut save_primary: impl FnMut(&PrimaryPatch) -> Result<PrimarySaveOutcome>,
    mut save: impl FnMut(&GuiOverrides) -> Result<()>,
    mut autostart: impl FnMut(bool) -> Result<()>,
) -> Outcome {
    finish_worker();
    let mut outcome = Outcome {
        primary_saved: false,
        settings_saved: false,
        autostart: None,
        error: None,
    };
    if let Some((patch, _)) = &plan.primary {
        if !preview {
            match save_primary(patch) {
                Ok(saved) => {
                    outcome.primary_saved = true;
                    if let Some(warning) = saved.warning {
                        outcome.error = Some(warning.into());
                        return outcome;
                    }
                }
                Err(error) => {
                    // 显示首层脱敏说明，不展开解析输入。
                    outcome.error = Some(format!(
                        "退出前主网络保存失败：{error}。可重试保存，或明确放弃修改退出"
                    ));
                    return outcome;
                }
            }
        }
        outcome.primary_saved = true;
    }
    if let Some(overrides) = &plan.overrides {
        if !preview && save(overrides).is_err() {
            outcome.error =
                Some("退出前保存设置失败；请检查用户数据目录权限后重试，或明确放弃修改退出".into());
            return outcome;
        }
        outcome.settings_saved = true;
    }
    if let Some((enabled, generation)) = plan.autostart {
        if !preview && autostart(enabled).is_err() {
            outcome.error =
                Some("设置已保存，但退出前修改自启失败；请重试，或放弃未完成的修改退出".into());
            return outcome;
        }
        outcome.autostart = Some((enabled, generation));
    }
    outcome
}

pub(super) fn launch(
    worker: Option<Worker>,
    plan: Plan,
    preview: bool,
    config_path: PathBuf,
) -> (mpsc::Receiver<Outcome>, thread::JoinHandle<()>) {
    // 先设置取消标志，不让等待线程调度期间继续后续认证步骤。
    if let Some(worker) = &worker {
        worker.request_stop();
    }
    let (sender, receiver) = mpsc::channel();
    let thread = thread::spawn(move || {
        let outcome = run_with(
            &plan,
            preview,
            || {
                if let Some(worker) = worker {
                    worker.join();
                }
            },
            |patch| patch.save_to(&config_path),
            GuiOverrides::save,
            |enabled| {
                platform::set_autostart_with_config(enabled, &config_path)?;
                if platform::autostart_enabled() != enabled {
                    bail!("自启状态未生效");
                }
                Ok(())
            },
        );
        let _ = sender.send(outcome);
    });
    (receiver, thread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn plan() -> Plan {
        Plan {
            primary: None,
            overrides: Some(GuiOverrides {
                fallback_ssid: Some("Latest_Synthetic_WiFi".into()),
                ..GuiOverrides::default()
            }),
            autostart: None,
        }
    }

    #[test]
    fn final_draft_is_written_after_all_older_worker_writes_finish() {
        let stored = RefCell::new(String::new());
        let order = RefCell::new(Vec::new());
        let result = run_with(
            &plan(),
            false,
            || {
                *stored.borrow_mut() = "Older_Synthetic_WiFi".into();
                order.borrow_mut().push("join");
            },
            |_| panic!("没有主网络草稿时不得写主网络配置"),
            |overrides| {
                *stored.borrow_mut() = overrides.fallback_ssid.clone().unwrap();
                order.borrow_mut().push("save");
                Ok(())
            },
            |_| panic!("未明确修改自启时不得调用平台"),
        );
        assert_eq!(*stored.borrow(), "Latest_Synthetic_WiFi");
        assert_eq!(*order.borrow(), ["join", "save"]);
        assert!(result.settings_saved && result.error.is_none());
        assert!(!result.primary_saved);
    }

    #[test]
    fn preview_never_calls_persistence_or_autostart() {
        let mut plan = plan();
        plan.primary = Some(primary_draft());
        plan.autostart = Some((true, 1));
        let result = run_with(
            &plan,
            true,
            || {},
            |_| panic!("预览不得写主网络配置"),
            |_| panic!("预览不得写文件"),
            |_| panic!("预览不得修改自启"),
        );
        assert!(result.error.is_none());
        assert!(result.primary_saved && result.settings_saved);
        assert_eq!(result.autostart, Some((true, 1)));
    }

    #[test]
    fn failure_is_reported_and_does_not_attempt_later_side_effects() {
        let mut plan = plan();
        plan.autostart = Some((true, 1));
        let result = run_with(
            &plan,
            false,
            || {},
            |_| panic!("没有主网络草稿时不得写主网络配置"),
            |_| bail!("synthetic save failure"),
            |_| panic!("保存失败后不得修改自启"),
        );
        assert!(result.error.as_deref().unwrap().contains("保存设置失败"));
        assert!(!result.settings_saved);
    }

    fn primary_draft() -> (PrimaryPatch, PrimarySettings) {
        let mut settings = PrimarySettings::from_config(&crate::config::AppConfig::default());
        settings.ssid = "Latest_Synthetic_Campus".into();
        settings.username = "synthetic.user".into();
        let patch = PrimaryPatch {
            ssid: Some(settings.ssid.clone()),
            username: Some(settings.username.clone()),
            ..PrimaryPatch::default()
        };
        (patch, settings)
    }

    #[test]
    fn final_primary_gui_and_autostart_writes_follow_worker_completion_in_order() {
        let mut plan = plan();
        plan.primary = Some(primary_draft());
        plan.autostart = Some((true, 7));
        let stored_primary = RefCell::new(String::new());
        let order = RefCell::new(Vec::new());
        let result = run_with(
            &plan,
            false,
            || {
                *stored_primary.borrow_mut() = "Older_Synthetic_Campus".into();
                order.borrow_mut().push("join");
            },
            |patch| {
                *stored_primary.borrow_mut() = patch.ssid.clone().unwrap();
                order.borrow_mut().push("primary");
                Ok(PrimarySaveOutcome::default())
            },
            |_| {
                assert_eq!(*stored_primary.borrow(), "Latest_Synthetic_Campus");
                order.borrow_mut().push("gui");
                Ok(())
            },
            |enabled| {
                assert!(enabled);
                order.borrow_mut().push("autostart");
                Ok(())
            },
        );
        assert_eq!(*order.borrow(), ["join", "primary", "gui", "autostart"]);
        assert!(result.primary_saved && result.settings_saved && result.error.is_none());
        assert_eq!(result.autostart, Some((true, 7)));
    }

    #[test]
    fn primary_failure_stops_gui_and_autostart_writes() {
        let mut plan = plan();
        plan.primary = Some(primary_draft());
        plan.autostart = Some((true, 8));
        let order = RefCell::new(Vec::new());
        let result = run_with(
            &plan,
            false,
            || order.borrow_mut().push("join"),
            |_| {
                order.borrow_mut().push("primary");
                bail!("synthetic failure")
            },
            |_| panic!("主网络失败后不得写 GUI 配置"),
            |_| panic!("主网络失败后不得修改自启"),
        );
        assert_eq!(*order.borrow(), ["join", "primary"]);
        assert!(!result.primary_saved && !result.settings_saved);
        assert!(result.autostart.is_none());
        assert!(result.error.as_deref().unwrap().contains("主网络保存失败"));
    }

    #[test]
    fn gui_failure_retains_primary_success_but_does_not_change_autostart() {
        let mut plan = plan();
        plan.primary = Some(primary_draft());
        plan.autostart = Some((false, 9));
        let result = run_with(
            &plan,
            false,
            || {},
            |_| Ok(PrimarySaveOutcome::default()),
            |_| bail!("synthetic GUI failure"),
            |_| panic!("GUI 失败后不得修改自启"),
        );
        assert!(result.primary_saved && !result.settings_saved);
        assert!(result.autostart.is_none());
        assert!(result.error.is_some());
    }

    #[test]
    fn post_commit_cleanup_warning_acknowledges_primary_before_pausing_exit() {
        let mut plan = plan();
        plan.primary = Some(primary_draft());
        let result = run_with(
            &plan,
            false,
            || {},
            |_| {
                Ok(PrimarySaveOutcome {
                    warning: Some("已保存，私有旧配置清理失败"),
                })
            },
            |_| panic!("警告须先可见反馈"),
            |_| panic!("警告期间不修改自启"),
        );
        assert!(result.primary_saved);
        assert!(!result.settings_saved);
        assert!(result.error.as_deref().unwrap().contains("清理失败"));
    }
}
