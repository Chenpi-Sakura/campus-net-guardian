//! 兼容既有 YAML 的配置、环境变量覆盖与 GUI 白名单覆盖层。

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub portal: PortalConfig,
    pub account: AccountConfig,
    pub wifi: WifiConfig,
    pub daemon: DaemonConfig,
    pub logging: LoggingConfig,
    pub env_overrides: BTreeMap<String, String>,
    /// 校园跑账号与默认路线的本机 YAML 配置。
    pub running: RunningSettings,
}

/// 校园跑账号与默认赛道设置，仅保存在本机主 YAML 中。
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RunningSettings {
    /// 校园跑账号手机号。
    #[serde(deserialize_with = "scalar_string")]
    pub phone: String,
    /// 校园跑账号密码，保留原始空格。
    pub password: String,
    /// 默认校园 ID。
    pub default_campus: String,
    /// 赛道目录文件路径。
    pub catalog_file: String,
}

impl Default for RunningSettings {
    fn default() -> Self {
        Self {
            phone: String::new(),
            password: String::new(),
            default_campus: "demo-a".into(),
            catalog_file: String::new(),
        }
    }
}

impl RunningSettings {
    /// 校验校园跑设置，不在错误中包含字段值。
    pub fn validate(&self) -> Result<()> {
        if !self.phone.is_empty()
            && (self.phone.len() != 11 || !self.phone.bytes().all(|byte| byte.is_ascii_digit()))
        {
            bail!("校园跑手机号必须为空或 11 位 ASCII 数字");
        }
        if self.password.len() > 1024 || self.password.contains('\0') {
            bail!("校园跑密码长度或字符无效");
        }
        if self.default_campus.is_empty()
            || self.default_campus.len() > 64
            || !self
                .default_campus
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            bail!("默认校园 ID 格式无效");
        }
        if self.catalog_file.chars().count() > 4096
            || self.catalog_file.contains(['\0', '\r', '\n'])
            || self.catalog_file.starts_with(r"\\")
            || self.catalog_file.starts_with("//")
        {
            bail!("赛道目录文件路径长度或字符无效");
        }
        Ok(())
    }

    /// 仅生成相对上一草稿发生变化的校园跑字段。
    pub fn patch_against(&self, previous: &Self) -> RunningPatch {
        let changed =
            |current: &String, prior: &String| (current != prior).then(|| current.clone());
        RunningPatch {
            phone: changed(&self.phone, &previous.phone),
            password: changed(&self.password, &previous.password),
            default_campus: changed(&self.default_campus, &previous.default_campus),
            catalog_file: changed(&self.catalog_file, &previous.catalog_file),
        }
    }
}

/// 校园跑 YAML 字段白名单；不实现 Debug 或 Serialize，避免凭据意外外泄。
#[derive(Clone, Default)]
pub struct RunningPatch {
    /// 修改手机号。
    pub phone: Option<String>,
    /// 修改密码。
    pub password: Option<String>,
    /// 修改默认校园 ID。
    pub default_campus: Option<String>,
    /// 修改赛道目录文件路径。
    pub catalog_file: Option<String>,
}

impl RunningPatch {
    /// 判断是否没有明确修改的字段。
    pub fn is_empty(&self) -> bool {
        self.phone.is_none()
            && self.password.is_none()
            && self.default_campus.is_none()
            && self.catalog_file.is_none()
    }

    /// 合并最新 YAML 中的校园跑白名单字段并原子写回。
    pub fn save_to(&self, path: &Path) -> Result<PrimarySaveOutcome> {
        if self.is_empty() {
            return Ok(PrimarySaveOutcome::default());
        }
        let target = fs::canonicalize(path).map_err(|_| anyhow::anyhow!("无法读取配置文件"))?;
        let original =
            fs::read_to_string(&target).map_err(|_| anyhow::anyhow!("无法读取配置文件"))?;
        let mut document: serde_json::Value = serde_saphyr::from_str_with_options(
            &original,
            serde_saphyr::options! { no_schema: true },
        )
        .map_err(|_| anyhow::anyhow!("配置格式无效，请先修正 YAML 格式"))?;
        let typed: AppConfig = serde_saphyr::from_str(&original)
            .map_err(|_| anyhow::anyhow!("配置格式无效，请检查 YAML 字段与类型"))?;
        restore_known_string_values(&mut document, &typed);
        let running = RunningSettings {
            phone: self.phone.clone().unwrap_or(typed.running.phone),
            password: self.password.clone().unwrap_or(typed.running.password),
            default_campus: self
                .default_campus
                .clone()
                .unwrap_or(typed.running.default_campus),
            catalog_file: self
                .catalog_file
                .clone()
                .unwrap_or(typed.running.catalog_file),
        };
        running.validate()?;
        let root = document
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("配置顶层必须是 YAML 映射"))?;
        let section = root
            .entry("running")
            .or_insert_with(|| serde_json::json!({}));
        if section.is_null() {
            *section = serde_json::json!({});
        }
        let section = section
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("校园跑配置必须是 YAML 映射"))?;
        for (name, value) in [
            ("phone", &running.phone),
            ("password", &running.password),
            ("default_campus", &running.default_campus),
            ("catalog_file", &running.catalog_file),
        ] {
            section.insert(name.into(), serde_json::Value::String(value.clone()));
        }
        let serialized =
            serde_saphyr::to_string(&document).map_err(|_| anyhow::anyhow!("无法生成配置文件"))?;
        let parsed: AppConfig = serde_saphyr::from_str(&serialized)
            .map_err(|_| anyhow::anyhow!("配置格式无效，请检查 YAML 字段与类型"))?;
        parsed.running.validate()?;
        write_primary_atomic(&target, &original, serialized.as_bytes())
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PortalConfig {
    pub entry_url: String,
    pub api_base: String,
    pub probe_url: String,
    pub service: String,
    pub timeout: u64,
    pub user_agent: String,
}
impl Default for PortalConfig {
    fn default() -> Self {
        Self {
            entry_url: String::new(),
            api_base: String::new(),
            probe_url: "http://www.sina.com.cn/".into(),
            service: "cmcc".into(),
            timeout: 10,
            user_agent: "campus-guardian/0.2".into(),
        }
    }
}

#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct AccountConfig {
    #[serde(deserialize_with = "scalar_string")]
    pub username: String,
    #[serde(deserialize_with = "scalar_string")]
    pub password: String,
}

fn scalar_string<'de, D: serde::Deserializer<'de>>(de: D) -> std::result::Result<String, D::Error> {
    let value = serde_json::Value::deserialize(de)?;
    match value {
        serde_json::Value::String(v) => Ok(v),
        serde_json::Value::Number(v) => Ok(v.to_string()),
        serde_json::Value::Null => Ok(String::new()),
        _ => Err(serde::de::Error::custom("账号字段必须是字符串或数字")),
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct WifiConfig {
    pub ssid: String,
    pub profile_xml: String,
    pub connect_timeout: u64,
    pub fallback: FallbackWifiConfig,
}
impl Default for WifiConfig {
    fn default() -> Self {
        Self {
            ssid: String::new(),
            profile_xml: String::new(),
            connect_timeout: 15,
            fallback: FallbackWifiConfig::default(),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct FallbackWifiConfig {
    pub enabled: bool,
    pub ssid: String,
    pub password: String,
    pub retry: u32,
    pub retry_delay: f64,
    pub connect_timeout: u64,
    pub encryption: String,
    pub auth: String,
    pub switch_back_interval: u64,
    pub revert_on_stop: bool,
}
impl Default for FallbackWifiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ssid: String::new(),
            password: String::new(),
            retry: 3,
            retry_delay: 2.0,
            connect_timeout: 20,
            encryption: "WPA2PSK".into(),
            auth: "AES".into(),
            switch_back_interval: 0,
            revert_on_stop: false,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub probe_interval: u64,
    pub failure_threshold: u32,
    pub reconnect_backoff: BackoffConfig,
}
impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            probe_interval: 30,
            failure_threshold: 3,
            reconnect_backoff: BackoffConfig::default(),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct BackoffConfig {
    pub initial: u64,
    pub max: u64,
    pub factor: u64,
}
impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial: 5,
            max: 300,
            factor: 2,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub file: String,
    pub max_bytes: u64,
    pub backup_count: u32,
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "INFO".into(),
            file: String::new(),
            max_bytes: 10 * 1024 * 1024,
            backup_count: 5,
        }
    }
}

impl AppConfig {
    /// 加载 YAML 并应用显式映射的环境变量，不在错误中输出原文或凭据。
    pub fn load(path: &Path) -> Result<Self> {
        let config = Self::read(path)?;
        config.validate()?;
        Ok(config)
    }

    /// 解析并规范化配置及环境变量覆盖，保留尚未完成的主网络字段供设置页修正。
    pub fn read(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path).context("无法读取配置文件")?;
        Self::parse_with_env(&contents, |key| std::env::var(key).ok())
    }

    fn parse_with_env(contents: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let mut config: Self = serde_saphyr::from_str(contents)
            .map_err(|_| anyhow::anyhow!("配置格式无效，请检查 YAML 字段与类型"))?;
        config.apply_env(lookup);
        config.portal.service = config.portal.service.trim().to_lowercase();
        config.logging.level = config.logging.level.trim().to_uppercase();
        config.portal.entry_url = config.portal.entry_url.trim().into();
        config.portal.api_base = config.portal.api_base.trim().into();
        config.portal.probe_url = config.portal.probe_url.trim().into();
        config.account.username = config.account.username.trim().into();
        Ok(config)
    }

    fn apply_env(&mut self, lookup: impl Fn(&str) -> Option<String>) {
        for (key, name) in &self.env_overrides {
            let Some(value) = lookup(name).filter(|v| !v.is_empty()) else {
                continue;
            };
            match key.as_str() {
                "username" => self.account.username = value.trim().into(),
                "password" => self.account.password = value,
                "service" => self.portal.service = value.trim().to_lowercase(),
                "log_level" => self.logging.level = value.trim().to_uppercase(),
                _ => {}
            }
        }
    }

    /// 校验范围与敏感字段，避免错误配置造成忙循环或隐式降级。
    pub fn validate(&self) -> Result<()> {
        if std::env::var("CG_BIND_INTERFACE_IP").ok().is_some_and(|v| {
            !v.trim().is_empty() && v.trim() != "auto" && !valid_source_ipv4(v.trim())
        }) {
            bail!("绑源 IP 环境变量无效，必须填写有效的单播 IPv4 地址或 auto");
        }
        for field in [
            &self.portal.entry_url,
            &self.portal.api_base,
            &self.portal.probe_url,
        ] {
            let url = url::Url::parse(field).context("门户或探测地址无效")?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                bail!("地址必须使用 http/https，且不得内嵌账密");
            }
        }
        if self.portal.entry_url.contains("__PORTAL_HOST__")
            || self.portal.api_base.contains("__PORTAL_HOST__")
            || self.account.username.trim().is_empty()
            || self.account.password.is_empty()
            || matches!(
                self.account.username.as_str(),
                "__REPLACE_ME__" | "__USERNAME__"
            )
            || matches!(
                self.account.password.as_str(),
                "__REPLACE_ME__" | "__PASSWORD__"
            )
        {
            bail!("请先填写门户地址与校园网账号密码");
        }
        if !matches!(self.portal.service.as_str(), "cmcc" | "ctcc" | "unicom") {
            bail!("运营商必须为 cmcc、ctcc 或 unicom");
        }
        if !(1..=120).contains(&self.portal.timeout)
            || !(1..=3600).contains(&self.daemon.probe_interval)
            || !(1..=100).contains(&self.daemon.failure_threshold)
        {
            bail!("请求超时、探测间隔或失败阈值超出安全范围");
        }
        let backoff = &self.daemon.reconnect_backoff;
        if backoff.initial == 0
            || backoff.initial > backoff.max
            || backoff.max > 86400
            || !(1..=10).contains(&backoff.factor)
        {
            bail!("重连退避参数无效");
        }
        if !(1..=120).contains(&self.wifi.connect_timeout) {
            bail!("主网络连接超时必须为 1–120 秒");
        }
        let fb = &self.wifi.fallback;
        if self.wifi.ssid.len() > 32
            || self.wifi.ssid.contains('\0')
            || fb.ssid.len() > 32
            || fb.ssid.contains('\0')
        {
            bail!("WiFi SSID 必须是最多 32 字节且不含 NUL 的名称");
        }
        if self.wifi.ssid == "__WIFI_SSID__" {
            bail!("请先填写校园网 SSID，使用有线网络时可留空");
        }
        if fb.enabled && (fb.ssid.trim().is_empty() || fb.ssid == "__FALLBACK_WIFI_SSID__") {
            bail!("启用备用网络前请填写 SSID");
        }
        if !(1..=10).contains(&fb.retry)
            || !(1..=120).contains(&fb.connect_timeout)
            || !fb.retry_delay.is_finite()
            || !(0.0..=60.0).contains(&fb.retry_delay)
        {
            bail!("备用网络重试或超时参数无效");
        }
        if !matches!(
            fb.encryption.as_str(),
            "open" | "WPAPSK" | "WPA2PSK" | "WPA3SAE"
        ) || !matches!(fb.auth.as_str(), "open" | "TKIP" | "AES")
        {
            bail!("备用网络安全类型无效");
        }
        if !matches!(
            self.logging.level.to_uppercase().as_str(),
            "DEBUG" | "INFO" | "WARNING" | "ERROR"
        ) {
            bail!("日志级别无效");
        }
        if self.logging.backup_count > 100 {
            bail!("日志备份数量不得超过 100");
        }
        Ok(())
    }

    /// 应用 GUI 白名单覆盖项，保留 YAML 中的账号和门户契约。
    pub fn effective(&self, overrides: &GuiOverrides) -> Self {
        let mut out = self.clone();
        if let Some(v) = overrides.probe_interval {
            out.daemon.probe_interval = v;
        }
        if let Some(v) = overrides.failure_threshold {
            out.daemon.failure_threshold = v;
        }
        if let Some(v) = overrides.fallback_enabled {
            out.wifi.fallback.enabled = v;
        }
        if let Some(v) = &overrides.fallback_ssid {
            out.wifi.fallback.ssid = v.clone();
        }
        if let Some(v) = &overrides.fallback_password {
            out.wifi.fallback.password = v.clone();
        }
        if let Some(v) = overrides.switch_back_interval {
            out.wifi.fallback.switch_back_interval = v;
        }
        if let Some(v) = &overrides.log_level {
            out.logging.level = v.clone();
        }
        if out.wifi.fallback.ssid.trim().is_empty() {
            out.wifi.fallback.enabled = false;
        }
        out
    }
}

fn valid_source_ipv4(value: &str) -> bool {
    value
        .parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast())
}

/// 主网络编辑草稿；敏感字段只保存到 YAML，不进入 GUI 覆盖层。
#[derive(Clone, PartialEq, Eq)]
pub struct PrimarySettings {
    /// 主网络名称，留空表示使用有线网络。
    pub ssid: String,
    /// 校园网账号。
    pub username: String,
    /// 校园网密码，保留原始空格。
    pub password: String,
    /// 运营商标识。
    pub service: String,
    /// 门户入口地址。
    pub entry_url: String,
    /// 门户接口地址。
    pub api_base: String,
    /// 连通性探测地址。
    pub probe_url: String,
}

impl PrimarySettings {
    /// 从已解析配置构建显示草稿；环境变量的有效值不会被隐式回写。
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            ssid: config.wifi.ssid.clone(),
            username: config.account.username.clone(),
            password: config.account.password.clone(),
            service: config.portal.service.clone(),
            entry_url: config.portal.entry_url.clone(),
            api_base: config.portal.api_base.clone(),
            probe_url: config.portal.probe_url.clone(),
        }
    }

    /// 只生成相对原显示草稿明确改动的字段，避免覆盖 YAML 中的其他真实值。
    pub fn patch_against(&self, previous: &Self) -> PrimaryPatch {
        let changed =
            |current: &String, previous: &String| (current != previous).then(|| current.clone());
        PrimaryPatch {
            ssid: changed(&self.ssid, &previous.ssid),
            username: changed(&self.username, &previous.username),
            password: changed(&self.password, &previous.password),
            service: changed(&self.service, &previous.service),
            entry_url: changed(&self.entry_url, &previous.entry_url),
            api_base: changed(&self.api_base, &previous.api_base),
            probe_url: changed(&self.probe_url, &previous.probe_url),
        }
    }
}

/// YAML 主网络字段白名单；不实现 Debug 或 Serialize，避免凭据进入通用消息或 JSON。
#[derive(Clone, Default)]
pub struct PrimaryPatch {
    /// 修改主网络名称。
    pub ssid: Option<String>,
    /// 修改账号。
    pub username: Option<String>,
    /// 修改密码。
    pub password: Option<String>,
    /// 修改运营商。
    pub service: Option<String>,
    /// 修改门户入口地址。
    pub entry_url: Option<String>,
    /// 修改门户接口地址。
    pub api_base: Option<String>,
    /// 修改探测地址。
    pub probe_url: Option<String>,
}

/// 主网络写回已提交的结果；警告仅为固定维护提示，不含配置或凭据。
#[derive(Debug, Default)]
pub struct PrimarySaveOutcome {
    /// 已提交后发生的非致命清理问题，不能作为保存失败回执。
    pub warning: Option<&'static str>,
}

impl PrimaryPatch {
    /// 判断是否没有明确修改的字段。
    pub fn is_empty(&self) -> bool {
        [
            &self.ssid,
            &self.username,
            &self.password,
            &self.service,
            &self.entry_url,
            &self.api_base,
            &self.probe_url,
        ]
        .iter()
        .all(|value| value.is_none())
    }

    /// 合并当前 YAML、验证完整配置并原子写回；仅在重启后生效。
    pub fn save_to(&self, path: &Path) -> Result<PrimarySaveOutcome> {
        self.save_to_with_env(path, |name| std::env::var(name).ok())
    }

    fn save_to_with_env(
        &self,
        path: &Path,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<PrimarySaveOutcome> {
        if self.is_empty() {
            return Ok(PrimarySaveOutcome::default());
        }
        let target =
            fs::canonicalize(path).map_err(|_| anyhow::anyhow!("无法读取主网络配置文件"))?;
        let original =
            fs::read_to_string(&target).map_err(|_| anyhow::anyhow!("无法读取主网络配置文件"))?;
        let mut document: serde_json::Value = serde_saphyr::from_str_with_options(
            &original,
            serde_saphyr::options! { no_schema: true },
        )
        .map_err(|_| anyhow::anyhow!("配置格式无效，请先修正 YAML 格式"))?;
        let typed: AppConfig = serde_saphyr::from_str(&original)
            .map_err(|_| anyhow::anyhow!("配置格式无效，请检查 YAML 字段与类型"))?;
        restore_known_string_values(&mut document, &typed);
        let root = document
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("配置顶层必须是 YAML 映射"))?;
        for (section, field, label, value) in [
            ("wifi", "ssid", "主网络名称", &self.ssid),
            ("account", "username", "账号", &self.username),
            ("account", "password", "密码", &self.password),
            ("portal", "service", "运营商", &self.service),
            ("portal", "entry_url", "门户入口地址", &self.entry_url),
            ("portal", "api_base", "门户接口地址", &self.api_base),
            ("portal", "probe_url", "探测地址", &self.probe_url),
        ] {
            let Some(value) = value else {
                continue;
            };
            // 与 apply_env 保持一致：仅支持的映射键、变量存在且非空时才覆盖 YAML。
            if matches!(field, "username" | "password" | "service")
                && root
                    .get("env_overrides")
                    .and_then(|env| env.get(field))
                    .and_then(serde_json::Value::as_str)
                    .and_then(&lookup)
                    .is_some_and(|value| !value.is_empty())
            {
                bail!("{label}当前由环境变量覆盖，请先调整环境变量或配置映射");
            }
            let normalized = match field {
                "username" | "entry_url" | "api_base" | "probe_url" => value.trim().to_owned(),
                "service" => value.trim().to_lowercase(),
                _ => value.clone(),
            };
            let nested = root.entry(section).or_insert_with(|| serde_json::json!({}));
            if nested.is_null() {
                *nested = serde_json::json!({});
            }
            let nested = nested
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("配置中的{section}必须是 YAML 映射"))?;
            nested.insert(field.into(), serde_json::Value::String(normalized));
        }
        let serialized = serde_saphyr::to_string(&document)
            .map_err(|_| anyhow::anyhow!("无法生成主网络配置"))?;
        AppConfig::parse_with_env(&serialized, lookup)?
            .validate()
            .context("主网络设置无效，未保存")?;
        write_primary_atomic(&target, &original, serialized.as_bytes())
    }
}

fn restore_known_string_values(document: &mut serde_json::Value, original: &AppConfig) {
    // Value 的 schema 推断不能更改既有 String 语义；不注入缺省值，也不读取有效 env。
    for (path, value) in [
        ("/portal/entry_url", &original.portal.entry_url),
        ("/portal/api_base", &original.portal.api_base),
        ("/portal/probe_url", &original.portal.probe_url),
        ("/portal/service", &original.portal.service),
        ("/portal/user_agent", &original.portal.user_agent),
        ("/account/username", &original.account.username),
        ("/account/password", &original.account.password),
        ("/wifi/ssid", &original.wifi.ssid),
        ("/wifi/profile_xml", &original.wifi.profile_xml),
        ("/wifi/fallback/ssid", &original.wifi.fallback.ssid),
        ("/wifi/fallback/password", &original.wifi.fallback.password),
        (
            "/wifi/fallback/encryption",
            &original.wifi.fallback.encryption,
        ),
        ("/wifi/fallback/auth", &original.wifi.fallback.auth),
        ("/logging/file", &original.logging.file),
        ("/logging/level", &original.logging.level),
        ("/running/phone", &original.running.phone),
        ("/running/password", &original.running.password),
        ("/running/default_campus", &original.running.default_campus),
        ("/running/catalog_file", &original.running.catalog_file),
    ] {
        if let Some(current) = document.pointer_mut(path)
            && !current.is_string()
        {
            *current = serde_json::Value::String(value.clone());
        }
    }
    if let Some(mapping) = document
        .get_mut("env_overrides")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (name, value) in &original.env_overrides {
            if let Some(current) = mapping.get_mut(name)
                && !current.is_string()
            {
                *current = serde_json::Value::String(value.clone());
            }
        }
    }
}

fn write_primary_atomic(
    path: &Path,
    original: &str,
    contents: &[u8],
) -> Result<PrimarySaveOutcome> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    // *.local.yaml 已在仓库忽略规则中；即使异常退出，凭据临时文件也不会混入提交。
    let temporary = path.with_file_name(format!(
        ".cng-primary-{}-{sequence}.local.yaml",
        std::process::id()
    ));
    #[cfg(windows)]
    let mut file = primary_windows_file::create(&temporary, path)?;
    #[cfg(not(windows))]
    let mut file = {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&temporary)
            .map_err(|_| anyhow::anyhow!("无法创建主网络配置临时文件"))?
    };
    #[cfg(windows)]
    let mut cleanup_temporary = true;
    let result = (|| {
        file.write_all(contents)
            .map_err(|_| anyhow::anyhow!("无法写入主网络配置"))?;
        file.sync_all()
            .map_err(|_| anyhow::anyhow!("无法同步主网络配置"))?;
        drop(file);
        if fs::read_to_string(path).map_err(|_| anyhow::anyhow!("无法复核主网络配置"))? != original
        {
            bail!("配置文件已被其他程序修改，请重试");
        }
        #[cfg(windows)]
        let outcome = {
            // ReplaceFileW 少数失败可能已改名原文件；由专用路径处理恢复，不能通用删除。
            cleanup_temporary = false;
            let backup = path.with_file_name(format!(
                ".cng-primary-backup-{}-{sequence}.local.yaml",
                std::process::id()
            ));
            primary_windows_file::replace(path, &temporary, &backup, original)?
        };
        #[cfg(not(windows))]
        let outcome = {
            fs::rename(&temporary, path).map_err(|_| anyhow::anyhow!("无法替换主网络配置文件"))?;
            PrimarySaveOutcome::default()
        };
        Ok(outcome)
    })();
    #[cfg(not(windows))]
    let cleanup_temporary = true;
    if result.is_err() && cleanup_temporary {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
mod primary_windows_file {
    use super::*;
    use std::{
        ffi::c_void,
        mem::size_of,
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError, INVALID_HANDLE_VALUE},
        Security::{
            DACL_SECURITY_INFORMATION, GetFileSecurityW, GetSecurityDescriptorDacl,
            SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SetSecurityDescriptorControl,
        },
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const SECURITY_ATTRIBUTES,
            disposition: u32,
            attributes: u32,
            template: *mut c_void,
        ) -> *mut c_void;
        fn ReplaceFileW(
            target: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
        fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub(super) fn create(temporary: &Path, original: &Path) -> Result<fs::File> {
        let original = wide(original);
        let mut needed = 0;
        // 两阶段读取 self-relative 描述符；Vec<u32> 提供要求的 DWORD 对齐。
        let queried = unsafe {
            GetFileSecurityW(
                original.as_ptr(),
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if queried != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || needed == 0 {
            bail!("无法读取主网络配置的安全权限，未写入凭据");
        }
        let mut descriptor = vec![0u32; (needed as usize).div_ceil(size_of::<u32>())];
        let security = descriptor.as_mut_ptr().cast::<c_void>();
        if unsafe {
            GetFileSecurityW(
                original.as_ptr(),
                DACL_SECURITY_INFORMATION,
                security,
                needed,
                &mut needed,
            )
        } == 0
        {
            bail!("无法读取主网络配置的安全权限，未写入凭据");
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        if unsafe { GetSecurityDescriptorDacl(security, &mut present, &mut dacl, &mut defaulted) }
            == 0
            || present == 0
            || dacl.is_null()
        {
            bail!("主网络配置没有可安全复制的访问控制，未写入凭据");
        }
        // 临时文件不得额外继承目录中更宽的 ACE；原文件本身不受此修改影响。
        if unsafe { SetSecurityDescriptorControl(security, SE_DACL_PROTECTED, SE_DACL_PROTECTED) }
            == 0
        {
            bail!("无法保护主网络配置临时文件的权限，未写入凭据");
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security,
            bInheritHandle: 0,
        };
        let temporary = wide(temporary);
        // GENERIC_WRITE / 无共享 / CREATE_NEW / FILE_ATTRIBUTE_NORMAL；安全描述符在创建时生效。
        let handle = unsafe {
            CreateFileW(
                temporary.as_ptr(),
                0x4000_0000,
                0,
                &attributes,
                1,
                0x80,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            bail!("无法安全创建主网络配置临时文件");
        }
        // CreateFileW 已复制描述符；唯一有效句柄交由 File 持有并在 Drop 时关闭。
        Ok(unsafe { fs::File::from_raw_handle(handle) })
    }

    pub(super) fn replace(
        target: &Path,
        temporary: &Path,
        backup: &Path,
        original: &str,
    ) -> Result<PrimarySaveOutcome> {
        if backup.exists() {
            let _ = fs::remove_file(temporary);
            bail!("配置恢复文件已存在，请检查配置目录后重试");
        }
        let target_wide = wide(target);
        let temporary_wide = wide(temporary);
        let backup_wide = wide(backup);
        // 不忽略 ACL 合并错误；备份是原文件的同目录改名，保留其权限和原始内容。
        if unsafe {
            ReplaceFileW(
                target_wide.as_ptr(),
                temporary_wide.as_ptr(),
                backup_wide.as_ptr(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } != 0
        {
            let warning = fs::remove_file(backup)
                .err()
                .map(|_| "主网络设置已保存，但私有旧配置清理失败，请检查配置目录");
            return Ok(PrimarySaveOutcome { warning });
        }
        // 1176/1177 等部分失败可能将原文件移到 backup；只在目标缺失时尝试无覆盖恢复。
        if !target.exists() && backup.exists() {
            let _ = unsafe { MoveFileExW(backup_wide.as_ptr(), target_wide.as_ptr(), 0x8) };
        }
        if fs::read_to_string(target).is_ok_and(|contents| contents == original) {
            let _ = fs::remove_file(temporary);
            let _ = fs::remove_file(backup);
            bail!("无法替换主网络配置，原配置已保留，请重试");
        }
        // 无法确认原文件恢复时保留双方私有文件，不能删除唯一可恢复的数据。
        bail!("配置替换未完成，私有恢复文件已保留，请检查配置目录后重试")
    }
}

/// 界面预设风格，各预设独立保留浅色和深色自定义配色。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeStyle {
    #[default]
    Claude,
    ChatGpt,
}

impl ThemeStyle {
    /// 所有可选预设，顺序与设置界面一致。
    pub const ALL: &'static [Self] = &[Self::Claude, Self::ChatGpt];
}

impl fmt::Display for ThemeStyle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Claude => "Claude 风格",
            Self::ChatGpt => "ChatGPT 风格",
        })
    }
}

/// 界面明暗模式；系统模式由界面根据当前 Windows 设置解析。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    /// 所有可选明暗模式，顺序与设置界面一致。
    pub const ALL: &'static [Self] = &[Self::System, Self::Light, Self::Dark];
}

impl fmt::Display for ThemeMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::System => "跟随系统",
            Self::Light => "浅色",
            Self::Dark => "深色",
        })
    }
}

/// 单套配色覆盖项；留空的颜色继续使用预设默认值。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ColorOverrides {
    pub accent_color: Option<String>,
    pub background_color: Option<String>,
    pub surface_color: Option<String>,
    pub text_color: Option<String>,
}

impl ColorOverrides {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("强调色", &self.accent_color),
            ("背景色", &self.background_color),
            ("面板色", &self.surface_color),
            ("文字色", &self.text_color),
        ] {
            if let Some(value) = value {
                let bytes = value.as_bytes();
                if bytes.len() != 7
                    || bytes[0] != b'#'
                    || !bytes[1..].iter().all(u8::is_ascii_hexdigit)
                {
                    bail!("{name}必须使用 #RRGGBB 格式的六位十六进制颜色");
                }
            }
        }
        Ok(())
    }
}

/// 同一预设在浅色和深色模式下分别保存的配色。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct PresetColors {
    pub light: ColorOverrides,
    pub dark: ColorOverrides,
}

/// GUI 专用外观设置，不进入 YAML 或后台网络配置。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub style: ThemeStyle,
    pub mode: ThemeMode,
    pub claude: PresetColors,
    pub chatgpt: PresetColors,
}

impl Appearance {
    /// 获取当前预设在指定实际明暗模式下的配色覆盖项。
    pub fn colors(&self, dark: bool) -> &ColorOverrides {
        let preset = match self.style {
            ThemeStyle::Claude => &self.claude,
            ThemeStyle::ChatGpt => &self.chatgpt,
        };
        if dark { &preset.dark } else { &preset.light }
    }

    /// 编辑当前预设的单套配色，保留其它预设和明暗模式的自定义值。
    pub fn colors_mut(&mut self, dark: bool) -> &mut ColorOverrides {
        let preset = match self.style {
            ThemeStyle::Claude => &mut self.claude,
            ThemeStyle::ChatGpt => &mut self.chatgpt,
        };
        if dark {
            &mut preset.dark
        } else {
            &mut preset.light
        }
    }

    /// 校验全部四套配色的颜色语法，不在错误信息中回显输入内容。
    pub fn validate(&self) -> Result<()> {
        for (name, preset) in [("Claude", &self.claude), ("ChatGPT", &self.chatgpt)] {
            preset
                .light
                .validate()
                .with_context(|| format!("{name} 浅色配色无效"))?;
            preset
                .dark
                .validate()
                .with_context(|| format!("{name} 深色配色无效"))?;
        }
        Ok(())
    }
}

fn appearance_from_json(value: &serde_json::Value) -> Option<Appearance> {
    // Serde 派生结构体也接受数组；GUI JSON 契约只允许命名对象，避免 [] 被补成默认值。
    let appearance = value.as_object()?;
    for name in ["claude", "chatgpt"] {
        if let Some(preset) = appearance.get(name) {
            let preset = preset.as_object()?;
            for mode in ["light", "dark"] {
                if let Some(colors) = preset.get(mode) {
                    colors.as_object()?;
                }
            }
        }
    }
    serde_json::from_value::<Appearance>(value.clone())
        .ok()
        .filter(|appearance| appearance.validate().is_ok())
}

/// 界面显示缩放的百分比预设；独立于操作系统 DPI 与主题配色。
pub const DISPLAY_SCALE_PRESETS: &[u16] = &[50, 67, 90, 100, 110, 125, 150, 200];

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiOverrides {
    pub probe_interval: Option<u64>,
    pub failure_threshold: Option<u32>,
    pub fallback_enabled: Option<bool>,
    pub fallback_ssid: Option<String>,
    pub fallback_password: Option<String>,
    pub switch_back_interval: Option<u64>,
    pub log_level: Option<String>,
    pub appearance: Option<Appearance>,
    pub display_scale: Option<u16>,
}
impl GuiOverrides {
    /// 返回合法显示缩放，旧配置或无效值使用 100%。
    pub fn display_scale_percent(&self) -> u16 {
        self.display_scale
            .filter(|v| DISPLAY_SCALE_PRESETS.contains(v))
            .unwrap_or(100)
    }

    /// 容错读取既有 GUI JSON；错误字段单独忽略，不丢弃其它合法设置。
    pub fn load() -> Self {
        Self::load_from(&data_dir().join("gui_settings.json"))
    }
    /// 从指定路径读取覆盖项，供离线测试与诊断复用。
    pub fn load_from(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Self::default();
        };
        Self {
            probe_interval: raw["probe_interval"]
                .as_u64()
                .filter(|v| [15, 30, 60, 120].contains(v)),
            failure_threshold: raw["failure_threshold"]
                .as_u64()
                .filter(|v| [2, 3, 5].contains(v))
                .map(|v| v as u32),
            fallback_enabled: raw["fallback_enabled"].as_bool(),
            fallback_ssid: raw["fallback_ssid"].as_str().map(|v| v.trim().to_owned()),
            fallback_password: raw["fallback_password"].as_str().map(str::to_owned),
            switch_back_interval: raw["switch_back_interval"]
                .as_u64()
                .filter(|v| [0, 300, 900].contains(v)),
            log_level: raw["log_level"]
                .as_str()
                .map(str::to_uppercase)
                .filter(|v| matches!(v.as_str(), "INFO" | "DEBUG")),
            appearance: appearance_from_json(&raw["appearance"]),
            display_scale: raw["display_scale"]
                .as_u64()
                .and_then(|v| u16::try_from(v).ok())
                .filter(|v| DISPLAY_SCALE_PRESETS.contains(v)),
        }
    }

    /// 只保存 GUI 白名单键，不保存账号、门户或主网 SSID。
    pub fn save(&self) -> Result<()> {
        self.save_to(&data_dir().join("gui_settings.json"))
    }
    /// 原子保存到指定路径，失败时清理本次临时文件。
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if self
            .display_scale
            .is_some_and(|v| !DISPLAY_SCALE_PRESETS.contains(&v))
        {
            bail!("显示缩放必须使用预设值");
        }
        if let Some(appearance) = &self.appearance {
            appearance.validate()?;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("无法创建设置目录")?;
        }
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        let result = (|| {
            fs::write(&tmp, serde_json::to_vec_pretty(self)?).context("无法写入设置文件")?;
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                let source: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
                let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn MoveFileExW(a: *const u16, b: *const u16, flags: u32) -> i32;
                }
                if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0x1 | 0x8) } == 0 {
                    return Err(std::io::Error::last_os_error()).context("无法替换设置文件");
                }
            }
            #[cfg(not(windows))]
            fs::rename(&tmp, path).context("无法替换设置文件")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

/// 返回当前用户的数据目录，不读取其他用户的配置。
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|v| v.parent().map(Path::to_owned))
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("campus-guardian")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY_YAML: &str = r#"
portal:
  entry_url: https://portal.test/entry
  api_base: https://portal.test/api
  probe_url: https://probe.test/ping
  service: cmcc
account:
  username: synthetic-user
  password: synthetic-stored-password
wifi:
  ssid: Synthetic_Campus_WiFi
  extra: [1, two, false]
daemon:
  probe_interval: 47
  failure_threshold: 7
logging:
  level: WARNING
unknown:
  nested: {keep: value, number: 23, enabled: false}
env_overrides: {}
"#;

    fn with_primary_yaml(contents: &str, test: impl FnOnce(&Path)) {
        static NEXT_TEST: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cng-primary-test-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        fs::write(&path, contents).unwrap();
        test(&path);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn primary_patch_only_writes_explicit_whitelisted_fields_and_preserves_unknown_values() {
        with_primary_yaml(PRIMARY_YAML, |path| {
            let config = AppConfig::parse_with_env(PRIMARY_YAML, |_| None).unwrap();
            config.validate().unwrap();
            let previous = PrimarySettings::from_config(&config);
            assert!(previous.patch_against(&previous).is_empty());
            let mut draft = previous.clone();
            draft.ssid = "Another_Synthetic_WiFi".into();
            let patch = draft.patch_against(&previous);
            assert!(
                patch.username.is_none() && patch.password.is_none() && patch.entry_url.is_none()
            );
            patch.save_to_with_env(path, |_| None).unwrap();
            let mut expected: serde_json::Value = serde_saphyr::from_str(PRIMARY_YAML).unwrap();
            expected["wifi"]["ssid"] = serde_json::json!("Another_Synthetic_WiFi");
            let actual: serde_json::Value =
                serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
            let json = serde_json::to_value(GuiOverrides::default()).unwrap();
            for field in [
                "ssid",
                "username",
                "password",
                "service",
                "entry_url",
                "api_base",
                "probe_url",
            ] {
                assert!(json.get(field).is_none());
            }
        });
    }

    #[test]
    fn read_preserves_incomplete_template_and_full_primary_patch_can_complete_it() {
        let mut template: serde_json::Value =
            serde_saphyr::from_str(include_str!("../../config.example.yaml")).unwrap();
        // read 的回归不查用户进程中的真实凭据环境变量。
        template["env_overrides"] = serde_json::json!({});
        let template = serde_saphyr::to_string(&template).unwrap();
        with_primary_yaml(&template, |path| {
            let config = AppConfig::read(path).unwrap();
            assert_eq!(config.wifi.ssid, "__WIFI_SSID__");
            assert!(config.validate().is_err());
            let patch = PrimaryPatch {
                ssid: Some("Synthetic_Campus_WiFi".into()),
                username: Some("synthetic-user".into()),
                password: Some("synthetic-password".into()),
                service: Some(" CTCC ".into()),
                entry_url: Some(" https://portal.test/entry ".into()),
                api_base: Some("https://portal.test/api".into()),
                probe_url: Some("https://probe.test/ping".into()),
            };
            patch.save_to_with_env(path, |_| None).unwrap();
            let loaded =
                AppConfig::parse_with_env(&fs::read_to_string(path).unwrap(), |_| None).unwrap();
            loaded.validate().unwrap();
            assert_eq!(loaded.portal.service, "ctcc");
            assert_eq!(loaded.portal.entry_url, "https://portal.test/entry");
            assert_eq!(loaded.account.password, "synthetic-password");
        });
    }

    #[test]
    fn effective_environment_credentials_are_never_copied_into_unmodified_yaml_fields() {
        let yaml = PRIMARY_YAML.replace(
            "env_overrides: {}",
            "env_overrides: {username: SYNTHETIC_USER, password: SYNTHETIC_PASSWORD}",
        );
        with_primary_yaml(&yaml, |path| {
            let lookup = |key: &str| {
                Some(
                    if key == "SYNTHETIC_USER" {
                        "synthetic-effective-user"
                    } else {
                        "synthetic-effective-password"
                    }
                    .to_owned(),
                )
            };
            let effective = AppConfig::parse_with_env(&yaml, lookup).unwrap();
            let previous = PrimarySettings::from_config(&effective);
            let mut draft = previous.clone();
            draft.ssid = "Another_Synthetic_WiFi".into();
            draft
                .patch_against(&previous)
                .save_to_with_env(path, lookup)
                .unwrap();
            let raw: serde_json::Value =
                serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(raw["account"]["username"], "synthetic-user");
            assert_eq!(raw["account"]["password"], "synthetic-stored-password");
            assert_eq!(raw["env_overrides"]["password"], "SYNTHETIC_PASSWORD");
        });
    }

    #[test]
    fn active_environment_mapping_rejects_changes_without_echoing_values() {
        for field in ["username", "password", "service"] {
            let yaml = PRIMARY_YAML.replace(
                "env_overrides: {}",
                &format!("env_overrides: {{{field}: SYNTHETIC_VARIABLE}}"),
            );
            with_primary_yaml(&yaml, |path| {
                let patch = match field {
                    "username" => PrimaryPatch {
                        username: Some("synthetic-secret-input".into()),
                        ..PrimaryPatch::default()
                    },
                    "password" => PrimaryPatch {
                        password: Some("synthetic-secret-input".into()),
                        ..PrimaryPatch::default()
                    },
                    _ => PrimaryPatch {
                        service: Some("ctcc".into()),
                        ..PrimaryPatch::default()
                    },
                };
                let before = fs::read(path).unwrap();
                let error = patch
                    .save_to_with_env(path, |_| Some("synthetic-secret-environment".into()))
                    .unwrap_err();
                let message = format!("{error:#}");
                assert!(message.contains("环境变量"));
                assert!(
                    !message.contains("synthetic-secret")
                        && !message.contains("SYNTHETIC_VARIABLE")
                );
                assert_eq!(fs::read(path).unwrap(), before);
            });
        }
    }

    #[test]
    fn inactive_environment_mapping_keeps_mapping_and_allows_yaml_fallback_edits() {
        let yaml = PRIMARY_YAML.replace(
            "env_overrides: {}",
            "env_overrides: {username: SYNTHETIC_VARIABLE}",
        );
        for value in [None, Some(String::new())] {
            with_primary_yaml(&yaml, |path| {
                PrimaryPatch {
                    username: Some("synthetic-new-user".into()),
                    ..PrimaryPatch::default()
                }
                .save_to_with_env(path, |_| value.clone())
                .unwrap();
                let raw: serde_json::Value =
                    serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
                assert_eq!(raw["account"]["username"], "synthetic-new-user");
                assert_eq!(raw["env_overrides"]["username"], "SYNTHETIC_VARIABLE");
            });
        }
    }

    #[test]
    fn invalid_primary_patch_or_yaml_never_replaces_original_or_echoes_credentials() {
        for patch in [
            PrimaryPatch {
                entry_url: Some(
                    "https://synthetic-secret-user:synthetic-secret-password@portal.test/".into(),
                ),
                ..PrimaryPatch::default()
            },
            PrimaryPatch {
                password: Some(String::new()),
                ..PrimaryPatch::default()
            },
            PrimaryPatch {
                ssid: Some("synthetic-network-name-beyond-thirty-two-bytes".into()),
                ..PrimaryPatch::default()
            },
        ] {
            with_primary_yaml(PRIMARY_YAML, |path| {
                let original = fs::read(path).unwrap();
                let error = patch.save_to_with_env(path, |_| None).unwrap_err();
                assert!(!format!("{error:#}").contains("synthetic-secret"));
                assert_eq!(fs::read(path).unwrap(), original);
                assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
            });
        }
        for invalid in [
            "account: [synthetic-secret-input",
            "[synthetic-secret-input]",
            "synthetic-secret-input",
        ] {
            with_primary_yaml(invalid, |path| {
                let error = PrimaryPatch {
                    ssid: Some("Synthetic_WiFi".into()),
                    ..PrimaryPatch::default()
                }
                .save_to_with_env(path, |_| None)
                .unwrap_err();
                assert!(!format!("{error:#}").contains("synthetic-secret"));
                assert_eq!(fs::read_to_string(path).unwrap(), invalid);
            });
        }
    }

    #[test]
    fn non_string_unknown_yaml_keys_are_rejected_without_rewriting_the_file() {
        for key in ["7", "true", "false"] {
            let yaml = format!("{PRIMARY_YAML}\nextra_keys:\n  {key}: synthetic-secret-value\n");
            with_primary_yaml(&yaml, |path| {
                let error = PrimaryPatch {
                    ssid: Some("Another_Synthetic_WiFi".into()),
                    ..PrimaryPatch::default()
                }
                .save_to_with_env(path, |_| None)
                .unwrap_err();
                assert!(!format!("{error:#}").contains("synthetic-secret-value"));
                assert_eq!(fs::read_to_string(path).unwrap(), yaml);
                assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
            });
        }
        let yaml = format!("{PRIMARY_YAML}\nextra_keys:\n  \"7\": 23\n  \"true\": false\n");
        with_primary_yaml(&yaml, |path| {
            PrimaryPatch {
                ssid: Some("Another_Synthetic_WiFi".into()),
                ..PrimaryPatch::default()
            }
            .save_to_with_env(path, |_| None)
            .unwrap();
            let document: serde_json::Value =
                serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(document["extra_keys"]["7"], 23);
            assert_eq!(document["extra_keys"]["true"], false);
        });
    }

    #[test]
    fn yaml_roundtrip_keeps_original_typed_string_semantics_outside_the_patch() {
        let yaml = PRIMARY_YAML
            .replace("service: cmcc", "service: cmcc\n  user_agent: 000123")
            .replace("username: synthetic-user", "username: 001234")
            .replace("password: synthetic-stored-password", "password: 000567")
            .replace("ssid: Synthetic_Campus_WiFi", "ssid: 001234\n  profile_xml: 000456\n  fallback:\n    ssid: 000789\n    password: ON")
            .replace("level: WARNING", "level: WARNING\n  file: 000321")
            .replace("env_overrides: {}", "env_overrides: {username: 000345}");
        with_primary_yaml(&yaml, |path| {
            let before: AppConfig = serde_saphyr::from_str(&yaml).unwrap();
            assert_eq!(before.wifi.ssid, "001234");
            assert_eq!(before.wifi.fallback.password, "ON");
            assert_eq!(before.portal.user_agent, "000123");
            let outcome = PrimaryPatch {
                entry_url: Some("https://portal.test/changed".into()),
                ..PrimaryPatch::default()
            }
            .save_to_with_env(path, |_| None)
            .unwrap();
            assert!(outcome.warning.is_none());
            let after: AppConfig =
                serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            let strings = |config: &AppConfig| {
                [
                    config.account.username.clone(),
                    config.account.password.clone(),
                    config.wifi.ssid.clone(),
                    config.wifi.profile_xml.clone(),
                    config.wifi.fallback.ssid.clone(),
                    config.wifi.fallback.password.clone(),
                    config.wifi.fallback.encryption.clone(),
                    config.wifi.fallback.auth.clone(),
                    config.portal.user_agent.clone(),
                    config.portal.api_base.clone(),
                    config.portal.probe_url.clone(),
                    config.portal.service.clone(),
                    config.logging.file.clone(),
                    config.logging.level.clone(),
                ]
            };
            assert_eq!(strings(&before), strings(&after));
            assert_eq!(before.env_overrides, after.env_overrides);
            assert_eq!(after.portal.entry_url, "https://portal.test/changed");
        });
    }

    #[cfg(windows)]
    #[test]
    fn primary_temp_has_source_dacl_before_content_and_replace_keeps_it() {
        use std::{mem::size_of, os::windows::ffi::OsStrExt, ptr};
        use windows_sys::Win32::Security::{
            ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetFileSecurityW,
            GetSecurityDescriptorControl, GetSecurityDescriptorDacl, INHERITED_ACE,
            PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED, SetFileSecurityW,
            SetSecurityDescriptorControl,
        };

        fn descriptor(path: &Path) -> Vec<u32> {
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut needed = 0;
            // 测试只读取本测试创建的合成文件，绝不查询用户配置权限。
            assert_eq!(
                unsafe {
                    GetFileSecurityW(
                        path.as_ptr(),
                        DACL_SECURITY_INFORMATION,
                        ptr::null_mut(),
                        0,
                        &mut needed,
                    )
                },
                0
            );
            assert!(needed > 0);
            let mut bytes = vec![0u32; (needed as usize).div_ceil(size_of::<u32>())];
            assert_ne!(
                unsafe {
                    GetFileSecurityW(
                        path.as_ptr(),
                        DACL_SECURITY_INFORMATION,
                        bytes.as_mut_ptr().cast(),
                        needed,
                        &mut needed,
                    )
                },
                0
            );
            bytes
        }

        fn dacl_state(path: &Path) -> (Vec<u8>, bool) {
            let mut descriptor = descriptor(path);
            let security = descriptor.as_mut_ptr().cast();
            let mut present = 0;
            let mut defaulted = 0;
            let mut dacl: *mut ACL = ptr::null_mut();
            assert_ne!(
                unsafe {
                    GetSecurityDescriptorDacl(security, &mut present, &mut dacl, &mut defaulted)
                },
                0
            );
            assert_ne!(present, 0);
            assert!(!dacl.is_null());
            let offset = (dacl as usize)
                .checked_sub(descriptor.as_ptr() as usize)
                .unwrap();
            let capacity = descriptor.len() * size_of::<u32>();
            assert!(offset + size_of::<ACL>() <= capacity);
            // GetFileSecurityW 返回自相对描述符；确认有效 ACL 字节都在所有者缓冲区内。
            let length = usize::from(unsafe { (*dacl).AclSize });
            assert!(length >= size_of::<ACL>() && offset + length <= capacity);
            let mut bytes =
                unsafe { std::slice::from_raw_parts(dacl.cast::<u8>(), length) }.to_vec();
            let ace_count = unsafe { (*dacl).AceCount };
            let mut ace_offset = size_of::<ACL>();
            for _ in 0..ace_count {
                assert!(
                    ace_offset + size_of::<ACE_HEADER>() <= length,
                    "合成文件 ACL 的 ACE 头必须完整"
                );
                let ace_size = usize::from(u16::from_le_bytes([
                    bytes[ace_offset + 2],
                    bytes[ace_offset + 3],
                ]));
                assert!(
                    ace_size >= size_of::<ACE_HEADER>() && ace_offset + ace_size <= length,
                    "合成文件 ACL 的 ACE 必须位于缓冲区内"
                );
                // Windows 将受保护新文件的继承 ACE 转为显式 ACE；仅忽略来源标记，
                // 其余标志、类型、顺序、权限掩码、SID 和 ACL 头仍逐字节比较。
                bytes[ace_offset + 1] &= !(INHERITED_ACE as u8);
                ace_offset += ace_size;
            }
            let mut control = 0;
            let mut revision = 0;
            assert_ne!(
                unsafe { GetSecurityDescriptorControl(security, &mut control, &mut revision) },
                0
            );
            (bytes, control & SE_DACL_PROTECTED != 0)
        }

        with_primary_yaml(PRIMARY_YAML, |path| {
            let mut source_descriptor = descriptor(path);
            let source_security = source_descriptor.as_mut_ptr().cast();
            assert_ne!(
                unsafe {
                    SetSecurityDescriptorControl(
                        source_security,
                        SE_DACL_PROTECTED,
                        SE_DACL_PROTECTED,
                    )
                },
                0
            );
            let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            assert_ne!(
                unsafe {
                    SetFileSecurityW(
                        path_wide.as_ptr(),
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        source_security,
                    )
                },
                0
            );
            let before = dacl_state(path);
            assert!(before.1);
            let temporary = path.with_file_name(".permission-temp.local.yaml");
            let backup = path.with_file_name(".permission-backup.local.yaml");
            let file = primary_windows_file::create(&temporary, path).unwrap();
            assert_eq!(file.metadata().unwrap().len(), 0);
            drop(file);
            assert!(
                dacl_state(&temporary) == before,
                "空临时文件必须已持有与源文件相同的有效 DACL 和保护状态"
            );
            // 仅在确认空文件创建时已持有对应 DACL 后写入合成内容。
            let replacement =
                PRIMARY_YAML.replace("Synthetic_Campus_WiFi", "Another_Synthetic_WiFi");
            let mut file = fs::OpenOptions::new().write(true).open(&temporary).unwrap();
            file.write_all(replacement.as_bytes()).unwrap();
            file.sync_all().unwrap();
            drop(file);
            let outcome =
                primary_windows_file::replace(path, &temporary, &backup, PRIMARY_YAML).unwrap();
            assert!(outcome.warning.is_none());
            assert_eq!(fs::read_to_string(path).unwrap(), replacement);
            assert!(
                dacl_state(path) == before,
                "替换后的配置必须保留源文件的有效 DACL 和保护状态"
            );
            assert!(!temporary.exists() && !backup.exists());
            assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        });
    }
    #[test]
    fn all_env_overrides_reach_correct_sections() {
        let mut cfg = AppConfig {
            env_overrides: [
                ("username".into(), "U".into()),
                ("password".into(), "P".into()),
                ("service".into(), "S".into()),
                ("log_level".into(), "L".into()),
            ]
            .into(),
            ..Default::default()
        };
        cfg.apply_env(|key| {
            Some(
                match key {
                    "U" => "2024000000",
                    "P" => "synthetic password ",
                    "S" => "ctcc",
                    _ => "DEBUG",
                }
                .into(),
            )
        });
        assert_eq!(cfg.account.username, "2024000000");
        assert_eq!(cfg.account.password, "synthetic password ");
        assert_eq!(cfg.portal.service, "ctcc");
        assert_eq!(cfg.logging.level, "DEBUG");
    }
    #[test]
    fn numeric_username_and_password_are_compatible() {
        let a: AccountConfig =
            serde_saphyr::from_str("username: 2024000000\npassword: 12345678").unwrap();
        assert_eq!(a.username, "2024000000");
        assert_eq!(a.password, "12345678");
    }
    #[test]
    fn overrides_preserve_false_zero_and_whitelist() {
        let mut cfg = AppConfig::default();
        cfg.wifi.fallback.enabled = true;
        cfg.wifi.fallback.ssid = "Fallback_WiFi".into();
        cfg.wifi.fallback.switch_back_interval = 300;
        let ov = GuiOverrides {
            fallback_enabled: Some(false),
            switch_back_interval: Some(0),
            appearance: Some(Appearance::default()),
            ..Default::default()
        };
        let eff = cfg.effective(&ov);
        assert!(!eff.wifi.fallback.enabled);
        assert_eq!(eff.wifi.fallback.switch_back_interval, 0);
        let json = serde_json::to_string(&ov).unwrap();
        assert!(!json.contains("username"));
        assert!(!json.contains("portal"));
        assert!(json.contains("appearance"));
    }
    #[test]
    fn malformed_config_is_rejected_without_echoing_credentials() {
        let mut cfg = AppConfig::default();
        cfg.portal.entry_url = "https://portal.test/".into();
        cfg.portal.api_base = "https://portal.test/".into();
        cfg.account.username = "2024000000".into();
        cfg.account.password = "synthetic-secret".into();
        cfg.daemon.probe_interval = 0;
        assert!(
            !cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("synthetic-secret")
        );
    }
    #[test]
    fn malformed_override_field_does_not_discard_valid_fields() {
        let dir = std::env::temp_dir().join(format!("cng-config-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path,br#"{"probe_interval":"wrong","fallback_enabled":false,"switch_back_interval":0,"username":"ignored"}"#).unwrap();
        let ov = GuiOverrides::load_from(&path);
        assert!(ov.probe_interval.is_none());
        assert_eq!(ov.fallback_enabled, Some(false));
        assert_eq!(ov.switch_back_interval, Some(0));
        assert!(ov.appearance.is_none());
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn display_scale_presets_roundtrip_and_old_settings_default_to_full_size() {
        let dir =
            std::env::temp_dir().join(format!("cng-display-scale-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path, br#"{"probe_interval":60}"#).unwrap();
        let old = GuiOverrides::load_from(&path);
        assert_eq!(old.display_scale, None);
        assert_eq!(old.display_scale_percent(), 100);
        assert_eq!(old.probe_interval, Some(60));
        for &scale in DISPLAY_SCALE_PRESETS {
            let settings = GuiOverrides {
                display_scale: Some(scale),
                probe_interval: Some(60),
                appearance: Some(Appearance::default()),
                ..Default::default()
            };
            settings.save_to(&path).unwrap();
            let loaded = GuiOverrides::load_from(&path);
            assert_eq!(loaded.display_scale, Some(scale));
            assert_eq!(loaded.display_scale_percent(), scale);
            assert_eq!(loaded.probe_interval, Some(60));
            assert_eq!(loaded.appearance, Some(Appearance::default()));
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn malformed_display_scale_is_ignored_without_losing_other_preferences() {
        let dir =
            std::env::temp_dir().join(format!("cng-invalid-scale-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        for scale in [
            serde_json::json!(null),
            serde_json::json!("125"),
            serde_json::json!(125.0),
            serde_json::json!(true),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!(-100),
            serde_json::json!(0),
            serde_json::json!(99),
            serde_json::json!(201),
            serde_json::json!(u64::MAX),
        ] {
            let raw = serde_json::json!({
                "display_scale": scale,
                "probe_interval": 60,
                "fallback_enabled": false,
                "appearance": {"style": "chat_gpt"}
            });
            fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
            let loaded = GuiOverrides::load_from(&path);
            assert_eq!(loaded.display_scale, None);
            assert_eq!(loaded.display_scale_percent(), 100);
            assert_eq!(loaded.probe_interval, Some(60));
            assert_eq!(loaded.fallback_enabled, Some(false));
            assert_eq!(loaded.appearance.unwrap().style, ThemeStyle::ChatGpt);
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_display_scale_save_preserves_the_previous_file() {
        let dir = std::env::temp_dir().join(format!("cng-scale-save-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let previous = br#"{"display_scale":125,"probe_interval":60}"#;
        fs::write(&path, previous).unwrap();
        for scale in [0, 49, 51, 99, 101, 126, 201, u16::MAX] {
            let settings = GuiOverrides {
                display_scale: Some(scale),
                ..Default::default()
            };
            assert_eq!(settings.display_scale_percent(), 100);
            assert!(settings.save_to(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), previous);
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn appearance_defaults_and_enum_json_remain_stable() {
        let appearance: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(appearance, Appearance::default());
        assert_eq!(appearance.style, ThemeStyle::Claude);
        assert_eq!(appearance.mode, ThemeMode::System);
        for (style, expected) in [
            (ThemeStyle::Claude, "\"claude\""),
            (ThemeStyle::ChatGpt, "\"chat_gpt\""),
        ] {
            assert_eq!(serde_json::to_string(&style).unwrap(), expected);
            assert_eq!(serde_json::from_str::<ThemeStyle>(expected).unwrap(), style);
        }
        for (mode, expected) in [
            (ThemeMode::System, "\"system\""),
            (ThemeMode::Light, "\"light\""),
            (ThemeMode::Dark, "\"dark\""),
        ] {
            assert_eq!(serde_json::to_string(&mode).unwrap(), expected);
            assert_eq!(serde_json::from_str::<ThemeMode>(expected).unwrap(), mode);
        }
    }

    #[test]
    fn appearance_roundtrip_preserves_all_preset_palettes_and_modes() {
        let dir = std::env::temp_dir().join(format!("cng-appearance-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gui_settings.json");
        let mut appearance = Appearance::default();
        for (style, dark, colors) in [
            (
                ThemeStyle::Claude,
                false,
                ["#037EFE", "#FAF9F5", "#FFFFFF", "#2F2C26"],
            ),
            (
                ThemeStyle::Claude,
                true,
                ["#5399FE", "#1D1C19", "#262522", "#E8E5DD"],
            ),
            (
                ThemeStyle::ChatGpt,
                false,
                ["#02846F", "#FFFFFF", "#F7F7F8", "#202123"],
            ),
            (
                ThemeStyle::ChatGpt,
                true,
                ["#10A37F", "#212121", "#2F2F2F", "#ECECEC"],
            ),
        ] {
            appearance.style = style;
            *appearance.colors_mut(dark) = ColorOverrides {
                accent_color: Some(colors[0].into()),
                background_color: Some(colors[1].into()),
                surface_color: Some(colors[2].into()),
                text_color: Some(colors[3].into()),
            };
        }
        for &style in ThemeStyle::ALL {
            appearance.style = style;
            for &mode in ThemeMode::ALL {
                appearance.mode = mode;
                let overrides = GuiOverrides {
                    probe_interval: Some(60),
                    appearance: Some(appearance.clone()),
                    ..Default::default()
                };
                overrides.save_to(&path).unwrap();
                let loaded = GuiOverrides::load_from(&path);
                assert_eq!(loaded.appearance, Some(appearance.clone()));
                assert_eq!(loaded.probe_interval, Some(60));
            }
        }
        appearance.style = ThemeStyle::Claude;
        assert_eq!(
            appearance.colors(false).accent_color.as_deref(),
            Some("#037EFE")
        );
        assert_eq!(
            appearance.colors(true).accent_color.as_deref(),
            Some("#5399FE")
        );
        appearance.style = ThemeStyle::ChatGpt;
        assert_eq!(
            appearance.colors(false).accent_color.as_deref(),
            Some("#02846F")
        );
        assert_eq!(
            appearance.colors(true).accent_color.as_deref(),
            Some("#10A37F")
        );
        *appearance.colors_mut(true) = ColorOverrides::default();
        assert_eq!(appearance.chatgpt.dark, ColorOverrides::default());
        assert!(appearance.chatgpt.light.accent_color.is_some());
        assert!(appearance.claude.dark.accent_color.is_some());
        assert!(appearance.claude.light.accent_color.is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_appearance_does_not_discard_other_valid_overrides() {
        let dir =
            std::env::temp_dir().join(format!("cng-invalid-appearance-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        for appearance in [
            serde_json::json!({"style": "unknown"}),
            serde_json::json!({"mode": 1}),
            serde_json::json!({"claude": {"light": {"accent_color": "#123"}}}),
            serde_json::json!({"chatgpt": {"dark": {"text_color": 123}}}),
            serde_json::json!({"username": "synthetic-user"}),
            serde_json::json!({"claude": {"portal": "https://portal.test"}}),
            serde_json::json!({"chatgpt": {"dark": {"password": "synthetic-secret"}}}),
            serde_json::json!({"claude": []}),
            serde_json::json!({"chatgpt": null}),
            serde_json::json!({"claude": {"light": []}}),
            serde_json::json!({"claude": {"dark": null}}),
            serde_json::json!({"chatgpt": {"light": null}}),
            serde_json::json!({"chatgpt": {"dark": []}}),
            serde_json::Value::Null,
            serde_json::json!([]),
        ] {
            let raw = serde_json::json!({
                "appearance": appearance,
                "probe_interval": 60,
                "fallback_enabled": false,
                "switch_back_interval": 0,
                "username": "ignored"
            });
            fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
            let loaded = GuiOverrides::load_from(&path);
            assert!(loaded.appearance.is_none());
            assert_eq!(loaded.probe_interval, Some(60));
            assert_eq!(loaded.fallback_enabled, Some(false));
            assert_eq!(loaded.switch_back_interval, Some(0));
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn every_color_field_rejects_non_ascii_or_non_six_digit_hex() {
        for field in [
            "accent_color",
            "background_color",
            "surface_color",
            "text_color",
        ] {
            for invalid in [
                "",
                "#fff",
                "123456",
                "#1234567",
                "#12xz34",
                " #123456",
                "#123456\n",
                "#１２３",
            ] {
                let mut raw = serde_json::json!({});
                raw[field] = serde_json::Value::String(invalid.into());
                let palette: ColorOverrides = serde_json::from_value(raw).unwrap();
                assert!(palette.validate().is_err());
            }
            for valid in ["#037efe", "#ABCDEF", "#123456", "#000000"] {
                let mut raw = serde_json::json!({});
                raw[field] = serde_json::Value::String(valid.into());
                let palette: ColorOverrides = serde_json::from_value(raw).unwrap();
                palette.validate().unwrap();
            }
        }
    }

    #[test]
    fn invalid_inactive_palette_cannot_replace_saved_settings() {
        let dir = std::env::temp_dir().join(format!("cng-appearance-save-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gui_settings.json");
        let mut overrides = GuiOverrides {
            probe_interval: Some(30),
            appearance: Some(Appearance::default()),
            ..Default::default()
        };
        overrides.save_to(&path).unwrap();
        let original = fs::read(&path).unwrap();
        overrides
            .appearance
            .as_mut()
            .unwrap()
            .chatgpt
            .dark
            .text_color = Some("#bad-color".into());
        let error = overrides.save_to(&path).unwrap_err();
        assert!(error.to_string().contains("ChatGPT 深色"));
        assert!(!format!("{error:#}").contains("#bad-color"));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unspecified_source_cannot_silently_disable_binding() {
        assert!(!valid_source_ipv4("0.0.0.0"));
        assert!(!valid_source_ipv4("255.255.255.255"));
        assert!(!valid_source_ipv4("224.0.0.1"));
        assert!(valid_source_ipv4("10.20.30.40"));
    }
    #[test]
    fn shipped_template_deserializes_legacy_schema() {
        let cfg: AppConfig =
            serde_saphyr::from_str(include_str!("../../config.example.yaml")).unwrap();
        assert_eq!(cfg.portal.timeout, 10);
        assert_eq!(cfg.daemon.probe_interval, 30);
        assert_eq!(cfg.wifi.fallback.connect_timeout, 20);
        assert!(cfg.validate().is_err());
    }
    #[test]
    fn settings_replace_existing_json_atomically_without_yaml_fields() {
        let dir =
            std::env::temp_dir().join(format!("cng-atomic-config-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gui_settings.json");
        let mut ov = GuiOverrides {
            probe_interval: Some(30),
            fallback_password: Some("synthetic-password".into()),
            ..Default::default()
        };
        ov.save_to(&path).unwrap();
        ov.probe_interval = Some(60);
        ov.save_to(&path).unwrap();
        assert_eq!(GuiOverrides::load_from(&path).probe_interval, Some(60));
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(value.get("account").is_none());
        assert!(value.get("portal").is_none());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn running_settings_defaults_roundtrip_and_preserve_other_yaml_fields() {
        let legacy = PRIMARY_YAML;
        let old: AppConfig = serde_saphyr::from_str(legacy).unwrap();
        assert!(old.running == RunningSettings::default());
        let yaml = format!(
            "{legacy}\nrunning:\n  phone: 12345678901\n  password: ' synthetic pass '\n  default_campus: campus_1\n  catalog_file: tracks.yaml\n"
        );
        let loaded: AppConfig = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(loaded.running.phone, "12345678901");
        assert_eq!(loaded.running.password, " synthetic pass ");
        loaded.running.validate().unwrap();
        with_primary_yaml(legacy, |path| {
            RunningPatch {
                phone: Some("12345678901".into()),
                password: Some(" synthetic pass ".into()),
                default_campus: Some("campus_1".into()),
                catalog_file: Some("tracks.yaml".into()),
            }
            .save_to(path)
            .unwrap();
            let saved: AppConfig =
                serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert!(saved.running == loaded.running);
            assert_eq!(saved.wifi.ssid, "Synthetic_Campus_WiFi");
            assert_eq!(saved.account.username, "synthetic-user");
            let actual: serde_json::Value =
                serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            let expected: serde_json::Value = serde_saphyr::from_str(legacy).unwrap();
            assert_eq!(actual["unknown"], expected["unknown"]);
        });
        let json = serde_json::to_value(GuiOverrides::default()).unwrap();
        assert!(json.get("running").is_none());
        assert!(json.get("phone").is_none());
        assert!(json.get("password").is_none());
    }

    #[test]
    fn running_invalid_patch_does_not_modify_yaml() {
        for patch in [
            RunningPatch {
                phone: Some("123".into()),
                ..Default::default()
            },
            RunningPatch {
                default_campus: Some("bad campus".into()),
                ..Default::default()
            },
            RunningPatch {
                password: Some("bad\0password".into()),
                ..Default::default()
            },
        ] {
            with_primary_yaml(PRIMARY_YAML, |path| {
                let before = fs::read(path).unwrap();
                assert!(patch.save_to(path).is_err());
                assert_eq!(fs::read(path).unwrap(), before);
            });
        }
    }

    #[test]
    fn primary_patch_preserves_existing_running_fields() {
        let yaml = format!(
            "{PRIMARY_YAML}\nrunning:\n  phone: 12345678901\n  password: ' synthetic pass '\n  default_campus: campus_1\n  catalog_file: tracks.yaml\n"
        );
        with_primary_yaml(&yaml, |path| {
            PrimaryPatch {
                ssid: Some("Updated_Synthetic_WiFi".into()),
                ..Default::default()
            }
            .save_to_with_env(path, |_| None)
            .unwrap();
            let saved: AppConfig =
                serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(saved.running.phone, "12345678901");
            assert_eq!(saved.running.password, " synthetic pass ");
            assert_eq!(saved.running.default_campus, "campus_1");
            assert_eq!(saved.running.catalog_file, "tracks.yaml");
        });
    }
}
