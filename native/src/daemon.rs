//! 串行后台守护：UI 通过消息通信，网络请求和 WiFi 切换始终不在 UI 线程执行。

use crate::{
    auth::{AuthClient, AuthResult},
    config::{AppConfig, GuiOverrides, PrimaryPatch},
    platform,
};
use anyhow::Result;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetworkMode {
    #[default]
    Primary,
    Fallback,
    Offline,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Starting,
    Online,
    Authenticating,
    Offline,
    Fallback,
    Paused,
    Stopped,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub status: Status,
    pub mode: NetworkMode,
    pub online: bool,
    pub paused: bool,
    pub busy: bool,
    pub ssid: String,
    pub detail: String,
    pub last_login: Option<SystemTime>,
    pub next_probe: Option<Instant>,
    pub autostart: bool,
}

pub enum Command {
    Login,
    Pause(bool),
    Settings(Box<GuiOverrides>),
    PrimarySettings(Box<PrimaryPatch>, std::path::PathBuf),
    Autostart(bool, std::path::PathBuf),
    ScanNetworks,
    Stop,
}
pub enum Event {
    Snapshot(Snapshot),
    Log(String),
    Autostart(bool),
    Networks(Vec<String>),
    SettingsSaved(bool),
    PrimarySettingsSaved(bool, Option<&'static str>),
}

pub struct Worker {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
    handle: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}
impl Worker {
    /// 启动唯一守护线程；所有认证、设置持久化和平台调用按消息顺序执行。
    pub fn start(config: AppConfig, overrides: GuiOverrides) -> Self {
        let (commands, input) = mpsc::channel();
        let (output, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = thread::Builder::new()
            .name("guardian-network".into())
            .spawn(move || run(config, overrides, input, output, flag))
            .expect("无法启动守护线程");
        Self {
            commands,
            events,
            handle: Some(handle),
            stop,
        }
    }
    /// 优先取消排队任务和后续网络步骤，不等待 UI 线程中的网络请求。
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Stop);
    }
    /// 等待后台结束；调用前发送 Stop，勿在有网络请求时阻塞 UI 线程。
    pub fn join(mut self) {
        self.request_stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.request_stop();
    }
}

fn run(
    config: AppConfig,
    overrides: GuiOverrides,
    input: Receiver<Command>,
    output: Sender<Event>,
    stop: Arc<AtomicBool>,
) {
    let mut engine = Engine::new(config, overrides, Instant::now());
    let mut backend = RealBackend(stop.clone());
    engine.snapshot.autostart = platform::autostart_enabled();
    let log_config = std::cell::RefCell::new(engine.config.logging.clone());
    let mut emit = |event| {
        if let Event::Log(message) = &event
            && write_log(&log_config.borrow(), message).is_err()
        {
            let _ = output.send(Event::Log("日志文件写入失败，请检查目录权限".into()));
        }
        let _ = output.send(event);
    };
    let mut valid = engine.config.validate().is_ok();
    if !valid {
        engine.snapshot.status = Status::Offline;
        engine.snapshot.detail = "请打开配置文件，填写门户地址与账号密码后重新启动".into();
        engine.snapshot.next_probe = None;
    }
    emit(Event::Snapshot(engine.snapshot.clone()));
    emit(Event::Log(
        if valid {
            "守护已启动"
        } else {
            "配置未完成，守护尚未启动"
        }
        .into(),
    ));
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let delay = if !valid || engine.snapshot.paused {
            Duration::from_secs(3600)
        } else {
            engine.next.saturating_duration_since(Instant::now())
        };
        let command = input.recv_timeout(delay);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match command {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Command::Pause(paused)) => {
                engine.snapshot.paused = paused;
                engine.snapshot.busy = false;
                if paused {
                    engine.snapshot.status = Status::Paused;
                    engine.snapshot.next_probe = None;
                    engine.snapshot.detail = "守护已暂停，可手动登录或恢复守护".into();
                } else {
                    engine.next = Instant::now();
                    engine.snapshot.status = Status::Starting;
                    engine.snapshot.next_probe = Some(engine.next);
                }
                emit(Event::Snapshot(engine.snapshot.clone()));
            }
            Ok(Command::Login) => {
                if !valid {
                    emit(Event::Log("配置未完成，无法开始认证".into()));
                    continue;
                }
                // 手动认证不会偷偷恢复已暂停的守护。
                let paused = engine.snapshot.paused;
                engine.mode = NetworkMode::Primary;
                engine.auth_required = true;
                engine.snapshot.online = false;
                engine.cycle(&mut backend, &mut emit, Instant::now());
                engine.snapshot.paused = paused;
                if paused {
                    engine.snapshot.status = Status::Paused;
                    engine.snapshot.next_probe = None;
                    emit(Event::Snapshot(engine.snapshot.clone()));
                }
            }
            Ok(Command::Settings(settings)) => {
                if !settings_valid(&settings) {
                    emit(Event::Log("设置值无效，未保存".into()));
                    emit(Event::SettingsSaved(false));
                    continue;
                }
                let effective = engine.base.effective(&settings);
                if engine.base.validate().is_ok() && effective.validate().is_err() {
                    emit(Event::Log("设置组合无效，未保存".into()));
                    emit(Event::SettingsSaved(false));
                    continue;
                }
                // 未完成账号配置时仍允许保存日常选项，不开始认证。
                if settings.save().is_err() {
                    emit(Event::Log("设置保存失败，请检查用户数据目录权限".into()));
                    emit(Event::SettingsSaved(false));
                    continue;
                }
                valid = engine.apply_saved_config(effective, Instant::now());
                *log_config.borrow_mut() = engine.config.logging.clone();
                emit(Event::Log("设置已保存并生效".into()));
                emit(Event::SettingsSaved(true));
                emit(Event::Snapshot(engine.snapshot.clone()));
            }
            Ok(Command::PrimarySettings(patch, path)) => {
                // 只持久化 YAML；当前 Engine 的配置、会话和探测时间均保持原值。
                let result = patch.save_to(&path);
                match result {
                    Ok(outcome) => {
                        emit(Event::Log("主网络设置已保存，重启后生效".into()));
                        if let Some(warning) = outcome.warning {
                            emit(Event::Log(format!("主网络设置清理失败：{warning}")));
                        }
                        emit(Event::PrimarySettingsSaved(true, outcome.warning));
                    }
                    Err(error) => {
                        emit(Event::Log(format!("主网络设置保存失败：{error:#}")));
                        emit(Event::PrimarySettingsSaved(false, None));
                    }
                }
            }
            Ok(Command::Autostart(enabled, config_path)) => {
                match platform::set_autostart_with_config(enabled, &config_path) {
                    Ok(()) => emit(Event::Log(
                        if enabled {
                            "已开启登录时自启"
                        } else {
                            "已关闭登录时自启"
                        }
                        .into(),
                    )),
                    Err(_) => emit(Event::Log("自启设置失败，请检查启动目录权限".into())),
                };
                engine.snapshot.autostart = platform::autostart_enabled();
                emit(Event::Autostart(engine.snapshot.autostart));
                emit(Event::Snapshot(engine.snapshot.clone()));
            }
            Ok(Command::ScanNetworks) => match platform::visible_networks() {
                Ok(networks) => emit(Event::Networks(networks)),
                Err(_) => {
                    emit(Event::Log(
                        "无法扫描无线网络，请检查网卡与 Windows 位置权限".into(),
                    ));
                    emit(Event::Networks(Vec::new()));
                }
            },
            Err(RecvTimeoutError::Timeout) => {
                if valid && !engine.snapshot.paused {
                    engine.cycle(&mut backend, &mut emit, Instant::now());
                }
            }
        }
    }
    if engine.config.wifi.fallback.revert_on_stop {
        let _ = platform::ensure_fallback(&engine.config.wifi.fallback);
    }
    engine.snapshot.status = Status::Stopped;
    engine.snapshot.next_probe = None;
    engine.snapshot.busy = false;
    emit(Event::Snapshot(engine.snapshot));
}

fn settings_valid(s: &GuiOverrides) -> bool {
    s.probe_interval
        .is_none_or(|v| [15, 30, 60, 120].contains(&v))
        && s.failure_threshold.is_none_or(|v| [2, 3, 5].contains(&v))
        && s.switch_back_interval
            .is_none_or(|v| [0, 300, 900].contains(&v))
        && s.log_level
            .as_ref()
            .is_none_or(|v| matches!(v.as_str(), "INFO" | "DEBUG"))
        && s.fallback_ssid.as_ref().is_none_or(|v| v.len() <= 32)
        && s.appearance.as_ref().is_none_or(|v| v.validate().is_ok())
}

fn network_settings_changed(before: &AppConfig, after: &AppConfig) -> bool {
    // 只比较 GUI 可覆盖的实际网络值，覆盖项是否显式填写不影响判断。
    before.daemon.probe_interval != after.daemon.probe_interval
        || before.daemon.failure_threshold != after.daemon.failure_threshold
        || before.wifi.fallback.enabled != after.wifi.fallback.enabled
        || before.wifi.fallback.ssid != after.wifi.fallback.ssid
        || before.wifi.fallback.password != after.wifi.fallback.password
        || before.wifi.fallback.switch_back_interval != after.wifi.fallback.switch_back_interval
}

fn write_log(cfg: &crate::config::LoggingConfig, message: &str) -> Result<()> {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        path::{Path, PathBuf},
    };
    let error = message.contains("失败") || message.contains("无效");
    let debug = message.starts_with("DEBUG ");
    if debug && cfg.level != "DEBUG" {
        return Ok(());
    }
    if !error && matches!(cfg.level.as_str(), "WARNING" | "ERROR") {
        return Ok(());
    }
    let path = if cfg.file.is_empty() {
        crate::config::data_dir().join("guardian.log")
    } else {
        PathBuf::from(&cfg.file)
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let line = format!(
        "[{timestamp}] [{}] {message}\n",
        if error {
            "ERROR"
        } else if debug {
            "DEBUG"
        } else {
            "INFO"
        }
    );
    if cfg.max_bytes > 0
        && fs::metadata(&path)
            .map(|m| m.len().saturating_add(line.len() as u64) > cfg.max_bytes)
            .unwrap_or(false)
    {
        fn backup(path: &Path, n: u32) -> PathBuf {
            let mut name = path.as_os_str().to_os_string();
            name.push(format!(".{n}"));
            PathBuf::from(name)
        }
        let count = cfg.backup_count.min(100);
        if count > 0 {
            let oldest = backup(&path, count);
            if oldest.exists() {
                fs::remove_file(oldest)?;
            }
            for i in (1..count).rev() {
                let source = backup(&path, i);
                if source.exists() {
                    fs::rename(source, backup(&path, i + 1))?;
                }
            }
            if path.exists() {
                fs::rename(&path, backup(&path, 1))?;
            }
        } else if path.exists() {
            fs::remove_file(&path)?;
        }
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(line.as_bytes())?;
    Ok(())
}

trait Backend {
    fn ssid(&mut self) -> Result<String>;
    fn connect_primary(&mut self, cfg: &AppConfig) -> Result<bool>;
    fn connect_fallback(&mut self, cfg: &AppConfig) -> Result<bool>;
    fn login(&mut self, cfg: &AppConfig) -> Result<AuthResult>;
    fn probe(&mut self, cfg: &AppConfig, session: Option<&str>) -> Result<bool>;
}
struct RealBackend(Arc<AtomicBool>);
impl Backend for RealBackend {
    fn ssid(&mut self) -> Result<String> {
        platform::current_ssid()
    }
    fn connect_primary(&mut self, cfg: &AppConfig) -> Result<bool> {
        if self.0.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if !cfg.wifi.profile_xml.is_empty() {
            platform::import_profile(std::path::Path::new(&cfg.wifi.profile_xml))?;
        }
        platform::connect_wifi_cancel(
            &cfg.wifi.ssid,
            Duration::from_secs(cfg.wifi.connect_timeout),
            &self.0,
        )
    }
    fn connect_fallback(&mut self, cfg: &AppConfig) -> Result<bool> {
        platform::ensure_fallback_cancel(&cfg.wifi.fallback, &self.0)
    }
    fn login(&mut self, cfg: &AppConfig) -> Result<AuthResult> {
        let mut auth = AuthClient::new(cfg)?;
        auth.set_cancellation(self.0.clone());
        auth.login()
    }
    fn probe(&mut self, cfg: &AppConfig, session: Option<&str>) -> Result<bool> {
        let mut auth = AuthClient::new(cfg)?;
        auth.set_cancellation(self.0.clone());
        auth.probe(session)
    }
}

struct Engine {
    base: AppConfig,
    config: AppConfig,
    snapshot: Snapshot,
    mode: NetworkMode,
    next: Instant,
    session: Option<String>,
    auth_required: bool,
    primary_failures: u32,
    probe_failures: u32,
    fallback_failures: u32,
    last_recovery: Instant,
}
impl Engine {
    fn new(base: AppConfig, overrides: GuiOverrides, now: Instant) -> Self {
        let config = base.effective(&overrides);
        Self {
            base,
            config,
            snapshot: Snapshot::default(),
            mode: NetworkMode::Primary,
            next: now,
            session: None,
            auth_required: true,
            primary_failures: 0,
            probe_failures: 0,
            fallback_failures: 0,
            last_recovery: now,
        }
    }

    fn apply_saved_config(&mut self, config: AppConfig, now: Instant) -> bool {
        let network_changed = network_settings_changed(&self.config, &config);
        let fallback_changed = self.config.wifi.fallback.ssid != config.wifi.fallback.ssid;
        self.config = config;
        let valid = self.config.validate().is_ok();
        // 外观和日志修改不提前探测，不重置仍在退避中的网络状态。
        if network_changed {
            if self.mode == NetworkMode::Fallback
                && (!self.config.wifi.fallback.enabled || fallback_changed)
            {
                self.mode = if self.config.wifi.fallback.enabled {
                    NetworkMode::Fallback
                } else {
                    NetworkMode::Primary
                };
                self.snapshot.online = false;
                self.auth_required = true;
                self.fallback_failures = 0;
            }
            self.next = now;
            self.snapshot.next_probe = if valid && !self.snapshot.paused {
                Some(self.next)
            } else {
                None
            };
        }
        valid
    }

    fn cycle(&mut self, backend: &mut impl Backend, emit: &mut impl FnMut(Event), now: Instant) {
        let started = Instant::now();
        self.snapshot.busy = true;
        self.snapshot.status = if self.mode == NetworkMode::Primary {
            Status::Authenticating
        } else {
            Status::Fallback
        };
        self.snapshot.detail = match self.mode {
            NetworkMode::Primary => "正在检查校园网连接",
            NetworkMode::Fallback => "正在检查备用网络",
            NetworkMode::Offline => "退避结束，重新检查校园网",
        }
        .into();
        emit(Event::Snapshot(self.snapshot.clone()));
        let ssid = backend.ssid();
        let ssid_unavailable = ssid.is_err();
        self.snapshot.ssid = ssid.unwrap_or_default();
        if self.mode == NetworkMode::Offline {
            self.mode = NetworkMode::Primary;
            self.auth_required = true;
        }
        match self.mode {
            NetworkMode::Primary => self.primary(backend, emit, now),
            NetworkMode::Fallback => self.fallback(backend, emit, now),
            NetworkMode::Offline => {}
        }
        let elapsed = started.elapsed();
        self.next += elapsed;
        if self.last_recovery == now {
            self.last_recovery += elapsed;
        }
        self.snapshot.ssid = backend.ssid().unwrap_or_default();
        self.snapshot.mode = self.mode;
        self.snapshot.busy = false;
        if ssid_unavailable && !self.config.wifi.ssid.is_empty() && !self.snapshot.online {
            self.snapshot.detail =
                "无法读取无线网络，请检查网卡及 Windows 的桌面应用位置权限".into();
        }
        self.snapshot.status = if self.snapshot.paused {
            Status::Paused
        } else if self.snapshot.online {
            if self.mode == NetworkMode::Fallback {
                Status::Fallback
            } else {
                Status::Online
            }
        } else {
            Status::Offline
        };
        self.snapshot.next_probe = Some(self.next);
        if self.config.logging.level == "DEBUG" {
            emit(Event::Log(format!(
                "DEBUG 守护探测完成：模式={:?} 在线={} 主网失败={} 备用失败={}",
                self.mode, self.snapshot.online, self.primary_failures, self.fallback_failures
            )));
        }
        emit(Event::Snapshot(self.snapshot.clone()));
    }
    fn primary(&mut self, backend: &mut impl Backend, emit: &mut impl FnMut(Event), now: Instant) {
        if !backend.connect_primary(&self.config).unwrap_or(false) {
            self.snapshot.online = false;
            self.primary_failures = self.primary_failures.saturating_add(1);
            self.snapshot.detail = "校园网连接失败，请检查 WiFi 是否可用".into();
            self.after_primary_failure(backend, emit, now);
            return;
        }
        if !self.auth_required
            && backend
                .probe(&self.config, self.session.as_deref())
                .unwrap_or(false)
        {
            self.primary_success(now);
            self.snapshot.detail = "校园网连接正常，守护已启用".into();
            return;
        }
        self.snapshot.online = false;
        self.probe_failures = self.probe_failures.saturating_add(1);
        if !self.auth_required && self.probe_failures < self.config.daemon.failure_threshold {
            self.snapshot.detail = "网络探测失败，正在等待下一轮确认".into();
            self.schedule_probe(now);
            return;
        }
        self.auth_required = false;
        match backend.login(&self.config) {
            Ok(result) if result.success && result.online => {
                if result.session_id.is_some() {
                    self.session = result.session_id;
                }
                self.snapshot.last_login = Some(SystemTime::now());
                self.primary_success(now);
                self.snapshot.detail = "认证完成，校园网连接正常".into();
                emit(Event::Log("校园网认证成功".into()));
            }
            _ => {
                self.primary_failures = self.primary_failures.saturating_add(1);
                self.snapshot.detail = "认证未成功，请检查账号、门户配置或网络连接".into();
                emit(Event::Log("校园网认证失败，将按退避策略重试".into()));
                self.after_primary_failure(backend, emit, now);
            }
        }
    }
    fn primary_success(&mut self, now: Instant) {
        self.snapshot.online = true;
        self.mode = NetworkMode::Primary;
        self.primary_failures = 0;
        self.probe_failures = 0;
        self.fallback_failures = 0;
        self.schedule_probe(now);
    }
    fn after_primary_failure(
        &mut self,
        backend: &mut impl Backend,
        emit: &mut impl FnMut(Event),
        now: Instant,
    ) {
        let fb = &self.config.wifi.fallback;
        if fb.enabled
            && !fb.ssid.is_empty()
            && self.primary_failures >= self.config.daemon.failure_threshold
        {
            self.mode = NetworkMode::Fallback;
            self.session = None;
            self.last_recovery = now;
            self.fallback_failures = 0;
            emit(Event::Log("校园网连续失败，正在切换备用网络".into()));
            self.fallback(backend, emit, now);
        } else {
            let delay = backoff(self.primary_failures, &self.config);
            self.next = now + Duration::from_secs(delay);
        }
    }
    fn fallback(&mut self, backend: &mut impl Backend, emit: &mut impl FnMut(Event), now: Instant) {
        if !self.config.wifi.fallback.enabled {
            self.mode = NetworkMode::Primary;
            self.auth_required = true;
            self.snapshot.online = false;
            self.next = now;
            return;
        }
        if recovery_due(
            now,
            self.last_recovery,
            self.config.wifi.fallback.switch_back_interval,
        ) {
            self.last_recovery = now;
            emit(Event::Log("正在尝试恢复校园网".into()));
            if backend.connect_primary(&self.config).unwrap_or(false)
                && let Ok(result) = backend.login(&self.config)
                && result.success
                && result.online
            {
                self.session = result.session_id;
                self.snapshot.last_login = Some(SystemTime::now());
                self.primary_success(now);
                self.snapshot.detail = "已恢复校园网连接".into();
                emit(Event::Log("已从备用网络切回校园网".into()));
                return;
            }
            // 实际尝试主网后，失败必须恢复备用，不能仅查看“当前 SSID”假装完成回切。
            emit(Event::Log("校园网尚未恢复，继续使用备用网络".into()));
        }
        let connected = backend.connect_fallback(&self.config).unwrap_or(false);
        let online = connected && backend.probe(&self.config, None).unwrap_or(false);
        self.snapshot.online = online;
        if online {
            self.fallback_failures = 0;
            self.snapshot.detail = "正在使用备用网络，校园网守护仍在运行".into();
            self.schedule_probe(now);
        } else {
            // 只有真正恢复外网后才能清零，WiFi 已关联不代表互联网可用。
            self.fallback_failures = self.fallback_failures.saturating_add(1);
            self.snapshot.detail = "备用网络也无法上网，请检查热点或网络信号".into();
            if self.fallback_failures >= self.config.wifi.fallback.retry {
                self.mode = NetworkMode::Offline;
                self.session = None;
                self.next =
                    now + Duration::from_secs(backoff(self.primary_failures.max(1), &self.config));
                emit(Event::Log("主网与备用网络均不可用，进入退避等待".into()));
            } else {
                self.next = now
                    + Duration::from_secs(self.config.daemon.probe_interval).max(
                        Duration::from_secs_f64(self.config.wifi.fallback.retry_delay),
                    );
            }
        }
    }
    fn schedule_probe(&mut self, now: Instant) {
        self.next = now + Duration::from_secs(self.config.daemon.probe_interval);
    }
}

fn recovery_due(now: Instant, last: Instant, interval: u64) -> bool {
    interval > 0 && now.saturating_duration_since(last) >= Duration::from_secs(interval)
}
fn backoff(failures: u32, config: &AppConfig) -> u64 {
    let settings = &config.daemon.reconnect_backoff;
    (0..failures.saturating_sub(1).min(64))
        .fold(settings.initial, |n, _| {
            n.saturating_mul(settings.factor).min(settings.max)
        })
        .min(settings.max)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        primary: bool,
        fallback: bool,
        internet: bool,
        login: bool,
        primary_calls: u32,
        fallback_calls: u32,
        login_calls: u32,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                primary: false,
                fallback: true,
                internet: false,
                login: false,
                primary_calls: 0,
                fallback_calls: 0,
                login_calls: 0,
            }
        }
    }
    impl Backend for Fake {
        fn ssid(&mut self) -> Result<String> {
            Ok("Campus_WiFi".into())
        }
        fn connect_primary(&mut self, _: &AppConfig) -> Result<bool> {
            self.primary_calls += 1;
            Ok(self.primary)
        }
        fn connect_fallback(&mut self, _: &AppConfig) -> Result<bool> {
            self.fallback_calls += 1;
            Ok(self.fallback)
        }
        fn login(&mut self, _: &AppConfig) -> Result<AuthResult> {
            self.login_calls += 1;
            Ok(AuthResult {
                success: self.login,
                online: self.login,
                session_id: None,
                message: String::new(),
            })
        }
        fn probe(&mut self, _: &AppConfig, _: Option<&str>) -> Result<bool> {
            Ok(self.internet)
        }
    }
    fn engine(now: Instant) -> Engine {
        let mut cfg = AppConfig::default();
        cfg.wifi.fallback.enabled = true;
        cfg.wifi.fallback.ssid = "Fallback_WiFi".into();
        Engine::new(cfg, GuiOverrides::default(), now)
    }
    fn configured_engine(now: Instant) -> Engine {
        let mut cfg = AppConfig::default();
        cfg.portal.entry_url = "https://portal.test/entry".into();
        cfg.portal.api_base = "https://portal.test/api".into();
        cfg.account.username = "synthetic-user".into();
        cfg.account.password = "synthetic-password".into();
        cfg.wifi.fallback.enabled = true;
        cfg.wifi.fallback.ssid = "Fallback_WiFi".into();
        Engine::new(cfg, GuiOverrides::default(), now)
    }
    #[test]
    fn appearance_and_logging_changes_preserve_network_schedule_and_counters() {
        for (mode, paused, fallback_enabled) in [
            (NetworkMode::Primary, false, true),
            (NetworkMode::Fallback, false, true),
            (NetworkMode::Fallback, false, false),
            (NetworkMode::Offline, false, true),
            (NetworkMode::Primary, true, true),
        ] {
            for change in 0..3 {
                let now = Instant::now();
                let mut e = configured_engine(now);
                e.base.wifi.fallback.enabled = fallback_enabled;
                e.config.wifi.fallback.enabled = fallback_enabled;
                e.mode = mode;
                e.next = now + Duration::from_secs(180);
                e.snapshot.next_probe = (!paused).then_some(e.next);
                e.snapshot.paused = paused;
                e.snapshot.online = mode != NetworkMode::Offline;
                e.auth_required = false;
                e.primary_failures = 4;
                e.probe_failures = 2;
                e.fallback_failures = 3;
                let next = e.next;
                let next_probe = e.snapshot.next_probe;
                let online = e.snapshot.online;
                // 显式填写相同网络值不应被误判为需要重新连接。
                let settings = GuiOverrides {
                    probe_interval: Some(e.config.daemon.probe_interval),
                    failure_threshold: Some(e.config.daemon.failure_threshold),
                    appearance: (change != 1).then(crate::config::Appearance::default),
                    log_level: (change != 0).then(|| "DEBUG".into()),
                    ..Default::default()
                };
                assert!(
                    e.apply_saved_config(e.base.effective(&settings), now + Duration::from_secs(1))
                );
                assert_eq!(e.next, next);
                assert_eq!(e.snapshot.next_probe, next_probe);
                assert_eq!(e.primary_failures, 4);
                assert_eq!(e.probe_failures, 2);
                assert_eq!(e.fallback_failures, 3);
                assert_eq!(e.mode, mode);
                assert_eq!(e.snapshot.online, online);
                assert_eq!(e.snapshot.paused, paused);
                assert!(!e.auth_required);
                assert_eq!(e.last_recovery, now);
                assert_eq!(
                    e.config.logging.level,
                    if change == 0 { "INFO" } else { "DEBUG" }
                );
            }
        }
    }
    #[test]
    fn changed_effective_network_values_reschedule_without_resetting_failures() {
        for settings in [
            GuiOverrides {
                probe_interval: Some(60),
                ..Default::default()
            },
            GuiOverrides {
                failure_threshold: Some(5),
                ..Default::default()
            },
            GuiOverrides {
                fallback_enabled: Some(false),
                ..Default::default()
            },
            GuiOverrides {
                fallback_ssid: Some("Another_Synthetic_WiFi".into()),
                ..Default::default()
            },
            GuiOverrides {
                fallback_password: Some("another-synthetic-password".into()),
                ..Default::default()
            },
            GuiOverrides {
                switch_back_interval: Some(300),
                ..Default::default()
            },
        ] {
            for paused in [false, true] {
                let now = Instant::now();
                let mut e = configured_engine(now);
                e.snapshot.paused = paused;
                e.next = now + Duration::from_secs(180);
                e.primary_failures = 4;
                e.probe_failures = 2;
                e.fallback_failures = 3;
                let changed_at = now + Duration::from_secs(1);
                assert!(e.apply_saved_config(e.base.effective(&settings), changed_at));
                assert_eq!(e.next, changed_at);
                assert_eq!(e.snapshot.next_probe, (!paused).then_some(changed_at));
                assert_eq!(e.primary_failures, 4);
                assert_eq!(e.probe_failures, 2);
                assert_eq!(e.fallback_failures, 3);
            }
        }
    }
    #[test]
    fn ineffective_fallback_enable_does_not_reschedule() {
        let now = Instant::now();
        let mut e = configured_engine(now);
        e.base.wifi.fallback.enabled = false;
        e.base.wifi.fallback.ssid.clear();
        e.config = e.base.effective(&GuiOverrides::default());
        e.next = now + Duration::from_secs(180);
        e.snapshot.next_probe = Some(e.next);
        let next = e.next;
        let settings = GuiOverrides {
            fallback_enabled: Some(true),
            ..Default::default()
        };
        assert!(e.apply_saved_config(e.base.effective(&settings), now));
        assert!(!e.config.wifi.fallback.enabled);
        assert_eq!(e.next, next);
        assert_eq!(e.snapshot.next_probe, Some(next));
    }
    #[test]
    fn invalid_inactive_appearance_palette_is_rejected_before_save() {
        let mut settings = GuiOverrides {
            appearance: Some(crate::config::Appearance::default()),
            ..Default::default()
        };
        assert!(settings_valid(&settings));
        settings
            .appearance
            .as_mut()
            .unwrap()
            .chatgpt
            .dark
            .text_color = Some("#bad-color".into());
        assert!(!settings_valid(&settings));
    }
    #[test]
    fn fallback_connected_without_internet_reaches_offline() {
        let now = Instant::now();
        let mut e = engine(now);
        e.mode = NetworkMode::Fallback;
        let mut fake = Fake::default();
        for n in 0..3 {
            e.cycle(&mut fake, &mut |_| {}, now + Duration::from_secs(n * 30));
        }
        assert_eq!(e.fallback_failures, 3);
        assert_eq!(e.mode, NetworkMode::Offline);
        assert!(!e.snapshot.online);
    }
    #[test]
    fn successful_internet_resets_fallback_counter() {
        let now = Instant::now();
        let mut e = engine(now);
        e.mode = NetworkMode::Fallback;
        let mut fake = Fake::default();
        e.cycle(&mut fake, &mut |_| {}, now);
        fake.internet = true;
        e.cycle(&mut fake, &mut |_| {}, now + Duration::from_secs(30));
        assert_eq!(e.fallback_failures, 0);
        assert!(e.snapshot.online);
    }
    #[test]
    fn recovery_uses_elapsed_seconds_not_number_of_ticks() {
        let now = Instant::now();
        assert!(!recovery_due(now + Duration::from_secs(299), now, 300));
        assert!(recovery_due(now + Duration::from_secs(300), now, 300));
        assert!(!recovery_due(now + Duration::from_secs(9000), now, 0));
    }
    #[test]
    fn failed_primary_recovery_restores_fallback() {
        let now = Instant::now();
        let mut e = engine(now);
        e.mode = NetworkMode::Fallback;
        e.config.wifi.fallback.switch_back_interval = 300;
        let mut fake = Fake {
            primary: true,
            internet: true,
            ..Default::default()
        };
        e.cycle(&mut fake, &mut |_| {}, now + Duration::from_secs(300));
        assert_eq!(fake.primary_calls, 1);
        assert_eq!(fake.login_calls, 1);
        assert_eq!(fake.fallback_calls, 1);
        assert_eq!(e.mode, NetworkMode::Fallback);
        assert!(e.snapshot.online);
    }
    #[test]
    fn successful_primary_recovery_changes_mode() {
        let now = Instant::now();
        let mut e = engine(now);
        e.mode = NetworkMode::Fallback;
        e.config.wifi.fallback.switch_back_interval = 300;
        let mut fake = Fake {
            primary: true,
            login: true,
            ..Default::default()
        };
        e.cycle(&mut fake, &mut |_| {}, now + Duration::from_secs(300));
        assert_eq!(e.mode, NetworkMode::Primary);
        assert!(e.snapshot.online);
        assert_eq!(fake.fallback_calls, 0);
    }
    #[test]
    fn login_success_without_session_is_still_online() {
        let now = Instant::now();
        let mut e = engine(now);
        let mut fake = Fake {
            primary: true,
            login: true,
            ..Default::default()
        };
        e.cycle(&mut fake, &mut |_| {}, now);
        assert!(e.snapshot.online);
        assert!(e.session.is_none());
    }
    #[test]
    fn backoff_saturates_without_overflow() {
        let mut cfg = AppConfig::default();
        cfg.daemon.reconnect_backoff.max = 60;
        assert_eq!(backoff(1, &cfg), 5);
        assert_eq!(backoff(2, &cfg), 10);
        assert_eq!(backoff(u32::MAX, &cfg), 60);
    }
    #[test]
    fn rotating_log_honors_size_backups_and_level() {
        let dir = std::env::temp_dir().join(format!("cng-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.log");
        let mut cfg = crate::config::LoggingConfig {
            file: path.to_string_lossy().into_owned(),
            max_bytes: 1,
            backup_count: 2,
            ..Default::default()
        };
        write_log(&cfg, "first").unwrap();
        write_log(&cfg, "second").unwrap();
        write_log(&cfg, "third").unwrap();
        assert!(dir.join("guardian.log.1").is_file());
        assert!(dir.join("guardian.log.2").is_file());
        assert!(!dir.join("guardian.log.3").exists());
        let before = std::fs::read(&path).unwrap();
        cfg.level = "ERROR".into();
        write_log(&cfg, "informational").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }
}
