#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Result, bail};
use campus_guardian::{
    auth::AuthClient,
    config::{self, AppConfig},
    platform,
};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Gui,
    Once,
    Probe,
    Check,
    Autostart(bool),
    AutostartStatus,
    Version,
    Help,
}
#[derive(Debug, Default)]
struct Options {
    mode: Mode,
    config: Option<PathBuf>,
    benchmark: bool,
    hidden: bool,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Options> {
    let mut result = Options::default();
    let mut mode_set = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mode = match arg.as_str() {
            "--config" | "-c" => {
                let value = args
                    .next()
                    .filter(|v| !v.starts_with('-'))
                    .ok_or_else(|| anyhow::anyhow!("--config 后需要文件路径"))?;
                if result.config.replace(value.into()).is_some() {
                    bail!("配置路径只能指定一次");
                }
                continue;
            }
            "--gui" => Mode::Gui,
            "--hidden" => {
                result.hidden = true;
                continue;
            }
            "--benchmark-ui" => {
                result.benchmark = true;
                Mode::Gui
            }
            "--benchmark-tray" => {
                result.benchmark = true;
                result.hidden = true;
                Mode::Gui
            }
            "--once" => Mode::Once,
            "--probe" => Mode::Probe,
            "--check" => Mode::Check,
            "--enable-autostart" => Mode::Autostart(true),
            "--disable-autostart" => Mode::Autostart(false),
            "--autostart-status" => Mode::AutostartStatus,
            "--version" | "-V" => Mode::Version,
            "--help" | "-h" => Mode::Help,
            _ => bail!("不支持此启动参数，请使用 --help 查看 Rust 版可用命令"),
        };
        if mode_set {
            bail!("一次只能指定一种运行模式");
        }
        mode_set = true;
        result.mode = mode;
    }
    if result.mode != Mode::Gui && (result.hidden || result.benchmark) {
        bail!("隐藏或性能测量参数只适用于桌面模式");
    }
    if result.benchmark && result.config.is_some() {
        bail!("性能测量模式不接受真实配置文件");
    }
    Ok(result)
}
fn main() {
    if let Err(error) = run() {
        #[cfg(windows)]
        campus_guardian::ui::show_error(&format!("{error}"));
        #[cfg(not(windows))]
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let options = parse(std::env::args().skip(1))?;
    if options.mode == Mode::Version {
        println!("campus-guardian {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if options.mode == Mode::Help {
        println!(
            "校园网守护\n--gui [--hidden]  桌面守护\n--config <路径>  指定配置\n--once          登录一次\n--probe         探测连接\n--check         校验配置\n--enable-autostart / --disable-autostart / --autostart-status\n--version"
        );
        return Ok(());
    }
    if options.mode == Mode::AutostartStatus {
        println!("{}", platform::autostart_enabled());
        return Ok(());
    }
    let path = options.config.unwrap_or_else(|| {
        let local = PathBuf::from("config.yaml");
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("config.yaml")));
        if local.is_file() {
            local
        } else if let Some(sibling) = sibling.filter(|p| p.is_file()) {
            sibling
        } else {
            config::data_dir().join("config.yaml")
        }
    });
    if let Mode::Autostart(enabled) = options.mode {
        return platform::set_autostart_with_config(enabled, &path);
    }
    if options.mode == Mode::Gui {
        #[cfg(windows)]
        return campus_guardian::ui::run(path, options.benchmark, options.hidden);
        #[cfg(not(windows))]
        bail!("桌面应用仅支持 Windows");
    }
    let cfg = AppConfig::load(&path)?;
    if options.mode == Mode::Check {
        println!("配置校验通过");
        return Ok(());
    }
    if options.mode == Mode::Once && !cfg.wifi.ssid.is_empty() {
        if !cfg.wifi.profile_xml.is_empty() {
            platform::import_profile(std::path::Path::new(&cfg.wifi.profile_xml))?;
        }
        if !platform::connect_wifi(
            &cfg.wifi.ssid,
            Duration::from_secs(cfg.wifi.connect_timeout),
        )? {
            bail!("未能连接校园网 WiFi");
        }
    }
    let mut auth = AuthClient::new(&cfg)?;
    if options.mode == Mode::Probe {
        if !auth.probe(None)? {
            bail!("外网不可达");
        }
        println!("外网连接正常");
    } else {
        let result = auth.login()?;
        if !result.success || !result.online {
            bail!("认证未成功");
        }
        println!("认证成功");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> impl Iterator<Item = String> {
        values.iter().map(|s| s.to_string())
    }
    #[test]
    fn rejects_unknown_and_legacy_before_side_effects() {
        assert!(parse(args(&["--uninstall", "--yes"])).is_err());
        assert!(parse(args(&["--typo"])).is_err());
    }
    #[test]
    fn rejects_missing_config_and_mode_conflict() {
        assert!(parse(args(&["--config", "--hidden"])).is_err());
        assert!(parse(args(&["--once", "--probe"])).is_err());
    }
    #[test]
    fn benchmark_never_accepts_local_config() {
        assert!(parse(args(&["--benchmark-ui", "--config", "config.yaml"])).is_err());
    }
    #[test]
    fn supports_hidden_startup() {
        let result = parse(args(&["--gui", "--hidden"])).unwrap();
        assert!(result.hidden);
        assert_eq!(result.mode, Mode::Gui);
    }
}
