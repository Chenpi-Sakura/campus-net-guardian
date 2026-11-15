//! Iced 单窗面板；后台串行执行网络工作，预览与真实配置完全隔离。
mod appearance;
mod autosave;
mod motion;
mod scroll_edge;
mod shutdown;
mod smooth_scroll;
mod tray;
mod view;
mod window_icons;

use crate::{
    config::{
        self, AppConfig, Appearance, ColorOverrides, GuiOverrides, PrimaryPatch, PrimarySettings,
        ThemeMode, ThemeStyle,
    },
    daemon::{Command, Event, Snapshot, Status, Worker},
    platform,
};
use anyhow::{Context, Result, bail};
use autosave::Autosave;
use iced::{Element, Font, Size, Subscription, Task, Theme, window};
use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Settings,
}
macro_rules! choices {
    ($name:ident, $ty:ty, [$($value:literal),+], $unit:literal) => {
        #[derive(Clone, Copy, PartialEq, Eq)] struct $name($ty);
        impl $name { const ALL: &'static [Self] = &[$(Self($value)),+]; }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                if Self::ALL.contains(self) {write!(f, concat!("{}", $unit), self.0)}
                else {write!(f, concat!("{}", $unit, "（配置文件）"), self.0)}
            }
        }
    }
}
choices!(IntervalChoice, u64, [15, 30, 60, 120], " 秒");
choices!(ThresholdChoice, u32, [2, 3, 5], " 次");
#[derive(Clone, Copy, PartialEq, Eq)]
enum ServiceChoice {
    Mobile,
    Telecom,
    Unicom,
    Unknown,
}
impl ServiceChoice {
    const ALL: &'static [Self] = &[Self::Mobile, Self::Telecom, Self::Unicom];
    fn code(self) -> &'static str {
        match self {
            Self::Mobile => "cmcc",
            Self::Telecom => "ctcc",
            Self::Unicom => "unicom",
            Self::Unknown => "",
        }
    }
    fn from_code(code: &str) -> Self {
        match code {
            "ctcc" => Self::Telecom,
            "unicom" => Self::Unicom,
            "cmcc" => Self::Mobile,
            _ => Self::Unknown,
        }
    }
}
impl fmt::Display for ServiceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mobile => "中国移动",
            Self::Telecom => "中国电信",
            Self::Unicom => "中国联通",
            Self::Unknown => "请选择运营商",
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct RecoveryChoice(u64);
impl RecoveryChoice {
    const ALL: &'static [Self] = &[Self(0), Self(300), Self(900)];
}
impl fmt::Display for RecoveryChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => f.write_str("手动切回"),
            300 => f.write_str("每 5 分钟尝试"),
            900 => f.write_str("每 15 分钟尝试"),
            n => write!(f, "每 {n} 秒（配置文件）"),
        }
    }
}
struct Form {
    primary_ssid: String,
    username: String,
    account_password: String,
    account_password_visible: bool,
    service: ServiceChoice,
    original_service: String,
    entry_url: String,
    api_base: String,
    probe_url: String,
    autostart: bool,
    fallback_enabled: bool,
    ssid: String,
    password: String,
    interval: IntervalChoice,
    threshold: ThresholdChoice,
    recovery: RecoveryChoice,
    debug: bool,
    password_visible: bool,
    mode: ThemeMode,
    style: ThemeStyle,
    accent: String,
    background: String,
    surface: String,
    text: String,
    appearance: Appearance,
}
impl Form {
    fn new(config: &AppConfig, appearance: Appearance, autostart: bool, system_dark: bool) -> Self {
        let [accent, background, surface, text] = appearance::fields(&appearance, system_dark);
        Self {
            primary_ssid: config.wifi.ssid.clone(),
            username: config.account.username.clone(),
            account_password: config.account.password.clone(),
            account_password_visible: false,
            service: ServiceChoice::from_code(&config.portal.service),
            original_service: config.portal.service.clone(),
            entry_url: config.portal.entry_url.clone(),
            api_base: config.portal.api_base.clone(),
            probe_url: config.portal.probe_url.clone(),
            autostart,
            fallback_enabled: config.wifi.fallback.enabled,
            ssid: config.wifi.fallback.ssid.clone(),
            password: config.wifi.fallback.password.clone(),
            interval: IntervalChoice(config.daemon.probe_interval),
            threshold: ThresholdChoice(config.daemon.failure_threshold),
            recovery: RecoveryChoice(config.wifi.fallback.switch_back_interval),
            debug: config.logging.level == "DEBUG",
            password_visible: false,
            mode: appearance.mode,
            style: appearance.style,
            accent,
            background,
            surface,
            text,
            appearance,
        }
    }
    fn load_colors(&mut self, system_dark: bool) {
        [self.accent, self.background, self.surface, self.text] =
            appearance::fields(&self.appearance, system_dark);
    }
    fn primary_settings(&self) -> PrimarySettings {
        PrimarySettings {
            ssid: self.primary_ssid.clone(),
            username: self.username.trim().into(),
            password: self.account_password.clone(),
            service: if self.service == ServiceChoice::Unknown {
                self.original_service.clone()
            } else {
                self.service.code().into()
            },
            entry_url: self.entry_url.trim().into(),
            api_base: self.api_base.trim().into(),
            probe_url: self.probe_url.trim().into(),
        }
    }
    fn validate_primary(&self, config: &AppConfig) -> Result<()> {
        let mut candidate = config.clone();
        let fields = self.primary_settings();
        candidate.wifi.ssid = fields.ssid;
        candidate.account.username = fields.username;
        candidate.account.password = fields.password;
        candidate.portal.service = fields.service;
        candidate.portal.entry_url = fields.entry_url;
        candidate.portal.api_base = fields.api_base;
        candidate.portal.probe_url = fields.probe_url;
        candidate.validate()
    }
    fn capture_colors(&mut self, system_dark: bool) -> Result<()> {
        let mut next = self.appearance.clone();
        let dark = appearance::is_dark(&next, system_dark);
        *next.colors_mut(dark) = ColorOverrides {
            accent_color: Some(self.accent.clone()),
            background_color: Some(self.background.clone()),
            surface_color: Some(self.surface.clone()),
            text_color: Some(self.text.clone()),
        };
        next.validate()?;
        self.appearance = next;
        Ok(())
    }
    fn overrides(&self, previous: &GuiOverrides, config: &AppConfig) -> GuiOverrides {
        GuiOverrides {
            probe_interval: if IntervalChoice::ALL.contains(&self.interval) {
                Some(self.interval.0)
            } else {
                previous.probe_interval
            },
            failure_threshold: if ThresholdChoice::ALL.contains(&self.threshold) {
                Some(self.threshold.0)
            } else {
                previous.failure_threshold
            },
            switch_back_interval: if RecoveryChoice::ALL.contains(&self.recovery) {
                Some(self.recovery.0)
            } else {
                previous.switch_back_interval
            },
            fallback_enabled: Some(self.fallback_enabled),
            fallback_ssid: Some(self.ssid.trim().into()),
            fallback_password: Some(self.password.clone()),
            log_level: if self.debug {
                Some("DEBUG".into())
            } else if config.logging.level == "DEBUG" {
                Some("INFO".into())
            } else {
                previous.log_level.clone()
            },
            appearance: Some(self.appearance.clone()),
        }
    }

    fn prepared_overrides(
        &mut self,
        previous: &GuiOverrides,
        config: &AppConfig,
        base: &AppConfig,
        system_dark: bool,
    ) -> Result<GuiOverrides> {
        self.capture_colors(system_dark)?;
        appearance::validate(&self.appearance)?;
        let ssid = self.ssid.trim();
        if ssid.len() > 32 || ssid.contains('\0') {
            bail!("备用 WiFi 名称必须最多 32 字节且不含 NUL");
        }
        if self.fallback_enabled && (ssid.is_empty() || ssid == "__FALLBACK_WIFI_SSID__") {
            bail!("请先填写备用 WiFi 名称，再开启备用网络");
        }
        let overrides = self.overrides(previous, config);
        if base.validate().is_ok() {
            base.effective(&overrides).validate()?;
        }
        Ok(overrides)
    }
}

struct PendingSave {
    overrides: GuiOverrides,
    primary_settings: PrimarySettings,
    autostart: Option<(bool, u64)>,
    revision: u64,
}

#[derive(Clone)]
enum Message {
    Tick,
    Opened(window::Id),
    CloseRequested(window::Id),
    SystemTheme(iced::theme::Mode),
    Scale(f32),
    IconsReady,
    Login,
    TogglePause,
    OpenSettings,
    Back,
    PrimarySsid(String),
    Username(String),
    AccountPassword(String),
    ToggleAccountPassword,
    Service(ServiceChoice),
    EntryUrl(String),
    ApiBase(String),
    ProbeUrl(String),
    TogglePortal,
    Autostart(bool),
    Fallback(bool),
    Ssid(String),
    Password(String),
    TogglePassword,
    Recovery(RecoveryChoice),
    Interval(IntervalChoice),
    Threshold(ThresholdChoice),
    Debug(bool),
    Scan,
    OpenConfig,
    OpenLog,
    RequestExit,
    CancelExit,
    ConfirmExit,
    DiscardAndExit,
    Mode(ThemeMode),
    Style(ThemeStyle),
    Accent(String),
    Background(String),
    Surface(String),
    TextColor(String),
    ResetColors,
    ToggleColors,
}
// Iced 调试输出不得携带备用网络名称和密码。
impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tick => "Tick",
            Self::Opened(_) => "Opened",
            Self::CloseRequested(_) => "CloseRequested",
            Self::SystemTheme(_) => "SystemTheme",
            Self::Scale(_) => "Scale",
            Self::IconsReady => "IconsReady",
            Self::Login => "Login",
            Self::TogglePause => "TogglePause",
            Self::OpenSettings => "OpenSettings",
            Self::Back => "Back",
            Self::PrimarySsid(_) => "PrimarySsid([REDACTED])",
            Self::Username(_) => "Username([REDACTED])",
            Self::AccountPassword(_) => "AccountPassword([REDACTED])",
            Self::ToggleAccountPassword => "ToggleAccountPassword",
            Self::Service(_) => "Service",
            Self::EntryUrl(_) => "EntryUrl([REDACTED])",
            Self::ApiBase(_) => "ApiBase([REDACTED])",
            Self::ProbeUrl(_) => "ProbeUrl([REDACTED])",
            Self::TogglePortal => "TogglePortal",
            Self::Autostart(_) => "Autostart",
            Self::Fallback(_) => "Fallback",
            Self::Ssid(_) => "Ssid([REDACTED])",
            Self::Password(_) => "Password([REDACTED])",
            Self::TogglePassword => "TogglePassword",
            Self::Recovery(_) => "Recovery",
            Self::Interval(_) => "Interval",
            Self::Threshold(_) => "Threshold",
            Self::Debug(_) => "Debug",
            Self::Scan => "Scan",
            Self::OpenConfig => "OpenConfig",
            Self::OpenLog => "OpenLog",
            Self::RequestExit => "RequestExit",
            Self::CancelExit => "CancelExit",
            Self::ConfirmExit => "ConfirmExit",
            Self::DiscardAndExit => "DiscardAndExit",
            Self::Mode(_) => "Mode",
            Self::Style(_) => "Style",
            Self::Accent(_) => "Accent",
            Self::Background(_) => "Background",
            Self::Surface(_) => "Surface",
            Self::TextColor(_) => "TextColor",
            Self::ResetColors => "ResetColors",
            Self::ToggleColors => "ToggleColors",
        })
    }
}
struct App {
    page: Page,
    snapshot: Snapshot,
    startup_error: Option<String>,
    logs: VecDeque<String>,
    effective: AppConfig,
    form: Form,
    networks: Vec<String>,
    networks_revision: u64,
    pending: bool,
    feedback: Option<String>,
    preview: bool,
    exit_confirm: bool,
    exit_error: Option<String>,
    discard_on_exit: bool,
    logo: iced::widget::image::Handle,
    advanced_colors: bool,
    advanced_portal: bool,
    primary_saved: PrimarySettings,
    base: AppConfig,
    overrides: GuiOverrides,
    system_dark: bool,
    worker: Option<Worker>,
    tray: Option<tray::Tray>,
    _guard: tray::InstanceGuard,
    window: window::Id,
    visible: bool,
    config_path: PathBuf,
    saving: Option<PendingSave>,
    save_warning: Option<&'static str>,
    autostart_requested: Option<(bool, u64)>,
    autostart_intent: Option<(bool, u64)>,
    autostart_generation: u64,
    autosave: Autosave,
    shutdown: Option<mpsc::Receiver<shutdown::Outcome>>,
    exit_plan: Option<shutdown::Plan>,
    shutdown_thread: Option<std::thread::JoinHandle<()>>,
    preview_login: Option<Instant>,
    icon_scale: f32,
    logo_color: [u8; 3],
    cached_theme: Theme,
    theme_appearance: Appearance,
    theme_dark: bool,
}
impl App {
    fn refresh_native(&mut self) -> Task<Message> {
        let colors = &self.form.appearance;
        if self.theme_appearance != *colors || self.theme_dark != self.system_dark {
            let candidate = appearance::theme(colors, self.system_dark);
            let palette = candidate.palette();
            // 编辑背景与文字的中间步骤可能暂时不可读，保留上一次可读预览，避免设置入口消失。
            if palette.text.relative_contrast(palette.background) >= 4.5
                && palette
                    .text
                    .relative_contrast(candidate.extended_palette().background.weak.color)
                    >= 4.5
            {
                self.cached_theme = candidate;
                self.theme_appearance = colors.clone();
                self.theme_dark = self.system_dark;
            }
        }
        let background = self.cached_theme.palette().background;
        let dark = background.r + background.g + background.b < 1.5;
        let color = self.cached_theme.palette().primary;
        let accent = [color.r, color.g, color.b].map(|n| (n * 255.0).round() as u8);
        if accent != self.logo_color
            && let Ok(logo) = load_logo(self.icon_scale, accent)
        {
            self.logo = logo;
            self.logo_color = accent;
        }
        if let Some(tray) = &mut self.tray
            && tray.set_accent(accent).is_err()
        {
            self.feedback = Some("托盘图标更新失败，其他设置仍然有效".into());
        }
        window_icons::refresh(self.window, dark, accent).map(|()| Message::IconsReady)
    }
    fn view(&self, _: window::Id) -> Element<'_, Message> {
        view::view(self)
    }
    fn theme(&self, _: window::Id) -> Theme {
        self.cached_theme.clone()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
            window::close_requests().map(Message::CloseRequested),
            iced::system::theme_changes().map(Message::SystemTheme),
            window::events().filter_map(|(_, event)| match event {
                window::Event::Rescaled(scale) => Some(Message::Scale(scale)),
                _ => None,
            }),
        ])
    }
    fn send(&mut self, command: Command) -> bool {
        let ok = self
            .worker
            .as_ref()
            .is_some_and(|w| w.commands.send(command).is_ok());
        if !ok {
            self.feedback = Some("后台守护未运行，请重新启动应用".into());
        }
        ok
    }
    fn log(&mut self, text: String) {
        if self.logs.len() == 200 {
            self.logs.pop_front();
        }
        self.logs.push_back(text);
    }
    fn edited(&mut self) {
        self.autosave.edit(Instant::now());
        self.feedback = None;
    }
    fn apply_saved_overrides(&mut self, overrides: GuiOverrides) {
        self.effective = self.base.effective(&overrides);
        self.overrides = overrides;
    }
    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        if self.autostart_intent.is_none() {
            self.form.autostart = snapshot.autostart;
        }
        if let Some(tray) = &self.tray {
            tray.set_paused(snapshot.paused);
        }
        self.snapshot = snapshot;
    }
    fn acknowledge_autostart(&mut self, enabled: bool, generation: u64) {
        self.snapshot.autostart = enabled;
        if self
            .autostart_intent
            .is_some_and(|(_, current)| current == generation)
        {
            self.autostart_intent = None;
            self.form.autostart = enabled;
        }
    }
    fn primary_exit_patch(&self, settings: &PrimarySettings) -> PrimaryPatch {
        let mut patch = settings.patch_against(&self.primary_saved);
        // 退出期间旧写入可能先完成；也覆盖这些字段的最新值，包括用户撤回的修改。
        if let Some(pending) = &self.saving {
            let inflight = settings.patch_against(&pending.primary_settings);
            macro_rules! merge { ($($field:ident),+) => { $(if patch.$field.is_none() { patch.$field = inflight.$field; })+ }; }
            merge!(
                ssid, username, password, service, entry_url, api_base, probe_url
            );
        }
        patch
    }
    fn prepare_exit_plan(&mut self) -> Result<shutdown::Plan> {
        let overrides = self.form.prepared_overrides(
            &self.overrides,
            &self.effective,
            &self.base,
            self.system_dark,
        )?;
        let primary_settings = self.form.primary_settings();
        let patch = self.primary_exit_patch(&primary_settings);
        if !patch.is_empty() {
            self.form.validate_primary(&self.effective)?;
        }
        Ok(shutdown::Plan {
            primary: (!patch.is_empty()).then_some((patch, primary_settings)),
            overrides: (self.autosave.is_dirty() || self.pending || self.saving.is_some())
                .then_some(overrides),
            autostart: self.autostart_intent,
        })
    }
    fn begin_shutdown(&mut self, plan: shutdown::Plan) {
        self.exit_confirm = true;
        self.exit_error = None;
        self.discard_on_exit = false;
        self.pending = true;
        self.feedback = Some("正在保存并结束后台守护…".into());
        self.exit_plan = Some(plan.clone());
        let (receiver, thread) = shutdown::launch(
            self.worker.take(),
            plan,
            self.preview,
            self.config_path.clone(),
        );
        self.shutdown = Some(receiver);
        self.shutdown_thread = Some(thread);
    }
    fn finish_shutdown(&mut self, outcome: shutdown::Outcome) -> Task<Message> {
        self.shutdown = None;
        if let Some(thread) = self.shutdown_thread.take() {
            let _ = thread.join();
        }
        if let Some(plan) = self.exit_plan.take() {
            if outcome.primary_saved
                && let Some((_, settings)) = plan.primary
            {
                self.primary_saved = settings;
            }
            if outcome.settings_saved
                && let Some(overrides) = plan.overrides
            {
                self.apply_saved_overrides(overrides);
            }
        }
        if let Some((enabled, generation)) = outcome.autostart {
            self.acknowledge_autostart(enabled, generation);
        }
        self.pending = false;
        self.saving = None;
        self.autostart_requested = None;
        if let Some(error) = outcome.error {
            self.autosave.finish(false);
            self.snapshot.status = Status::Stopped;
            self.snapshot.busy = false;
            self.snapshot.next_probe = None;
            self.startup_error =
                Some("后台守护已停止，退出尚未完成；可重试保存或放弃修改退出".into());
            self.feedback = Some(error.clone());
            self.exit_error = Some(error);
            self.discard_on_exit = true;
            self.exit_confirm = true;
            return self.show();
        }
        iced::exit()
    }
    fn finish_save(&mut self, success: bool, message: &str) {
        self.autosave.finish(success);
        self.pending = false;
        let warning = self.save_warning.take();
        self.feedback = Some(
            if let Some(warning) = warning {
                warning
            } else if success && self.autosave.is_dirty() {
                "后续修改正在等待自动保存"
            } else {
                message
            }
            .into(),
        );
    }
    fn flush_autosave(&mut self, now: Instant) {
        let Some(revision) = self.autosave.begin(now) else {
            return;
        };
        self.save_warning = None;
        let overrides = match self.form.prepared_overrides(
            &self.overrides,
            &self.effective,
            &self.base,
            self.system_dark,
        ) {
            Ok(overrides) => overrides,
            Err(error) => {
                self.finish_save(false, &format!("尚未保存：{error:#}"));
                return;
            }
        };
        let primary_settings = self.form.primary_settings();
        let primary_patch = primary_settings.patch_against(&self.primary_saved);
        if !primary_patch.is_empty()
            && let Err(error) = self.form.validate_primary(&self.effective)
        {
            self.finish_save(false, &format!("主网络尚未保存：{error:#}"));
            return;
        }
        if self.preview {
            self.primary_saved = primary_settings;
            self.apply_saved_overrides(overrides);
            if let Some((enabled, generation)) = self.autostart_intent {
                self.acknowledge_autostart(enabled, generation);
            }
            self.finish_save(true, "已自动保留预览设置，未写入文件或修改自启");
        } else {
            let command = if primary_patch.is_empty() {
                Command::Settings(Box::new(overrides.clone()))
            } else {
                Command::PrimarySettings(Box::new(primary_patch), self.config_path.clone())
            };
            if self.send(command) {
                self.pending = true;
                self.saving = Some(PendingSave {
                    overrides,
                    primary_settings,
                    autostart: self.autostart_intent,
                    revision,
                });
                self.feedback = None;
            } else {
                self.autosave.finish(false);
            }
        }
    }
    fn show(&mut self) -> Task<Message> {
        self.visible = true;
        window::set_mode(self.window, window::Mode::Windowed).chain(window::gain_focus(self.window))
    }
    fn hide(&mut self) -> Task<Message> {
        if self.tray.is_none() {
            self.exit_confirm = true;
            return self.show();
        }
        self.visible = false;
        window::set_mode(self.window, window::Mode::Hidden)
    }
    fn tick(&mut self) -> Task<Message> {
        if let Some(receiver) = &self.shutdown {
            let outcome = match receiver.try_recv() {
                Ok(outcome) => outcome,
                Err(mpsc::TryRecvError::Empty) => return Task::none(),
                Err(mpsc::TryRecvError::Disconnected) => shutdown::Outcome {
                    primary_saved: false,
                    settings_saved: false,
                    autostart: None,
                    error: Some("退出保存线程意外结束；请重试，或明确放弃修改退出".into()),
                },
            };
            return self.finish_shutdown(outcome);
        }
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(worker) = &self.worker {
            loop {
                match worker.events.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for event in events {
            match event {
                Event::Snapshot(snapshot) => {
                    self.apply_snapshot(snapshot);
                }
                Event::Log(text) => self.log(text),
                Event::Networks(networks) => {
                    if networks != self.networks {
                        self.networks_revision = self.networks_revision.wrapping_add(1);
                    }
                    self.networks = networks;
                    self.feedback = Some(
                        if self.networks.is_empty() {
                            "没有发现可用 WiFi"
                        } else {
                            "已更新可用 WiFi"
                        }
                        .into(),
                    );
                }
                Event::Autostart(enabled) => {
                    self.autostart_saved(enabled);
                }
                Event::SettingsSaved(ok) => self.settings_saved(ok),
                Event::PrimarySettingsSaved(ok, warning) => {
                    self.primary_settings_saved(ok, warning)
                }
            }
        }
        if self
            .preview_login
            .is_some_and(|when| Instant::now() >= when)
        {
            self.preview_login = None;
            self.snapshot.status = if self.snapshot.paused {
                Status::Paused
            } else {
                Status::Online
            };
            self.snapshot.busy = false;
            self.snapshot.online = true;
            self.snapshot.detail = if self.snapshot.paused {
                "自动探测已暂停，手动连接已完成"
            } else {
                "网络连接正常，守护正在后台运行"
            }
            .into();
            self.snapshot.last_login = Some(SystemTime::now());
            self.log("离线预览：连接状态已更新，未执行网络操作".into());
        }
        if self.preview {
            if self.snapshot.paused {
                self.snapshot.next_probe = None;
            } else if !self.snapshot.busy
                && self
                    .snapshot
                    .next_probe
                    .is_none_or(|next| next <= Instant::now())
            {
                self.snapshot.next_probe = Some(
                    Instant::now() + Duration::from_secs(self.effective.daemon.probe_interval),
                );
            }
        }
        let actions = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        let mut tasks = Vec::new();
        for action in actions {
            tasks.push(match action {
                tray::Action::Toggle => {
                    if self.visible {
                        self.hide()
                    } else {
                        self.show()
                    }
                }
                tray::Action::Show => self.show(),
                tray::Action::Login => self.update(Message::Login),
                tray::Action::Pause => self.update(Message::TogglePause),
                tray::Action::Exit => self.update(Message::RequestExit),
            });
        }
        if disconnected {
            self.worker.take();
            self.pending = false;
            self.saving = None;
            self.autostart_requested = None;
            self.autosave.finish(false);
            self.snapshot.status = Status::Stopped;
            self.snapshot.busy = false;
            self.snapshot.next_probe = None;
            self.startup_error = Some("后台守护意外结束，请重新启动应用".into());
            self.feedback = self.startup_error.clone();
        }
        self.flush_autosave(Instant::now());
        Task::batch(tasks)
    }
    fn settings_saved(&mut self, success: bool) {
        let Some(PendingSave {
            overrides,
            autostart,
            revision,
            ..
        }) = self.saving.take()
        else {
            return;
        };
        if !success {
            self.finish_save(
                false,
                if self.autosave.is_current(revision) {
                    "自动保存失败，原有设置仍然有效；修改后会再次尝试"
                } else {
                    "较早修改保存失败，最新草稿仍在等待自动保存"
                },
            );
            return;
        }
        self.apply_saved_overrides(overrides);
        if let Some((enabled, generation)) = autostart {
            if enabled == self.snapshot.autostart {
                self.acknowledge_autostart(enabled, generation);
                self.finish_save(true, "已自动保存");
                return;
            }
            self.autostart_requested = Some((enabled, generation));
            if !self.send(Command::Autostart(enabled, self.config_path.clone())) {
                self.pending = false;
                self.autostart_requested = None;
                self.autosave.finish(false);
            }
        } else {
            self.finish_save(true, "已自动保存");
        }
    }
    fn primary_settings_saved(&mut self, success: bool, warning: Option<&'static str>) {
        let Some(pending) = &self.saving else {
            return;
        };
        if !success {
            self.saving = None;
            self.finish_save(false, "主网络设置尚未保存，请查看日志并修正；草稿已保留");
            return;
        }
        // 只更新磁盘基线，不改变运行中 Engine 的认证和 WiFi 配置。
        self.primary_saved = pending.primary_settings.clone();
        self.save_warning = warning;
        let overrides = pending.overrides.clone();
        if !self.send(Command::Settings(Box::new(overrides))) {
            self.saving = None;
            self.finish_save(false, "主网络已保存，界面设置保存失败；请重试");
        }
    }
    fn autostart_saved(&mut self, enabled: bool) {
        self.snapshot.autostart = enabled;
        if let Some((desired, generation)) = self.autostart_requested.take() {
            if desired == enabled {
                self.acknowledge_autostart(enabled, generation);
            }
            self.finish_save(
                desired == enabled,
                if desired == enabled {
                    "已自动保存；自启状态已更新"
                } else {
                    "设置已保存，但自启修改失败；请查看日志"
                },
            );
        } else if self.autostart_intent.is_none() {
            self.form.autostart = enabled;
        }
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        if self.shutdown.is_some() {
            return match message {
                Message::Tick => self.tick(),
                _ => Task::none(),
            };
        }
        let edited = matches!(
            message,
            Message::Autostart(_)
                | Message::PrimarySsid(_)
                | Message::Username(_)
                | Message::AccountPassword(_)
                | Message::Service(_)
                | Message::EntryUrl(_)
                | Message::ApiBase(_)
                | Message::ProbeUrl(_)
                | Message::Fallback(_)
                | Message::Ssid(_)
                | Message::Password(_)
                | Message::Recovery(_)
                | Message::Interval(_)
                | Message::Threshold(_)
                | Message::Debug(_)
                | Message::Mode(_)
                | Message::Style(_)
                | Message::Accent(_)
                | Message::Background(_)
                | Message::Surface(_)
                | Message::TextColor(_)
                | Message::ResetColors
        );
        match message {
            Message::Tick => return self.tick(),
            Message::Opened(id) => {
                self.window = id;
                match tray::Tray::new(self.preview, self.logo_color) {
                    Ok(tray) => self.tray = Some(tray),
                    Err(_) => {
                        self.feedback = Some("系统托盘不可用，请保持窗口打开".into());
                        self.visible = true;
                    }
                }
                if !self.preview {
                    self.worker = Some(Worker::start(self.base.clone(), self.overrides.clone()));
                }
                let tasks = Task::batch([
                    window::scale_factor(id).map(Message::Scale),
                    self.refresh_native(),
                ]);
                return if self.visible {
                    Task::batch([self.show(), tasks])
                } else {
                    tasks
                };
            }
            Message::CloseRequested(id) if id == self.window => return self.hide(),
            Message::CloseRequested(_) => {}
            Message::SystemTheme(mode) => {
                let dark = mode == iced::theme::Mode::Dark;
                if self.form.mode == ThemeMode::System && dark != self.system_dark {
                    let _ = self.form.capture_colors(self.system_dark);
                    self.system_dark = dark;
                    self.form.load_colors(dark);
                } else {
                    self.system_dark = dark;
                }
                return self.refresh_native();
            }
            Message::Scale(scale) => {
                self.icon_scale = scale;
                if let Ok(logo) = load_logo(scale, self.logo_color) {
                    self.logo = logo;
                }
                return self.refresh_native();
            }
            Message::IconsReady => return Task::none(),
            Message::OpenSettings => {
                self.page = Page::Settings;
            }
            Message::Back => {
                self.page = Page::Home;
                self.form.password_visible = false;
                self.form.account_password_visible = false;
                let now = Instant::now();
                self.autosave.expedite(now);
                self.flush_autosave(now);
            }
            Message::Login => {
                if self.snapshot.busy || self.startup_error.is_some() {
                    return Task::none();
                }
                if self.preview {
                    self.snapshot.status = Status::Authenticating;
                    self.snapshot.busy = true;
                    self.preview_login = Some(Instant::now() + Duration::from_millis(900));
                } else {
                    self.send(Command::Login);
                }
            }
            Message::TogglePause => {
                if self.preview {
                    self.preview_login = None;
                    self.snapshot.busy = false;
                    self.snapshot.paused = !self.snapshot.paused;
                    self.snapshot.status = if self.snapshot.paused {
                        Status::Paused
                    } else {
                        Status::Online
                    };
                    if let Some(tray) = &self.tray {
                        tray.set_paused(self.snapshot.paused);
                    }
                } else {
                    self.send(Command::Pause(!self.snapshot.paused));
                }
            }
            Message::Autostart(v) => {
                self.form.autostart = v;
                self.autostart_generation = self.autostart_generation.wrapping_add(1);
                self.autostart_intent = Some((v, self.autostart_generation));
            }
            Message::Fallback(v) => self.form.fallback_enabled = v,
            Message::PrimarySsid(v) => self.form.primary_ssid = v,
            Message::Username(v) => self.form.username = v,
            Message::AccountPassword(v) => self.form.account_password = v,
            Message::ToggleAccountPassword => {
                self.form.account_password_visible = !self.form.account_password_visible
            }
            Message::Service(v) => self.form.service = v,
            Message::EntryUrl(v) => self.form.entry_url = v,
            Message::ApiBase(v) => self.form.api_base = v,
            Message::ProbeUrl(v) => self.form.probe_url = v,
            Message::TogglePortal => self.advanced_portal = !self.advanced_portal,
            Message::Ssid(v) => self.form.ssid = v,
            Message::Password(v) => self.form.password = v,
            Message::TogglePassword => self.form.password_visible = !self.form.password_visible,
            Message::Recovery(v) => self.form.recovery = v,
            Message::Interval(v) => self.form.interval = v,
            Message::Threshold(v) => self.form.threshold = v,
            Message::Debug(v) => self.form.debug = v,
            Message::Scan => {
                if self.preview {
                    let networks = vec!["Campus_WiFi".into(), "Fallback_WiFi".into()];
                    if networks != self.networks {
                        self.networks_revision = self.networks_revision.wrapping_add(1);
                    }
                    self.networks = networks;
                } else {
                    self.send(Command::ScanNetworks);
                }
            }
            Message::ToggleColors => self.advanced_colors = !self.advanced_colors,
            Message::Mode(mode) => {
                if let Err(error) = self.form.capture_colors(self.system_dark) {
                    self.feedback = Some(error.to_string());
                    return Task::none();
                }
                self.form.mode = mode;
                self.form.appearance.mode = mode;
                self.form.load_colors(self.system_dark);
                self.feedback = None;
            }
            Message::Style(style) => {
                if let Err(error) = self.form.capture_colors(self.system_dark) {
                    self.feedback = Some(error.to_string());
                    return Task::none();
                }
                self.form.style = style;
                self.form.appearance.style = style;
                self.form.load_colors(self.system_dark);
                self.feedback = None;
            }
            Message::Accent(v) => {
                self.form.accent = v;
                let _ = self.form.capture_colors(self.system_dark);
            }
            Message::Background(v) => {
                self.form.background = v;
                let _ = self.form.capture_colors(self.system_dark);
            }
            Message::Surface(v) => {
                self.form.surface = v;
                let _ = self.form.capture_colors(self.system_dark);
            }
            Message::TextColor(v) => {
                self.form.text = v;
                let _ = self.form.capture_colors(self.system_dark);
            }
            Message::ResetColors => {
                let dark = appearance::is_dark(&self.form.appearance, self.system_dark);
                *self.form.appearance.colors_mut(dark) = ColorOverrides::default();
                self.form.load_colors(self.system_dark);
                self.feedback = None;
            }
            Message::OpenConfig | Message::OpenLog => {
                if self.preview {
                    self.feedback = Some("离线预览不打开真实配置或日志".into());
                } else {
                    let path = if matches!(message, Message::OpenConfig) {
                        self.config_path.clone()
                    } else if self.effective.logging.file.is_empty() {
                        config::data_dir().join("guardian.log")
                    } else {
                        PathBuf::from(&self.effective.logging.file)
                    };
                    if platform::open_file(&path).is_err() {
                        self.feedback =
                            Some("无法打开文件，请检查文件是否存在及默认打开程序".into());
                    }
                }
            }
            Message::RequestExit => {
                self.exit_confirm = true;
                self.exit_error = None;
                self.discard_on_exit = false;
                return self.show();
            }
            Message::CancelExit => {
                self.exit_confirm = false;
                self.exit_error = None;
                self.discard_on_exit = false;
            }
            Message::ConfirmExit => match self.prepare_exit_plan() {
                Ok(plan) => self.begin_shutdown(plan),
                Err(error) => {
                    let error =
                        format!("修改尚未保存：{error:#}。请返回设置修正，或明确放弃修改退出");
                    self.feedback = Some(error.clone());
                    self.exit_error = Some(error);
                    self.discard_on_exit = true;
                    self.exit_confirm = true;
                    return self.show();
                }
            },
            Message::DiscardAndExit => {
                if self.discard_on_exit {
                    self.begin_shutdown(shutdown::Plan {
                        primary: None,
                        overrides: None,
                        autostart: None,
                    });
                }
            }
        }
        if edited {
            self.edited();
        }
        self.refresh_native()
    }
}
impl Drop for App {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join();
        }
        if let Some(thread) = self.shutdown_thread.take() {
            let _ = thread.join();
        }
    }
}

// 先按实际 DPI 用面积覆盖的 Lanczos 滤波缩小，避免渲染器直接将大图双线性缩成图标时漏采边缘。
fn load_logo(scale: f32, accent: [u8; 3]) -> Result<iced::widget::image::Handle> {
    let pixels = (34.0 * scale).round().clamp(16.0, 256.0) as u32;
    let mut logo = image::load_from_memory(include_bytes!("../../logo.png"))?
        .resize_exact(pixels, pixels, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    for pixel in logo.pixels_mut() {
        pixel.0[..3].copy_from_slice(&accent);
    }
    Ok(iced::widget::image::Handle::from_rgba(
        pixels,
        pixels,
        logo.into_raw(),
    ))
}

/// 启动 Iced 面板；预览不读取真实配置、不启动后台、不改变网络或自启。
pub fn run(config_path: PathBuf, preview: bool, hidden: bool) -> Result<()> {
    let Some(guard) = tray::InstanceGuard::acquire(preview)? else {
        return Ok(());
    };
    let (base, overrides, startup_error) = if preview {
        let mut config = AppConfig::default();
        config.portal.entry_url = "http://portal.test/".into();
        config.portal.api_base = "http://portal.test/".into();
        config.account.username = "2024000000".into();
        config.account.password = "synthetic-preview-only".into();
        config.wifi.ssid = "Campus_WiFi".into();
        (config, GuiOverrides::default(), None)
    } else {
        if !config_path.exists() {
            if let Some(parent) = config_path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).context("无法创建配置目录")?;
            }
            std::fs::write(&config_path, include_str!("../../config.example.yaml"))
                .context("无法创建配置模板")?;
        }
        match AppConfig::read(&config_path) {
            Ok(config) => {
                let error = config.validate().err().map(|error| error.to_string());
                (config, GuiOverrides::load(), error)
            }
            Err(error) => (
                AppConfig::default(),
                GuiOverrides::load(),
                Some(format!("{error}")),
            ),
        }
    };
    let saved_appearance = overrides
        .appearance
        .clone()
        .filter(|a| appearance::validate(a).is_ok())
        .unwrap_or_default();
    let effective = base.effective(&overrides);
    let cached_theme = appearance::theme(&saved_appearance, false);
    let primary = cached_theme.palette().primary;
    let theme_appearance = saved_appearance.clone();
    let logo_color = [primary.r, primary.g, primary.b].map(|n| (n * 255.0).round() as u8);
    let snapshot = if preview {
        Snapshot {
            status: Status::Online,
            online: true,
            ssid: "Campus_WiFi".into(),
            detail: "网络连接正常，守护正在后台运行".into(),
            last_login: Some(SystemTime::now()),
            next_probe: Some(Instant::now() + Duration::from_secs(30)),
            ..Snapshot::default()
        }
    } else {
        Snapshot {
            status: if startup_error.is_some() {
                Status::Offline
            } else {
                Status::Starting
            },
            ..Snapshot::default()
        }
    };
    let icon_image = image::load_from_memory(include_bytes!("../../logo.png"))?.to_rgba8();
    let icon = window::icon::from_rgba(
        icon_image.clone().into_raw(),
        icon_image.width(),
        icon_image.height(),
    )
    .ok();
    let (id, open) = window::open(window::Settings {
        size: Size::new(432.0, 560.0),
        min_size: Some(Size::new(360.0, 480.0)),
        resizable: true,
        visible: !hidden,
        exit_on_close_request: false,
        icon,
        ..window::Settings::default()
    });
    let app = App {
        page: Page::Home,
        form: Form::new(&effective, saved_appearance.clone(), false, false),
        snapshot,
        startup_error,
        logs: VecDeque::new(),
        effective,
        networks: Vec::new(),
        networks_revision: 0,
        pending: false,
        feedback: None,
        preview,
        exit_confirm: false,
        exit_error: None,
        discard_on_exit: false,
        logo: load_logo(1.0, logo_color)?,
        advanced_colors: false,
        advanced_portal: false,
        primary_saved: PrimarySettings::from_config(&base),
        base,
        overrides,
        system_dark: false,
        worker: None,
        tray: None,
        _guard: guard,
        window: id,
        visible: !hidden,
        config_path,
        saving: None,
        save_warning: None,
        autostart_requested: None,
        autostart_intent: None,
        autostart_generation: 0,
        autosave: Autosave::default(),
        shutdown: None,
        exit_plan: None,
        shutdown_thread: None,
        preview_login: None,
        icon_scale: 1.0,
        logo_color,
        cached_theme,
        theme_appearance,
        theme_dark: false,
    };
    let boot = RefCell::new(Some((
        app,
        Task::batch([
            open.map(Message::Opened),
            iced::system::theme().map(Message::SystemTheme),
        ]),
    )));
    iced::daemon(
        move || boot.borrow_mut().take().expect("Iced 初始化只能执行一次"),
        App::update,
        App::view,
    )
    .title(if preview {
        "CUIT Helper · 离线预览"
    } else {
        "CUIT Helper"
    })
    .theme(App::theme)
    .subscription(App::subscription)
    .default_font(Font::with_name("Microsoft YaHei UI"))
    .antialiasing(true)
    .run()
    .map_err(|_| anyhow::anyhow!("桌面界面初始化失败，请检查显示环境"))
}
/// 在尚未建立 Iced 事件循环时呈现启动错误。
pub fn show_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "CUIT Helper".encode_utf16().chain(Some(0)).collect();
    // UTF-16 缓冲区以 NUL 结尾，并在同步调用期间保持有效。
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    // 不启动 Iced、不创建窗口或托盘、不启动 Worker；所有 Task 都只构造而不运行。
    fn offline_app() -> App {
        let base = AppConfig::default();
        let appearance = Appearance::default();
        let cached_theme = appearance::theme(&appearance, false);
        App {
            page: Page::Settings,
            snapshot: Snapshot::default(),
            startup_error: Some("合成测试配置".into()),
            logs: VecDeque::new(),
            effective: base.clone(),
            form: Form::new(&base, appearance.clone(), false, false),
            networks: Vec::new(),
            networks_revision: 0,
            pending: false,
            feedback: None,
            preview: true,
            exit_confirm: false,
            exit_error: None,
            discard_on_exit: false,
            logo: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
            advanced_colors: false,
            advanced_portal: false,
            primary_saved: PrimarySettings::from_config(&base),
            base,
            overrides: GuiOverrides::default(),
            system_dark: false,
            worker: None,
            tray: None,
            _guard: tray::InstanceGuard::acquire(true).unwrap().unwrap(),
            window: window::Id::unique(),
            visible: true,
            config_path: PathBuf::from("synthetic-unused.yaml"),
            saving: None,
            save_warning: None,
            autostart_requested: None,
            autostart_intent: None,
            autostart_generation: 0,
            autosave: Autosave::default(),
            shutdown: None,
            exit_plan: None,
            shutdown_thread: None,
            preview_login: None,
            icon_scale: 1.0,
            logo_color: [0, 0, 0],
            cached_theme,
            theme_appearance: appearance,
            theme_dark: false,
        }
    }

    fn stage_save(app: &mut App) {
        let revision = app
            .autosave
            .begin(Instant::now() + Duration::from_secs(1))
            .unwrap();
        let overrides = app
            .form
            .prepared_overrides(&app.overrides, &app.effective, &app.base, false)
            .unwrap();
        app.pending = true;
        app.saving = Some(PendingSave {
            overrides,
            primary_settings: app.form.primary_settings(),
            autostart: app.autostart_intent,
            revision,
        });
    }

    fn valid_primary_app() -> App {
        let mut app = offline_app();
        app.base.portal.entry_url = "https://portal.test/entry".into();
        app.base.portal.api_base = "https://portal.test/api".into();
        app.base.portal.probe_url = "https://probe.test/check".into();
        app.base.account.username = "synthetic-student".into();
        app.base.account.password = "synthetic-password".into();
        app.base.wifi.ssid = "Synthetic_Campus".into();
        app.effective = app.base.clone();
        app.primary_saved = PrimarySettings::from_config(&app.base);
        app.form = Form::new(&app.base, Appearance::default(), false, false);
        app
    }

    #[test]
    fn preview_primary_save_only_changes_memory_and_keeps_runtime_network() {
        let mut app = valid_primary_app();
        let _ = app.update(Message::PrimarySsid("New_Synthetic_Campus".into()));
        let _ = app.update(Message::AccountPassword("new-synthetic-password".into()));
        app.flush_autosave(Instant::now() + Duration::from_secs(2));
        assert_eq!(app.primary_saved.ssid, "New_Synthetic_Campus");
        assert_eq!(app.primary_saved.password, "new-synthetic-password");
        assert_eq!(app.effective.wifi.ssid, "Synthetic_Campus");
        assert_eq!(app.base.account.password, "synthetic-password");
        let json = serde_json::to_string(&app.overrides).unwrap();
        assert!(!json.contains("New_Synthetic_Campus"));
        assert!(!json.contains("new-synthetic-password"));
        assert!(!app.autosave.is_dirty());
        assert!(app.worker.is_none());
    }

    #[test]
    fn appearance_edit_preserves_spaced_ssid_and_invalid_unedited_service() {
        let mut base = AppConfig::default();
        base.wifi.ssid = " Synthetic_Campus ".into();
        base.portal.service = "synthetic-unknown".into();
        let previous = PrimarySettings::from_config(&base);
        let mut form = Form::new(&base, Appearance::default(), false, false);
        form.style = ThemeStyle::ChatGpt;
        assert!(form.primary_settings().patch_against(&previous).is_empty());
        form.service = ServiceChoice::Telecom;
        let patch = form.primary_settings().patch_against(&previous);
        assert_eq!(patch.service.as_deref(), Some("ctcc"));
        assert!(patch.ssid.is_none());
    }

    #[test]
    fn exit_reverted_primary_field_overwrites_in_flight_older_value() {
        let mut app = valid_primary_app();
        let _ = app.update(Message::PrimarySsid("Older_Synthetic_Campus".into()));
        stage_save(&mut app);
        let _ = app.update(Message::PrimarySsid("Synthetic_Campus".into()));
        let plan = app.prepare_exit_plan().unwrap();
        let (patch, final_settings) = plan.primary.unwrap();
        assert_eq!(patch.ssid.as_deref(), Some("Synthetic_Campus"));
        assert_eq!(final_settings.ssid, "Synthetic_Campus");
        assert!(patch.password.is_none());
    }

    #[test]
    fn primary_ack_preserves_newer_input_and_does_not_apply_network() {
        let mut app = valid_primary_app();
        let _ = app.update(Message::Username("first-synthetic-student".into()));
        stage_save(&mut app);
        let _ = app.update(Message::Username("latest-synthetic-student".into()));
        app.primary_settings_saved(true, None);
        assert_eq!(app.primary_saved.username, "first-synthetic-student");
        assert_eq!(app.form.username, "latest-synthetic-student");
        assert_eq!(app.effective.account.username, "synthetic-student");
        assert!(app.autosave.is_dirty());
    }

    #[test]
    fn committed_primary_cleanup_warning_advances_baseline_and_keeps_revert() {
        let mut app = valid_primary_app();
        let _ = app.update(Message::PrimarySsid("New_Synthetic_Campus".into()));
        stage_save(&mut app);
        app.primary_settings_saved(true, Some("主网络已保存，私有旧配置清理失败"));
        assert_eq!(app.primary_saved.ssid, "New_Synthetic_Campus");
        let _ = app.update(Message::PrimarySsid("Synthetic_Campus".into()));
        let plan = app.prepare_exit_plan().unwrap();
        assert_eq!(
            plan.primary.unwrap().0.ssid.as_deref(),
            Some("Synthetic_Campus")
        );
    }

    #[test]
    fn invalid_primary_draft_is_kept_and_blocks_exit_persistence() {
        let mut app = valid_primary_app();
        let _ = app.update(Message::EntryUrl("invalid-synthetic-url".into()));
        app.flush_autosave(Instant::now() + Duration::from_secs(2));
        assert_eq!(app.form.entry_url, "invalid-synthetic-url");
        assert_eq!(app.primary_saved.entry_url, "https://portal.test/entry");
        assert!(app.feedback.as_deref().unwrap().contains("尚未保存"));
        assert!(app.prepare_exit_plan().is_err());
    }

    #[test]
    fn theme_edit_before_first_snapshot_never_changes_existing_autostart() {
        let mut app = offline_app();
        let _ = app.update(Message::Style(ThemeStyle::ChatGpt));
        assert!(app.autosave.is_dirty());
        app.apply_snapshot(Snapshot {
            autostart: true,
            ..Snapshot::default()
        });
        assert!(app.form.autostart);
        assert!(app.autostart_intent.is_none());
        stage_save(&mut app);
        assert!(app.saving.as_ref().unwrap().autostart.is_none());
        app.settings_saved(true);
        assert!(app.autostart_requested.is_none());
        assert!(app.form.autostart);
    }

    #[test]
    fn autostart_ack_uses_its_own_generation_after_unrelated_edits() {
        let mut app = offline_app();
        let _ = app.update(Message::Autostart(true));
        stage_save(&mut app);
        app.autostart_requested = app.autostart_intent;
        let _ = app.update(Message::Ssid("Later_Synthetic_WiFi".into()));
        app.autostart_saved(true);
        assert!(app.form.autostart);
        assert!(app.autostart_intent.is_none());
        assert!(app.autosave.is_dirty());
    }

    #[test]
    fn exit_captures_latest_valid_draft_before_debounce_or_after_failure() {
        let mut app = offline_app();
        let before_edit = Instant::now();
        let _ = app.update(Message::Ssid("Latest_Synthetic_WiFi".into()));
        assert!(app.autosave.begin(before_edit).is_none());
        let plan = app.prepare_exit_plan().unwrap();
        assert_eq!(
            plan.overrides.unwrap().fallback_ssid.as_deref(),
            Some("Latest_Synthetic_WiFi")
        );
        stage_save(&mut app);
        app.settings_saved(false);
        assert!(
            app.autosave
                .begin(Instant::now() + Duration::from_secs(10))
                .is_none()
        );
        let plan = app.prepare_exit_plan().unwrap();
        assert_eq!(
            plan.overrides.unwrap().fallback_ssid.as_deref(),
            Some("Latest_Synthetic_WiFi")
        );
        assert!(plan.autostart.is_none());
    }

    #[test]
    fn exit_snapshot_supersedes_an_older_in_flight_save() {
        let mut app = offline_app();
        let _ = app.update(Message::Ssid("Older_Synthetic_WiFi".into()));
        stage_save(&mut app);
        let _ = app.update(Message::Ssid("Latest_Synthetic_WiFi".into()));
        let plan = app.prepare_exit_plan().unwrap();
        let stored = RefCell::new(String::new());
        let result = shutdown::run_with(
            &plan,
            false,
            || {
                *stored.borrow_mut() = "Older_Synthetic_WiFi".into();
            },
            |_| panic!("仅修改备用网络不得写主网络"),
            |overrides| {
                *stored.borrow_mut() = overrides.fallback_ssid.clone().unwrap();
                Ok(())
            },
            |_| panic!("仅修改网络草稿不得操作自启"),
        );
        assert_eq!(*stored.borrow(), "Latest_Synthetic_WiFi");
        app.exit_plan = Some(plan);
        let _ = app.finish_shutdown(result);
        assert_eq!(app.effective.wifi.fallback.ssid, "Latest_Synthetic_WiFi");
        assert!(!app.pending);
        assert!(app.saving.is_none());
    }

    #[test]
    fn shutdown_save_failure_keeps_a_visible_retry_or_discard_path() {
        let mut app = offline_app();
        let _ = app.update(Message::Ssid("Unsaved_Synthetic_WiFi".into()));
        app.exit_plan = Some(app.prepare_exit_plan().unwrap());
        let _ = app.finish_shutdown(shutdown::Outcome {
            primary_saved: false,
            settings_saved: false,
            autostart: None,
            error: Some("合成保存失败".into()),
        });
        assert!(app.shutdown.is_none());
        assert!(app.exit_confirm && app.discard_on_exit);
        assert_eq!(app.exit_error.as_deref(), Some("合成保存失败"));
        assert_eq!(app.form.ssid, "Unsaved_Synthetic_WiFi");
        assert_eq!(app.overrides.fallback_ssid, None);
        assert!(app.prepare_exit_plan().unwrap().overrides.is_some());
    }

    #[test]
    fn invalid_exit_draft_requires_explicit_discard_without_stopping_worker() {
        let mut app = offline_app();
        let _ = app.update(Message::Accent("#03".into()));
        let _ = app.update(Message::ConfirmExit);
        assert!(app.shutdown.is_none());
        assert!(app.exit_confirm && app.discard_on_exit);
        assert!(app.exit_error.as_deref().unwrap().contains("尚未保存"));
        assert_eq!(app.form.accent, "#03");
    }

    #[test]
    fn older_save_ack_updates_baseline_without_replacing_newer_draft() {
        let mut app = offline_app();
        let _ = app.update(Message::Ssid("First_Synthetic_WiFi".into()));
        stage_save(&mut app);
        let _ = app.update(Message::Ssid("Latest_Synthetic_WiFi".into()));
        app.settings_saved(true);
        assert_eq!(app.effective.wifi.fallback.ssid, "First_Synthetic_WiFi");
        assert_eq!(app.form.ssid, "Latest_Synthetic_WiFi");
        assert!(app.autosave.is_dirty());
        app.flush_autosave(Instant::now() + Duration::from_secs(2));
        assert_eq!(app.effective.wifi.fallback.ssid, "Latest_Synthetic_WiFi");
        assert!(!app.autosave.is_dirty());
        assert!(app.worker.is_none());
    }

    #[test]
    fn autostart_ack_does_not_replace_newer_user_selection() {
        let mut app = offline_app();
        let _ = app.update(Message::Autostart(true));
        let revision = app
            .autosave
            .begin(Instant::now() + Duration::from_secs(1))
            .unwrap();
        app.autostart_requested = Some((true, revision));
        app.pending = true;
        let _ = app.update(Message::Autostart(false));
        app.autostart_saved(true);
        assert!(app.snapshot.autostart);
        assert!(!app.form.autostart);
        assert!(app.autosave.is_dirty());
        app.flush_autosave(Instant::now() + Duration::from_secs(2));
        assert!(!app.snapshot.autostart);
        assert!(!app.autosave.is_dirty());
    }

    #[test]
    fn failed_save_preserves_draft_and_navigation_does_not_retry_it() {
        let mut app = offline_app();
        let _ = app.update(Message::Ssid("Unsaved_Synthetic_WiFi".into()));
        stage_save(&mut app);
        app.settings_saved(false);
        let _ = app.update(Message::Back);
        let _ = app.update(Message::OpenSettings);
        assert_eq!(app.form.ssid, "Unsaved_Synthetic_WiFi");
        assert_eq!(app.effective.wifi.fallback.ssid, "");
        assert!(app.autosave.is_dirty());
        assert!(app.feedback.as_deref().unwrap().contains("失败"));
        assert_eq!(
            app.autosave.begin(Instant::now() + Duration::from_secs(10)),
            None
        );
    }

    #[test]
    fn invalid_drafts_survive_navigation_and_never_replace_saved_overrides() {
        let mut app = offline_app();
        let previous = serde_json::to_value(&app.overrides).unwrap();
        let _ = app.update(Message::Accent("#03".into()));
        let _ = app.update(Message::Back);
        let _ = app.update(Message::OpenSettings);
        assert_eq!(app.form.accent, "#03");
        assert_eq!(serde_json::to_value(&app.overrides).unwrap(), previous);
        assert!(app.feedback.as_deref().unwrap().contains("尚未保存"));
        for ssid in ["", "contains\0nul", "synthetic-network-name-over-32-bytes"] {
            let mut form = Form::new(&app.base, Appearance::default(), false, false);
            form.fallback_enabled = true;
            form.ssid = ssid.into();
            assert!(
                form.prepared_overrides(&app.overrides, &app.effective, &app.base, false)
                    .is_err()
            );
        }
    }

    #[test]
    fn saved_settings_restore_form_without_creating_a_dirty_draft() {
        let base = AppConfig::default();
        let mut appearance = Appearance {
            style: ThemeStyle::ChatGpt,
            mode: ThemeMode::Dark,
            ..Default::default()
        };
        appearance.colors_mut(true).accent_color = Some("#E0AA88".into());
        let overrides = GuiOverrides {
            probe_interval: Some(60),
            failure_threshold: Some(5),
            fallback_ssid: Some("Saved_Synthetic_WiFi".into()),
            appearance: Some(appearance.clone()),
            ..Default::default()
        };
        let restored: GuiOverrides =
            serde_json::from_value(serde_json::to_value(overrides).unwrap()).unwrap();
        let form = Form::new(
            &base.effective(&restored),
            restored.appearance.unwrap(),
            true,
            false,
        );
        assert_eq!(form.ssid, "Saved_Synthetic_WiFi");
        assert_eq!(form.interval.0, 60);
        assert_eq!(form.threshold.0, 5);
        assert!(form.autostart);
        assert_eq!(form.appearance, appearance);
        assert_eq!(form.accent, "#E0AA88");
        assert!(!Autosave::default().is_dirty());
    }

    #[test]
    fn preserves_yaml_only_values_when_saving_appearance() {
        let mut config = AppConfig::default();
        config.daemon.probe_interval = 47;
        config.daemon.failure_threshold = 7;
        config.wifi.fallback.switch_back_interval = 731;
        config.logging.level = "WARNING".into();
        let form = Form::new(&config, Appearance::default(), false, false);
        let overrides = form.overrides(&GuiOverrides::default(), &config);
        let actual = config.effective(&overrides);
        assert_eq!(actual.daemon.probe_interval, 47);
        assert_eq!(actual.daemon.failure_threshold, 7);
        assert_eq!(actual.wifi.fallback.switch_back_interval, 731);
        assert_eq!(actual.logging.level, "WARNING");
    }
    #[test]
    fn incomplete_hex_does_not_replace_valid_preview() {
        let mut form = Form::new(&AppConfig::default(), Appearance::default(), false, false);
        let old = form.appearance.clone();
        form.accent = "#03".into();
        assert!(form.capture_colors(false).is_err());
        assert_eq!(form.appearance, old);
    }
    #[test]
    fn unreadable_intermediate_colors_keep_the_last_readable_preview() {
        let mut app = offline_app();
        let previous = app.cached_theme.palette();
        let _ = app.update(Message::Background("#141413".into()));
        assert_eq!(app.cached_theme.palette().background, previous.background);
        app.flush_autosave(Instant::now() + Duration::from_secs(2));
        assert!(app.feedback.as_deref().unwrap().contains("尚未保存"));
        let _ = app.update(Message::TextColor("#FAF9F5".into()));
        let _ = app.update(Message::Surface("#212121".into()));
        let palette = app.cached_theme.palette();
        assert!(palette.text.relative_contrast(palette.background) >= 4.5);
        assert_eq!(palette.background, iced::Color::from_rgb8(20, 20, 19));
    }
    #[test]
    fn debug_messages_redact_sensitive_inputs() {
        assert!(!format!("{:?}", Message::Password("test-secret".into())).contains("test-secret"));
        assert!(!format!("{:?}", Message::Ssid("test-network".into())).contains("test-network"));
        for message in [
            Message::PrimarySsid("private-synthetic".into()),
            Message::Username("private-synthetic".into()),
            Message::AccountPassword("private-synthetic".into()),
            Message::EntryUrl("private-synthetic".into()),
            Message::ApiBase("private-synthetic".into()),
            Message::ProbeUrl("private-synthetic".into()),
        ] {
            assert!(!format!("{message:?}").contains("private-synthetic"));
        }
    }
}
