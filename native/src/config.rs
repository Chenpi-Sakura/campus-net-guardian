//! 兼容既有 YAML 的配置、环境变量覆盖与 GUI 白名单覆盖层。

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
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
        let contents = fs::read_to_string(path).context("无法读取配置文件")?;
        let mut config: Self = serde_saphyr::from_str(&contents)
            .map_err(|_| anyhow::anyhow!("配置格式无效，请检查 YAML 字段与类型"))?;
        config.apply_env(|key| std::env::var(key).ok());
        config.portal.service = config.portal.service.trim().to_lowercase();
        config.logging.level = config.logging.level.trim().to_uppercase();
        config.portal.entry_url = config.portal.entry_url.trim().into();
        config.portal.api_base = config.portal.api_base.trim().into();
        config.portal.probe_url = config.portal.probe_url.trim().into();
        config.account.username = config.account.username.trim().into();
        config.validate()?;
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
            bail!("请先在配置文件填写门户地址与账号密码");
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
            bail!("请先在配置文件填写校园网 SSID，使用有线网络时可留空");
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
}
impl GuiOverrides {
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
        }
    }

    /// 只保存 GUI 白名单键，不保存账号、门户或主网 SSID。
    pub fn save(&self) -> Result<()> {
        self.save_to(&data_dir().join("gui_settings.json"))
    }
    /// 原子保存到指定路径，失败时清理本次临时文件。
    pub fn save_to(&self, path: &Path) -> Result<()> {
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
            ..Default::default()
        };
        let eff = cfg.effective(&ov);
        assert!(!eff.wifi.fallback.enabled);
        assert_eq!(eff.wifi.fallback.switch_back_interval, 0);
        let json = serde_json::to_string(&ov).unwrap();
        assert!(!json.contains("username"));
        assert!(!json.contains("portal"));
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
        let _ = fs::remove_dir_all(&dir);
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
}
