//! 普通用户权限的 Windows WLAN、自启快捷方式与文件打开操作。

use crate::config::FallbackWifiConfig;
use anyhow::{Context, Result, bail};
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// 返回当前无线网络名；未连接时为空，不启动 netsh 或 PowerShell。
pub fn current_ssid() -> Result<String> {
    #[cfg(windows)]
    {
        native::current_connection().map(|v| v.map(|c| c.1).unwrap_or_default())
    }
    #[cfg(not(windows))]
    {
        Ok(String::new())
    }
}

/// 按当前 WLAN 接口查询 IPv4，每次认证重新解析以应对 DHCP 和切网。
pub fn wlan_ipv4() -> Option<Ipv4Addr> {
    if let Ok(value) = std::env::var("CG_BIND_INTERFACE_IP")
        && !value.trim().is_empty()
        && value.trim() != "auto"
    {
        return value
            .trim()
            .parse::<Ipv4Addr>()
            .ok()
            .filter(|ip| !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast());
    }
    #[cfg(windows)]
    {
        native::wlan_ipv4()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 扫描结果只用于用户选择备用网络，不记录到日志。
pub fn visible_networks() -> Result<Vec<String>> {
    #[cfg(windows)]
    {
        native::visible_networks()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// 连接系统已保存的 WiFi profile，等待实际关联成功后才返回。
pub fn connect_wifi(ssid: &str, timeout: Duration) -> Result<bool> {
    connect_wifi_inner(ssid, timeout, None)
}
/// 带停止信号的连接操作，后台退出时在下一次轮询立即结束。
pub(crate) fn connect_wifi_cancel(
    ssid: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<bool> {
    connect_wifi_inner(ssid, timeout, Some(cancel))
}
fn connect_wifi_inner(ssid: &str, timeout: Duration, cancel: Option<&AtomicBool>) -> Result<bool> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Ok(false);
    }
    if ssid.is_empty() {
        return Ok(true);
    }
    if current_ssid()? == ssid {
        return Ok(true);
    }
    #[cfg(windows)]
    native::request_connection(ssid)?;
    #[cfg(not(windows))]
    bail!("WiFi 管理仅支持 Windows");
    #[cfg(windows)]
    {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Ok(false);
            }
            if current_ssid()? == ssid {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(false)
    }
}

/// 导入用户显式配置的 WLAN XML，凭据不进入日志或命令行。
pub fn import_profile(path: &Path) -> Result<()> {
    let xml = std::fs::read_to_string(path).context("无法读取 WiFi profile XML")?;
    #[cfg(windows)]
    {
        native::set_profile(&xml)
    }
    #[cfg(not(windows))]
    {
        let _ = xml;
        bail!("WiFi 管理仅支持 Windows")
    }
}

/// 按备用配置建立 profile 并连接；无密码时复用 Windows 中已有 profile。
pub fn ensure_fallback(cfg: &FallbackWifiConfig) -> Result<bool> {
    ensure_fallback_inner(cfg, None)
}
/// 后台连接备用网络时响应停止信号。
pub(crate) fn ensure_fallback_cancel(
    cfg: &FallbackWifiConfig,
    cancel: &AtomicBool,
) -> Result<bool> {
    ensure_fallback_inner(cfg, Some(cancel))
}
fn ensure_fallback_inner(cfg: &FallbackWifiConfig, cancel: Option<&AtomicBool>) -> Result<bool> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Ok(false);
    }
    if !cfg.enabled || cfg.ssid.trim().is_empty() {
        return Ok(false);
    }
    if current_ssid()? == cfg.ssid {
        return Ok(true);
    }
    if !cfg.password.is_empty() || cfg.encryption == "open" {
        let xml = profile_xml(cfg)?;
        #[cfg(windows)]
        native::set_profile(&xml)?;
        #[cfg(not(windows))]
        {
            let _ = xml;
        }
    }
    // 每轮守护只做一次连接；失败轮数由状态机计数，避免 retry × retry 的隐式乘法。
    connect_wifi_inner(&cfg.ssid, Duration::from_secs(cfg.connect_timeout), cancel)
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn profile_xml(cfg: &FallbackWifiConfig) -> Result<String> {
    if cfg.ssid.is_empty() || cfg.ssid.len() > 32 {
        bail!("SSID 长度无效");
    }
    if !matches!(
        cfg.encryption.as_str(),
        "open" | "WPAPSK" | "WPA2PSK" | "WPA3SAE"
    ) || !matches!(cfg.auth.as_str(), "open" | "TKIP" | "AES")
    {
        bail!("WiFi 安全配置无效");
    }
    let open = cfg.encryption == "open";
    let authentication = if open {
        "open"
    } else {
        cfg.encryption.as_str()
    };
    let encryption = if open { "none" } else { cfg.auth.as_str() };
    let key = if open {
        String::new()
    } else {
        format!(
            "<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
            escape_xml(&cfg.password)
        )
    };
    let name = escape_xml(&cfg.ssid);
    let hex: String = cfg
        .ssid
        .as_bytes()
        .iter()
        .map(|v| format!("{v:02X}"))
        .collect();
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{name}</name><SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security><authEncryption><authentication>{authentication}</authentication><encryption>{encryption}</encryption><useOneX>false</useOneX></authEncryption>{key}</security></MSM></WLANProfile>"
    ))
}

fn startup_link() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::data_dir().join("Roaming"))
        .join("Microsoft/Windows/Start Menu/Programs/Startup/Campus Net Guardian.lnk")
}

/// 查询当前用户启动目录快捷方式是否存在。
pub fn autostart_enabled() -> bool {
    startup_link().is_file()
}

/// 只写用户启动目录，不注册服务、不触发 UAC。
pub fn set_autostart(enabled: bool) -> Result<()> {
    let local = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("config.yaml");
    let config = if local.is_file() {
        local
    } else {
        crate::config::data_dir().join("config.yaml")
    };
    set_autostart_with_config(enabled, &config)
}

/// 为当前明确选择的配置文件建立隐藏启动快捷方式，避免自启读取另一份配置。
pub fn set_autostart_with_config(enabled: bool, config: &Path) -> Result<()> {
    let link = startup_link();
    if !enabled {
        match std::fs::remove_file(&link) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("无法关闭登录时自启"),
        };
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let exe = std::env::current_exe().context("无法定位应用文件")?;
        std::fs::create_dir_all(link.parent().context("启动目录无效")?)
            .context("无法创建用户启动目录")?;
        let quote = |p: &Path| p.to_string_lossy().replace('\'', "''");
        let config = std::path::absolute(config).context("配置文件路径无效")?;
        let arguments = format!(
            "--gui --hidden --config {}",
            quote_windows_arg(&config.to_string_lossy())
        );
        let script = format!(
            "$ErrorActionPreference='Stop';$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='{}';$s.WorkingDirectory='{}';$s.Description='Campus Net Guardian';$s.Save()",
            quote(&link),
            quote(&exe),
            arguments.replace('\'', "''"),
            quote(exe.parent().unwrap_or(Path::new(".")))
        );
        let result = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x08000000)
            .output()
            .context("无法创建自启快捷方式")?;
        if !result.status.success() {
            bail!("创建自启快捷方式失败");
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = config;
        bail!("登录时自启仅支持 Windows")
    }
}

fn quote_windows_arg(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
            out.push('"');
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
            out.push(ch);
        }
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// 通过 Windows 关联程序打开配置或日志，不通过命令解释器拼接路径。
pub fn open_file(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: *mut std::ffi::c_void,
                op: *const u16,
                file: *const u16,
                params: *const u16,
                dir: *const u16,
                show: i32,
            ) -> isize;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        } <= 32
        {
            bail!("无法打开文件");
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("此操作仅支持 Windows")
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::c_void, ptr};
    type Handle = *mut c_void;
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub(super) struct Guid {
        a: u32,
        b: u16,
        c: u16,
        d: [u8; 8],
    }
    #[repr(C)]
    struct Interface {
        guid: Guid,
        description: [u16; 256],
        state: u32,
    }
    #[repr(C)]
    struct Ssid {
        len: u32,
        bytes: [u8; 32],
    }
    #[repr(C)]
    struct Association {
        ssid: Ssid,
        bss_type: u32,
        bssid: [u8; 6],
        phy_type: u32,
        phy_index: u32,
        signal: u32,
        rx: u32,
        tx: u32,
    }
    #[repr(C)]
    struct Connection {
        state: u32,
        mode: u32,
        profile: [u16; 256],
        association: Association,
        security: [u32; 4],
    }
    #[repr(C)]
    struct Parameters {
        mode: u32,
        profile: *const u16,
        ssid: *const Ssid,
        bssid_list: *const c_void,
        bss_type: u32,
        flags: u32,
    }
    #[repr(C)]
    struct Network {
        profile: [u16; 256],
        ssid: Ssid,
        bss_type: u32,
        bss_count: u32,
        connectable: i32,
        reason: u32,
        phy_count: u32,
        phy_types: [u32; 8],
        more_phy: i32,
        signal: u32,
        secure: i32,
        auth: u32,
        cipher: u32,
        flags: u32,
        reserved: u32,
    }
    #[repr(C)]
    struct IpString {
        next: *const IpString,
        address: [u8; 16],
        mask: [u8; 16],
        context: u32,
    }
    #[repr(C)]
    struct Adapter {
        next: *const Adapter,
        index_combo: u32,
        name: [u8; 260],
        description: [u8; 132],
        address_len: u32,
        address: [u8; 8],
        index: u32,
        kind: u32,
        dhcp: u32,
        current_ip: *const IpString,
        ip: IpString,
    }
    #[link(name = "wlanapi")]
    unsafe extern "system" {
        fn WlanOpenHandle(
            version: u32,
            reserved: *const c_void,
            negotiated: *mut u32,
            client: *mut Handle,
        ) -> u32;
        fn WlanCloseHandle(client: Handle, reserved: *const c_void) -> u32;
        fn WlanEnumInterfaces(
            client: Handle,
            reserved: *const c_void,
            list: *mut *mut c_void,
        ) -> u32;
        fn WlanQueryInterface(
            client: Handle,
            guid: *const Guid,
            opcode: u32,
            reserved: *const c_void,
            size: *mut u32,
            data: *mut *mut c_void,
            opcode_type: *mut u32,
        ) -> u32;
        fn WlanConnect(
            client: Handle,
            guid: *const Guid,
            parameters: *const Parameters,
            reserved: *const c_void,
        ) -> u32;
        fn WlanSetProfile(
            client: Handle,
            guid: *const Guid,
            flags: u32,
            xml: *const u16,
            security: *const u16,
            overwrite: i32,
            reserved: *const c_void,
            reason: *mut u32,
        ) -> u32;
        fn WlanGetAvailableNetworkList(
            client: Handle,
            guid: *const Guid,
            flags: u32,
            reserved: *const c_void,
            list: *mut *mut c_void,
        ) -> u32;
        fn WlanFreeMemory(memory: *mut c_void);
    }
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetAdaptersInfo(info: *mut Adapter, size: *mut u32) -> u32;
    }
    struct Client(Handle);
    impl Drop for Client {
        fn drop(&mut self) {
            unsafe {
                WlanCloseHandle(self.0, ptr::null());
            }
        }
    }
    struct Memory(*mut c_void);
    impl Drop for Memory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    WlanFreeMemory(self.0);
                }
            }
        }
    }
    fn client() -> Result<Client> {
        let mut out = ptr::null_mut();
        let mut version = 0;
        if unsafe { WlanOpenHandle(2, ptr::null(), &mut version, &mut out) } != 0 {
            bail!("无法访问 Windows 无线网络服务");
        }
        Ok(Client(out))
    }
    fn interfaces(client: &Client) -> Result<Vec<(Guid, u32)>> {
        let mut ptr = ptr::null_mut();
        if unsafe { WlanEnumInterfaces(client.0, ptr::null(), &mut ptr) } != 0 || ptr.is_null() {
            bail!("无法查询无线网卡");
        }
        let block = Memory(ptr);
        let count = unsafe { *(block.0 as *const u32) }.min(64);
        let entries = unsafe { (block.0 as *const u8).add(8) as *const Interface };
        Ok((0..count)
            .map(|i| unsafe {
                let v = &*entries.add(i as usize);
                (v.guid, v.state)
            })
            .collect())
    }
    fn ssid_text(ssid: &Ssid) -> String {
        String::from_utf8_lossy(&ssid.bytes[..(ssid.len as usize).min(32)]).into_owned()
    }
    pub(super) fn current_connection() -> Result<Option<(Guid, String)>> {
        let client = client()?;
        for (guid, state) in interfaces(&client)? {
            if state != 1 {
                continue;
            }
            let mut size = 0;
            let mut data = ptr::null_mut();
            let mut kind = 0;
            let code = unsafe {
                WlanQueryInterface(
                    client.0,
                    &guid,
                    7,
                    ptr::null(),
                    &mut size,
                    &mut data,
                    &mut kind,
                )
            };
            if code == 5 {
                bail!("Windows 阻止读取无线网络，请允许桌面应用访问位置");
            }
            if code == 0 && !data.is_null() {
                let block = Memory(data);
                if size as usize >= std::mem::size_of::<Connection>() {
                    let conn = unsafe { &*(block.0 as *const Connection) };
                    return Ok(Some((guid, ssid_text(&conn.association.ssid))));
                }
            }
        }
        Ok(None)
    }
    pub(super) fn wlan_ipv4() -> Option<Ipv4Addr> {
        let guid = current_connection().ok()??.0;
        let expected = format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            guid.a,
            guid.b,
            guid.c,
            guid.d[0],
            guid.d[1],
            guid.d[2],
            guid.d[3],
            guid.d[4],
            guid.d[5],
            guid.d[6],
            guid.d[7]
        );
        let mut size = 0;
        unsafe {
            GetAdaptersInfo(ptr::null_mut(), &mut size);
        }
        if size == 0 || size > 16 * 1024 * 1024 {
            return None;
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let mut adapter = buffer.as_mut_ptr() as *mut Adapter;
        if unsafe { GetAdaptersInfo(adapter, &mut size) } != 0 {
            return None;
        }
        let start = buffer.as_ptr() as usize;
        let end = start + buffer.len() * std::mem::size_of::<usize>();
        for _ in 0..128 {
            if adapter.is_null() {
                break;
            }
            let address = adapter as usize;
            if address < start
                || address
                    .checked_add(std::mem::size_of::<Adapter>())
                    .is_none_or(|last| last > end)
                || !address.is_multiple_of(std::mem::align_of::<Adapter>())
            {
                return None;
            }
            let v = unsafe { &*adapter };
            let name = String::from_utf8_lossy(
                &v.name[..v.name.iter().position(|v| *v == 0).unwrap_or(260)],
            );
            if name.eq_ignore_ascii_case(&expected) {
                let bytes = &v.ip.address;
                let text = String::from_utf8_lossy(
                    &bytes[..bytes.iter().position(|v| *v == 0).unwrap_or(16)],
                );
                return text
                    .parse::<Ipv4Addr>()
                    .ok()
                    .filter(|ip| !ip.is_unspecified());
            }
            adapter = v.next as *mut Adapter;
        }
        None
    }
    pub(super) fn request_connection(ssid: &str) -> Result<()> {
        let client = client()?;
        let interfaces = interfaces(&client)?;
        let Some((guid, _)) = interfaces
            .iter()
            .find(|(_, state)| *state == 1)
            .or_else(|| interfaces.first())
        else {
            bail!("没有可用的无线网卡");
        };
        let profile: Vec<u16> = ssid.encode_utf16().chain(Some(0)).collect();
        let parameters = Parameters {
            mode: 0,
            profile: profile.as_ptr(),
            ssid: ptr::null(),
            bssid_list: ptr::null(),
            bss_type: 1,
            flags: 0,
        };
        if unsafe { WlanConnect(client.0, guid, &parameters, ptr::null()) } != 0 {
            bail!("无法连接 WiFi，请先在 Windows 中保存该网络");
        }
        Ok(())
    }
    pub(super) fn set_profile(xml: &str) -> Result<()> {
        let client = client()?;
        let interfaces = interfaces(&client)?;
        let Some((guid, _)) = interfaces
            .iter()
            .find(|(_, state)| *state == 1)
            .or_else(|| interfaces.first())
        else {
            bail!("没有可用的无线网卡");
        };
        let xml: Vec<u16> = xml.encode_utf16().chain(Some(0)).collect();
        let mut reason = 0;
        if unsafe {
            WlanSetProfile(
                client.0,
                guid,
                2,
                xml.as_ptr(),
                ptr::null(),
                1,
                ptr::null(),
                &mut reason,
            )
        } != 0
        {
            bail!("无法保存 WiFi profile，请检查网络安全类型和密码");
        }
        Ok(())
    }
    pub(super) fn visible_networks() -> Result<Vec<String>> {
        let client = client()?;
        let mut networks = Vec::new();
        for (guid, _) in interfaces(&client)? {
            let mut data = ptr::null_mut();
            if unsafe { WlanGetAvailableNetworkList(client.0, &guid, 0, ptr::null(), &mut data) }
                != 0
                || data.is_null()
            {
                continue;
            }
            let block = Memory(data);
            let count = unsafe { *(block.0 as *const u32) }.min(2048);
            let entries = unsafe { (block.0 as *const u8).add(8) as *const Network };
            for i in 0..count {
                let name = ssid_text(unsafe { &(*entries.add(i as usize)).ssid });
                if !name.is_empty() {
                    networks.push(name);
                }
            }
        }
        networks.sort();
        networks.dedup();
        Ok(networks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_uses_utf8_ssid_octets_and_escapes_xml() {
        let cfg = FallbackWifiConfig {
            ssid: "测试&WiFi".into(),
            password: "synthetic<&>\"".into(),
            ..Default::default()
        };
        let xml = profile_xml(&cfg).unwrap();
        assert!(xml.contains("测试&amp;WiFi"));
        assert!(xml.contains("E6B58BE8AF952657694669"));
        assert!(xml.contains("synthetic&lt;&amp;&gt;&quot;"));
        assert!(!xml.contains("<authAlgorithm>"));
        assert!(xml.contains("<authentication>WPA2PSK</authentication>"));
    }
    #[test]
    fn open_profile_has_no_shared_secret() {
        let cfg = FallbackWifiConfig {
            ssid: "Fallback_WiFi".into(),
            encryption: "open".into(),
            auth: "open".into(),
            ..Default::default()
        };
        let xml = profile_xml(&cfg).unwrap();
        assert!(!xml.contains("sharedKey"));
        assert!(xml.contains("<encryption>none</encryption>"));
    }
    #[test]
    fn invalid_security_cannot_inject_xml() {
        let cfg = FallbackWifiConfig {
            ssid: "Fallback_WiFi".into(),
            encryption: "<bad>".into(),
            ..Default::default()
        };
        assert!(profile_xml(&cfg).is_err());
    }
    #[test]
    fn cancelled_connection_does_not_touch_wlan() {
        let stop = AtomicBool::new(true);
        assert!(!connect_wifi_cancel("Campus_WiFi", Duration::from_secs(120), &stop).unwrap());
    }
    #[test]
    fn startup_arguments_preserve_spaces_quotes_and_trailing_backslashes() {
        assert_eq!(
            quote_windows_arg(r"C:\Config Folder\config.yaml"),
            r#""C:\Config Folder\config.yaml""#
        );
        assert_eq!(quote_windows_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_windows_arg("C:\\folder\\"), "\"C:\\folder\\\\\"");
    }
}
