//! 原生 HTTP 认证：网关拦截链、CAS 会话和 eportal 工作流。
//!
//! 所有认证请求直连；账号只提交给配置中的 CAS 地址。错误不附带 URL、响应正文或凭据。

use std::io::Read;
use std::net::IpAddr;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use regex::Regex;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, LOCATION, ORIGIN, REFERER};
use reqwest::redirect::Policy;
use serde_json::{Value, json};
use url::Url;

use crate::config::AppConfig;

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_REDIRECTS: usize = 8;
const INTERNET_URLS: &[&str] = &[
    "https://www.msftconnecttest.com/connecttest.txt",
    "https://www.baidu.com/",
    "https://www.qq.com/",
];

/// 已经同时检查门户和外网的认证结果。
#[derive(Clone)]
pub struct AuthResult {
    pub success: bool,
    pub online: bool,
    pub session_id: Option<String>,
    pub message: String,
}

impl std::fmt::Debug for AuthResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthResult")
            .field("success", &self.success)
            .field("online", &self.online)
            .field(
                "session_id",
                &self.session_id.as_ref().map(|_| "[REDACTED]"),
            )
            .field("message", &self.message)
            .finish()
    }
}

/// 串行使用的认证客户端；每次操作重新解析 WLAN 地址，切网后不会复用旧连接。
pub struct AuthClient {
    cfg: AppConfig,
    base: Url,
    entry: Url,
    client: Client,
    internet_urls: Vec<Url>,
    cancellation: Option<Arc<AtomicBool>>,
}

impl AuthClient {
    /// 根据本地配置构造认证器，不发起网络请求。
    pub fn new(cfg: &AppConfig) -> Result<Self> {
        let base = parse_http_url(&cfg.portal.api_base)?;
        let entry = parse_http_url(&cfg.portal.entry_url)?;
        parse_http_url(&cfg.portal.probe_url)?;
        let client = build_client(cfg, &base)?;
        Ok(Self {
            cfg: cfg.clone(),
            base,
            entry,
            client,
            internet_urls: INTERNET_URLS
                .iter()
                .filter_map(|s| Url::parse(s).ok())
                .collect(),
            cancellation: None,
        })
    }

    /// 共享退出标记；当前请求结束后，不再执行后续认证或外网探测请求。
    pub fn set_cancellation(&mut self, cancellation: Arc<AtomicBool>) {
        self.cancellation = Some(cancellation);
    }

    /// 执行完整认证，只有门户和外网均确认在线才返回成功。
    pub fn login(&mut self) -> Result<AuthResult> {
        self.check_cancelled()?;
        self.refresh_client()?;
        let Some(session_id) = self.acquire_session()? else {
            if self.internet_online() {
                return Ok(AuthResult {
                    success: true,
                    online: true,
                    session_id: None,
                    message: "已在线，无需重新认证".into(),
                });
            }
            bail!("未检测到网关拦截且外网不可达，请检查校园网连接和探测地址");
        };

        let mut cas_url = self.endpoint("/cas-sso/login")?;
        cas_url.query_pairs_mut().extend_pairs([
            ("flowSessionId", session_id.as_str()),
            ("preview", "false"),
            ("appType", "normal"),
            ("language", "zh-CN"),
        ]);
        let response = self.get_portal(cas_url.clone())?;
        if !response.status().is_success() {
            bail!("CAS 登录页不可用，请稍后重试");
        }
        let html = read_text(response)?;
        let execution = extract_field(&html, "execution");
        if execution.is_empty() {
            bail!("CAS 登录页缺少 execution，请检查门户配置或门户版本");
        }
        let croypto = extract_field(&html, "croypto");
        let form = [
            ("username", self.cfg.account.username.as_str()),
            ("password", self.cfg.account.password.as_str()),
            ("type", "UsernamePassword"),
            ("_eventId", "submit"),
            ("geolocation", ""),
            ("execution", execution.as_str()),
            ("captcha_code", ""),
            ("croypto", croypto.as_str()),
            ("captcha_payload", ""),
        ];
        // 禁止自动重定向：307/308 不能把带密码的表单转交给另一个地址。
        self.check_cancelled()?;
        let response = self
            .client
            .post(cas_url.clone())
            .form(&form)
            .send()
            .map_err(|_| anyhow!("CAS 登录提交失败，请检查网络后重试"))?;
        if !matches!(response.status().as_u16(), 301..=303) {
            bail!("CAS 未接受登录，请检查账号密码或稍后重试");
        }
        let callback = response
            .headers()
            .get(LOCATION)
            .and_then(|s| s.to_str().ok())
            .ok_or_else(|| anyhow!("CAS 未返回认证回调"))?;
        let callback = self.portal_target(&cas_url, callback)?;
        if callback.path() != "/portal/assets/auth-success.html" {
            bail!("CAS 返回了非预期认证回调");
        }
        // 使用服务端签发的原始 ticket，而不是历史实现中的 ticket=ignored。
        let response = self.get_portal(callback)?;
        if !response.status().is_success() {
            bail!("CAS 认证回调未完成，请重试登录");
        }
        self.finish_workflow(&session_id)?;
        let portal_online = self.portal_online(&session_id)?;
        let online = portal_online && self.internet_online();
        Ok(AuthResult {
            success: online,
            online,
            session_id: Some(session_id),
            message: if online {
                "认证成功，校园网已连接".into()
            } else {
                "门户认证已完成，外网尚未确认可达，将继续重试".into()
            },
        })
    }

    /// 检查外网真实可达性；已有会话时同时验证门户状态。
    pub fn probe(&mut self, session_id: Option<&str>) -> Result<bool> {
        self.check_cancelled()?;
        self.refresh_client()?;
        if let Some(session_id) = session_id
            && !self.portal_online(session_id).unwrap_or(false)
        {
            return Ok(false);
        }
        Ok(self.internet_online())
    }

    fn refresh_client(&mut self) -> Result<()> {
        self.client = build_client(&self.cfg, &self.base)?;
        Ok(())
    }

    fn check_cancelled(&self) -> Result<()> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            bail!("操作已取消");
        }
        Ok(())
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.base
            .join(path)
            .map_err(|_| anyhow!("门户 API 地址无效"))
    }

    fn portal_target(&self, from: &Url, target: &str) -> Result<Url> {
        let target = from
            .join(&html_escape::decode_html_entities(target))
            .map_err(|_| anyhow!("门户返回了无效跳转地址"))?;
        if !target.username().is_empty()
            || target.password().is_some()
            || !matches!(target.scheme(), "http" | "https")
            || (target.origin() != self.base.origin() && target.origin() != self.entry.origin())
        {
            bail!("已阻止跳转到配置之外的门户地址，请检查门户配置");
        }
        Ok(target)
    }

    fn get_portal(&self, mut url: Url) -> Result<Response> {
        for _ in 0..MAX_REDIRECTS {
            self.check_cancelled()?;
            let response = self
                .client
                .get(url.clone())
                .send()
                .map_err(|_| anyhow!("无法访问门户，请检查校园网连接"))?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let target = response
                .headers()
                .get(LOCATION)
                .and_then(|s| s.to_str().ok())
                .ok_or_else(|| anyhow!("门户重定向缺少目标地址"))?;
            url = self.portal_target(&url, target)?;
        }
        bail!("门户重定向次数过多，请检查门户配置");
    }

    fn acquire_session(&self) -> Result<Option<String>> {
        self.check_cancelled()?;
        let probe = parse_http_url(&self.cfg.portal.probe_url)?;
        let response = self
            .client
            .get(probe.clone())
            .send()
            .map_err(|_| anyhow!("无法访问网关探测地址，请检查校园网连接"))?;
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|s| s.to_str().ok())
            .map(str::to_owned);
        let body = read_text(response)?;
        let target = location.or_else(|| gateway_redirect(&body));
        let Some(target) = target else {
            return Ok(None);
        };
        let resolved = probe
            .join(&html_escape::decode_html_entities(&target))
            .map_err(|_| anyhow!("网关返回了无效跳转地址"))?;
        if resolved.path() != "/eportal/index.jsp"
            && !resolved
                .query_pairs()
                .any(|(name, _)| name == "sessionId" || name == "flowSessionId")
        {
            // 普通网站的 HTTP → HTTPS 跳转不是校园网拦截。
            return Ok(None);
        }
        // 只有已知门户才能收到拦截会话；任意网页中的 location 不会被信任。
        let mut current = self.portal_target(&probe, &target)?;
        for _ in 0..MAX_REDIRECTS {
            self.check_cancelled()?;
            if let Some(session_id) = current
                .query_pairs()
                .find(|(name, _)| name == "sessionId" || name == "flowSessionId")
                .map(|(_, value)| value.into_owned())
            {
                if !session_id.is_empty() && session_id.len() <= 128 {
                    return Ok(Some(session_id));
                }
                bail!("网关返回的认证会话无效");
            }
            let response = self
                .client
                .get(current.clone())
                .send()
                .map_err(|_| anyhow!("无法读取网关会话，请重试登录"))?;
            let target = response
                .headers()
                .get(LOCATION)
                .and_then(|s| s.to_str().ok())
                .map(str::to_owned);
            let body = read_text(response)?;
            let Some(target) = target.or_else(|| gateway_redirect(&body)) else {
                return Ok(None);
            };
            current = self.portal_target(&current, &target)?;
        }
        bail!("网关认证跳转次数过多，请检查门户配置");
    }

    fn post_json(&self, path: &str, payload: Value) -> Result<Value> {
        self.check_cancelled()?;
        let response = self
            .client
            .post(self.endpoint(path)?)
            .header(ACCEPT, "application/json, text/plain, */*")
            .header("X-Requested-With", "XMLHttpRequest")
            .header(ORIGIN, self.base.origin().ascii_serialization())
            .header(
                REFERER,
                self.endpoint("/portal/assets/auth-success.html")?.as_str(),
            )
            .json(&payload)
            .send()
            .map_err(|_| anyhow!("门户 API 请求失败，请稍后重试"))?;
        if !response.status().is_success() {
            bail!("门户 API 暂不可用，请稍后重试");
        }
        let body = read_text(response)?;
        let value: Value =
            serde_json::from_str(&body).map_err(|_| anyhow!("门户 API 返回了非预期数据"))?;
        if value.get("code").and_then(Value::as_i64) != Some(200) {
            bail!("门户未接受本次会话，请重新认证");
        }
        Ok(value)
    }

    fn finish_workflow(&self, session_id: &str) -> Result<()> {
        let payload = || json!({"sessionId": session_id, "flowKey": "portal_auth"});
        let node = self.post_json("/eportal/workFlow/getCurrentNode", payload())?;
        if node
            .pointer("/data/currentNodePath")
            .and_then(Value::as_str)
            == Some("finish")
        {
            return Ok(());
        }
        let services = self.post_json(
            "/eportal/network/serviceSelection",
            json!({"sessionId":session_id}),
        )?;
        if !services
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("value").and_then(Value::as_str) == Some(&self.cfg.portal.service)
                })
            })
        {
            bail!("所选运营商不在门户可用列表中，请修改运营商设置");
        }
        let result = self.post_json(
            "/eportal/network/serviceLogin",
            json!({"sessionId":session_id,"service":self.cfg.portal.service}),
        )?;
        if result.pointer("/data/authResult").and_then(Value::as_str) != Some("success") {
            bail!("运营商认证失败，请检查账号绑定或稍后重试");
        }
        let node = self.post_json("/eportal/workFlow/getCurrentNode", payload())?;
        if node
            .pointer("/data/currentNodePath")
            .and_then(Value::as_str)
            != Some("finish")
        {
            bail!("门户工作流未完成，请稍后重试");
        }
        Ok(())
    }

    fn portal_online(&self, session_id: &str) -> Result<bool> {
        let result = self.post_json(
            "/eportal/network/userOnline",
            json!({"sessionId": session_id}),
        )?;
        Ok(result.pointer("/data/online").and_then(Value::as_bool) == Some(true))
    }

    fn internet_online(&self) -> bool {
        self.internet_urls.iter().any(|url| {
            if self.check_cancelled().is_err() {
                return false;
            }
            let permitted = url.scheme() == "https";
            #[cfg(test)]
            let permitted = permitted || is_loopback(url);
            if !permitted {
                return false;
            }
            let Ok(response) = self.client.get(url.clone()).send() else {
                return false;
            };
            if !response.status().is_success() {
                return false;
            }
            let Ok(body) = read_text(response) else {
                return false;
            };
            if url.host_str() == Some("www.msftconnecttest.com") {
                return body.trim() == "Microsoft Connect Test";
            }
            let lower = body.chars().take(4000).collect::<String>().to_lowercase();
            body.len() >= 500
                && ![
                    "cas-sso",
                    "eportal",
                    "captive portal",
                    "name=\"password\"",
                    "name='password'",
                ]
                .iter()
                .any(|marker| lower.contains(marker))
        })
    }
}

fn parse_http_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| anyhow!("门户或探测地址无效"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("门户或探测地址必须是无内嵌凭据的 HTTP/HTTPS 地址");
    }
    Ok(url)
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host == "localhost",
        None => false,
    }
}

fn build_client(cfg: &AppConfig, base: &Url) -> Result<Client> {
    let timeout = Duration::from_secs(cfg.portal.timeout.clamp(1, 120));
    let mut builder = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .cookie_store(true)
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(6)))
        .pool_max_idle_per_host(1)
        .user_agent(if cfg.portal.user_agent.trim().is_empty() {
            "campus-guardian/0.2"
        } else {
            &cfg.portal.user_agent
        });
    let address = if is_loopback(base) {
        None
    } else {
        crate::platform::wlan_ipv4()
    };
    if let Some(address) = source_address(cfg, base, address)? {
        builder = builder.local_address(IpAddr::V4(address));
    }
    builder
        .build()
        .map_err(|_| anyhow!("无法创建直连认证客户端，请检查网络配置"))
}

fn source_address(
    cfg: &AppConfig,
    base: &Url,
    address: Option<std::net::Ipv4Addr>,
) -> Result<Option<std::net::Ipv4Addr>> {
    if is_loopback(base) {
        return Ok(None);
    }
    if address.is_some_and(|ip| ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast()) {
        bail!("无线网卡的源 IPv4 地址无效，请检查网络或绑源配置");
    }
    if address.is_none() && !cfg.wifi.ssid.is_empty() {
        bail!("无线网卡尚未取得 IPv4 地址，请等待网络连接后重试");
    }
    Ok(address)
}

fn read_text(response: Response) -> Result<String> {
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("网络响应未完整接收，请稍后重试"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        bail!("网络响应超过安全大小限制");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn gateway_redirect(body: &str) -> Option<String> {
    static REDIRECT: OnceLock<Regex> = OnceLock::new();
    let re = REDIRECT.get_or_init(|| {
        Regex::new(r#"(?i)(?:window\.)?location(?:\.href)?\s*=\s*["']([^"']+)["']"#)
            .expect("constant gateway regex")
    });
    re.captures(body)
        .map(|m| html_escape::decode_html_entities(&m[1]).into_owned())
}

fn extract_field(body: &str, wanted: &str) -> String {
    static INPUT: OnceLock<Regex> = OnceLock::new();
    static PARAGRAPH: OnceLock<Regex> = OnceLock::new();
    let inputs =
        INPUT.get_or_init(|| Regex::new(r"(?is)<input\b([^>]*)>").expect("constant input regex"));
    for input in inputs.captures_iter(body) {
        if attribute(&input[1], "name").as_deref() == Some(wanted)
            || attribute(&input[1], "id").as_deref() == Some(wanted)
        {
            return attribute(&input[1], "value").unwrap_or_default();
        }
    }
    let paragraphs = PARAGRAPH.get_or_init(|| {
        Regex::new(r"(?is)<p\b([^>]*)>([^<]*)</p\s*>").expect("constant paragraph regex")
    });
    for paragraph in paragraphs.captures_iter(body) {
        let id = attribute(&paragraph[1], "id");
        if id.as_deref() == Some(wanted)
            || (wanted == "execution" && id.as_deref() == Some("login-page-flowkey"))
        {
            return html_escape::decode_html_entities(paragraph[2].trim()).into_owned();
        }
    }
    String::new()
}

fn attribute(tag: &str, wanted: &str) -> Option<String> {
    static ATTRIBUTE: OnceLock<Regex> = OnceLock::new();
    let re = ATTRIBUTE.get_or_init(|| {
        Regex::new(r#"(?i)\b([a-z_:][\w:.-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))"#)
            .expect("constant attribute regex")
    });
    re.captures_iter(tag).find_map(|m| {
        if !m[1].eq_ignore_ascii_case(wanted) {
            return None;
        }
        (2..=4)
            .find_map(|i| m.get(i))
            .map(|value| html_escape::decode_html_entities(value.as_str()).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    struct Route {
        method: &'static str,
        path: &'static str,
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    }

    impl Route {
        fn get(path: &'static str, body: impl Into<String>) -> Self {
            Self {
                method: "GET",
                path,
                status: 200,
                headers: vec![],
                body: body.into(),
            }
        }

        fn redirect(method: &'static str, path: &'static str, location: &'static str) -> Self {
            Self {
                method,
                path,
                status: 302,
                headers: vec![("Location", location)],
                body: String::new(),
            }
        }

        fn json(path: &'static str, value: Value) -> Self {
            Self {
                method: "POST",
                path,
                status: 200,
                headers: vec![("Content-Type", "application/json")],
                body: value.to_string(),
            }
        }
    }

    struct Server {
        base: String,
        handle: Option<JoinHandle<Vec<String>>>,
    }

    impl Server {
        fn start(routes: Vec<Route>) -> Self {
            Self::start_with_cancellation(routes, None)
        }

        fn start_with_cancellation(
            routes: Vec<Route>,
            cancellation: Option<(usize, Arc<AtomicBool>)>,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let handle = thread::spawn(move || {
                let mut requests = vec![];
                let deadline = Instant::now() + Duration::from_secs(10);
                for (index, route) in routes.into_iter().enumerate() {
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(
                                    Instant::now() < deadline,
                                    "mock server did not receive expected request"
                                );
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("mock listener error: {error}"),
                        }
                    };
                    // Windows 的 accept 会继承监听 socket 的非阻塞状态。
                    // 监听循环保留非阻塞，已接受连接用限时阻塞读取完整请求。
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let request = read_request(&mut stream);
                    let first = request.lines().next().unwrap();
                    let parts = first.split_whitespace().collect::<Vec<_>>();
                    assert_eq!(parts[0], route.method);
                    assert_eq!(parts[1].split('?').next().unwrap(), route.path);
                    requests.push(request);
                    if let Some((stop_after, flag)) = &cancellation
                        && index == *stop_after
                    {
                        flag.store(true, Ordering::Release);
                    }
                    let headers = route
                        .headers
                        .iter()
                        .map(|(key, value)| format!("{key}: {value}\r\n"))
                        .collect::<String>();
                    write!(
                        stream,
                        "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                        route.status,
                        route.body.len(),
                        headers,
                        route.body
                    )
                    .unwrap();
                }
                requests
            });
            Self {
                base,
                handle: Some(handle),
            }
        }

        fn finish(mut self) -> Vec<String> {
            self.handle
                .take()
                .unwrap()
                .join()
                .expect("mock route assertions failed")
        }

        fn client(&self) -> AuthClient {
            let cfg: AppConfig = serde_json::from_value(json!({
                "portal": {"entry_url":format!("{}/pc/center", self.base), "api_base":self.base,"probe_url":format!("{}/probe", self.base),"service":"cmcc","timeout":2,"user_agent":"Synthetic-Test-Agent"},
                "account":{"username":"2024000000","password":"synthetic test & password"}
            })).unwrap();
            let mut client = AuthClient::new(&cfg).unwrap();
            client.internet_urls = vec![Url::parse(&format!("{}/internet", self.base)).unwrap()];
            client
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = vec![];
        let header_end = loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 64 * 1024);
            if bytes.ends_with(b"\r\n\r\n") {
                break bytes.len();
            }
        };
        let headers = String::from_utf8_lossy(&bytes);
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        bytes.resize(header_end + length, 0);
        stream.read_exact(&mut bytes[header_end..]).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    fn gateway_routes() -> Vec<Route> {
        vec![
            Route::get(
                "/probe",
                "<script>location.href='/eportal/index.jsp?wlanuserip=10.20.30.40&amp;mac=aabbccddeeff'</script>",
            ),
            Route::redirect(
                "GET",
                "/eportal/index.jsp",
                "/portal/portal-main?sessionId=deadbeef1234",
            ),
        ]
    }

    fn cas_routes() -> Vec<Route> {
        let mut form = Route::get(
            "/cas-sso/login",
            "<input value='token&amp;value' name='execution'><p id='croypto'>synthetic-key</p>",
        );
        form.headers
            .push(("Set-Cookie", "CAS_TEST=session; Path=/; HttpOnly"));
        vec![
            form,
            Route::redirect(
                "POST",
                "/cas-sso/login",
                "/portal/assets/auth-success.html?ticket=ST-SYNTHETIC",
            ),
            Route::get("/portal/assets/auth-success.html", "ok"),
        ]
    }

    fn successful_routes() -> Vec<Route> {
        let mut routes = gateway_routes();
        routes.extend(cas_routes());
        routes.extend([
            Route::json(
                "/eportal/workFlow/getCurrentNode",
                json!({"code":200,"data":{"currentNodePath":"serviceSelection"}}),
            ),
            Route::json(
                "/eportal/network/serviceSelection",
                json!({"code":200,"data":[{"value":"cmcc"}]}),
            ),
            Route::json(
                "/eportal/network/serviceLogin",
                json!({"code":200,"data":{"authResult":"success"}}),
            ),
            Route::json(
                "/eportal/workFlow/getCurrentNode",
                json!({"code":200,"data":{"currentNodePath":"finish"}}),
            ),
            Route::json(
                "/eportal/network/userOnline",
                json!({"code":200,"data":{"online":true}}),
            ),
            Route::get("/internet", "x".repeat(600)),
        ]);
        routes
    }

    #[test]
    fn complete_auth_flow_preserves_cookie_ticket_and_encoded_form() {
        let server = Server::start(successful_routes());
        let result = server.client().login().unwrap();
        assert!(result.success && result.online);
        assert_eq!(result.session_id.as_deref(), Some("deadbeef1234"));
        let requests = server.finish();
        let submit = &requests[3];
        assert!(submit.to_lowercase().contains("cookie: cas_test=session"));
        assert!(submit.contains("flowSessionId=deadbeef1234"));
        assert!(submit.contains("username=2024000000"));
        assert!(submit.contains("password=synthetic+test+%26+password"));
        assert!(submit.contains("execution=token%26value"));
        assert!(requests[4].contains("ticket=ST-SYNTHETIC"));
        assert!(requests.iter().all(|request| {
            request
                .to_lowercase()
                .contains("user-agent: synthetic-test-agent")
        }));
    }

    #[test]
    fn gateway_http_redirect_form_also_authenticates() {
        let mut routes = successful_routes();
        routes[0] = Route::redirect("GET", "/probe", "/eportal/index.jsp?synthetic=1");
        let server = Server::start(routes);
        assert!(server.client().login().unwrap().success);
        server.finish();
    }

    #[test]
    fn portal_success_without_internet_is_not_login_success() {
        let mut routes = successful_routes();
        routes.last_mut().unwrap().body = "<html>captive portal</html>".repeat(30);
        let server = Server::start(routes);
        let result = server.client().login().unwrap();
        assert!(!result.success && !result.online);
        server.finish();
    }

    #[test]
    fn portal_offline_result_cannot_report_login_success() {
        let mut routes = successful_routes();
        routes.truncate(10);
        routes.last_mut().unwrap().body = json!({"code":200,"data":{"online":false}}).to_string();
        let server = Server::start(routes);
        let result = server.client().login().unwrap();
        assert!(!result.success && !result.online);
        server.finish();
    }

    #[test]
    fn already_online_does_not_submit_credentials() {
        let server = Server::start(vec![
            Route::get("/probe", "ordinary website"),
            Route::get("/internet", "x".repeat(600)),
        ]);
        let result = server.client().login().unwrap();
        assert!(result.success && result.online && result.session_id.is_none());
        assert!(
            server
                .finish()
                .iter()
                .all(|request| request.starts_with("GET "))
        );
    }

    #[test]
    fn ordinary_website_redirect_can_still_mean_already_online() {
        let server = Server::start(vec![
            Route::redirect("GET", "/probe", "https://example.org/"),
            Route::get("/internet", "x".repeat(600)),
        ]);
        assert!(server.client().login().unwrap().success);
        server.finish();
    }

    #[test]
    fn gateway_external_origin_is_rejected_before_following() {
        let server = Server::start(vec![Route::get(
            "/probe",
            "<script>location.href='http://untrusted.invalid/eportal/index.jsp'</script>",
        )]);
        let error = server.client().login().unwrap_err().to_string();
        assert!(error.contains("阻止"));
        assert!(!error.contains("untrusted.invalid"));
        server.finish();
    }

    #[test]
    fn cas_callback_external_origin_is_rejected() {
        let mut routes = gateway_routes();
        let mut cas = cas_routes();
        cas.truncate(2);
        cas[1].headers = vec![(
            "Location",
            "http://untrusted.invalid/portal/assets/auth-success.html?ticket=ST-PRIVATE",
        )];
        routes.extend(cas);
        let server = Server::start(routes);
        let error = server.client().login().unwrap_err().to_string();
        assert!(error.contains("阻止"));
        assert!(!error.contains("ST-PRIVATE"));
        server.finish();
    }

    #[test]
    fn rejected_cas_post_does_not_leak_response_or_credentials() {
        let mut routes = gateway_routes();
        routes.push(Route::get(
            "/cas-sso/login",
            "<input name=execution value=synthetic-token>",
        ));
        routes.push(Route {
            method: "POST",
            path: "/cas-sso/login",
            status: 200,
            headers: vec![],
            body: "synthetic test & password".into(),
        });
        let server = Server::start(routes);
        let error = server.client().login().unwrap_err().to_string();
        assert!(error.contains("CAS"));
        assert!(!error.contains("password"));
        server.finish();
    }

    #[test]
    fn missing_execution_is_clear_and_does_not_submit_password() {
        let mut routes = gateway_routes();
        routes.push(Route::get("/cas-sso/login", "missing token"));
        let server = Server::start(routes);
        assert!(
            server
                .client()
                .login()
                .unwrap_err()
                .to_string()
                .contains("execution")
        );
        assert_eq!(server.finish().len(), 3);
    }

    #[test]
    fn cas_307_is_not_followed_with_credentials() {
        let mut routes = gateway_routes();
        routes.push(Route::get(
            "/cas-sso/login",
            "<input name=execution value=synthetic-token>",
        ));
        let mut redirect = Route::redirect("POST", "/cas-sso/login", "/another-handler");
        redirect.status = 307;
        routes.push(redirect);
        let server = Server::start(routes);
        assert!(server.client().login().is_err());
        assert_eq!(server.finish().len(), 4);
    }

    #[test]
    fn finished_workflow_skips_service_selection_and_login() {
        let mut routes = gateway_routes();
        routes.extend(cas_routes());
        routes.extend([
            Route::json(
                "/eportal/workFlow/getCurrentNode",
                json!({"code":200,"data":{"currentNodePath":"finish"}}),
            ),
            Route::json(
                "/eportal/network/userOnline",
                json!({"code":200,"data":{"online":true}}),
            ),
            Route::get("/internet", "x".repeat(600)),
        ]);
        let server = Server::start(routes);
        assert!(server.client().login().unwrap().success);
        assert_eq!(server.finish().len(), 8);
    }

    #[test]
    fn failed_service_login_cannot_report_success() {
        let mut routes = gateway_routes();
        routes.extend(cas_routes());
        routes.extend([
            Route::json(
                "/eportal/workFlow/getCurrentNode",
                json!({"code":200,"data":{"currentNodePath":"serviceSelection"}}),
            ),
            Route::json(
                "/eportal/network/serviceSelection",
                json!({"code":200,"data":[{"value":"cmcc"}]}),
            ),
            Route::json(
                "/eportal/network/serviceLogin",
                json!({"code":200,"data":{"authResult":"failed"}}),
            ),
        ]);
        let server = Server::start(routes);
        assert!(
            server
                .client()
                .login()
                .unwrap_err()
                .to_string()
                .contains("运营商")
        );
        server.finish();
    }

    #[test]
    fn unfinished_workflow_is_not_a_success() {
        let mut routes = successful_routes();
        routes.truncate(9);
        routes.last_mut().unwrap().body =
            json!({"code":200,"data":{"currentNodePath":"authenticate"}}).to_string();
        let server = Server::start(routes);
        assert!(
            server
                .client()
                .login()
                .unwrap_err()
                .to_string()
                .contains("工作流")
        );
        server.finish();
    }

    #[test]
    fn invalid_json_is_a_sanitized_probe_failure() {
        let server = Server::start(vec![Route {
            method: "POST",
            path: "/eportal/network/userOnline",
            status: 200,
            headers: vec![],
            body: "invalid upstream secret-token".into(),
        }]);
        assert!(!server.client().probe(Some("deadbeef1234")).unwrap());
        server.finish();
    }

    #[test]
    fn unavailable_service_fails_before_service_login() {
        let mut routes = gateway_routes();
        routes.extend(cas_routes());
        routes.extend([
            Route::json(
                "/eportal/workFlow/getCurrentNode",
                json!({"code":200,"data":{"currentNodePath":"serviceSelection"}}),
            ),
            Route::json(
                "/eportal/network/serviceSelection",
                json!({"code":200,"data":[{"value":"ctcc"}]}),
            ),
        ]);
        let server = Server::start(routes);
        assert!(
            server
                .client()
                .login()
                .unwrap_err()
                .to_string()
                .contains("运营商")
        );
        server.finish();
    }

    #[test]
    fn stale_portal_online_flag_cannot_hide_internet_failure() {
        let server = Server::start(vec![
            Route::json(
                "/eportal/network/userOnline",
                json!({"code":200,"data":{"online":true}}),
            ),
            Route::get("/internet", "short captive page"),
        ]);
        assert!(!server.client().probe(Some("deadbeef1234")).unwrap());
        server.finish();
    }

    #[test]
    fn api_business_failure_is_not_treated_as_success() {
        let server = Server::start(vec![Route::json(
            "/eportal/network/userOnline",
            json!({"code":400,"data":{"online":true},"message":"sessionId=deadbeef1234"}),
        )]);
        assert!(!server.client().probe(Some("deadbeef1234")).unwrap());
        server.finish();
    }

    #[test]
    fn extract_field_handles_attribute_order_entities_and_new_portal_markup() {
        assert_eq!(
            extract_field("<input value='a&amp;b' name='execution'>", "execution"),
            "a&b"
        );
        assert_eq!(
            extract_field("<p id=login-page-flowkey> synthetic-flow </p>", "execution"),
            "synthetic-flow"
        );
        assert_eq!(
            extract_field("<INPUT ID=execution VALUE=synthetic-token>", "execution"),
            "synthetic-token"
        );
        assert!(extract_field("<html>missing</html>", "execution").is_empty());
    }

    #[test]
    fn configured_wifi_without_ipv4_cannot_use_default_ethernet_route() {
        let mut cfg = AppConfig::default();
        cfg.wifi.ssid = "Campus_WiFi".into();
        let base = Url::parse("http://portal.test/").unwrap();
        assert!(source_address(&cfg, &base, None).is_err());
        let ip = std::net::Ipv4Addr::new(10, 20, 30, 40);
        assert_eq!(source_address(&cfg, &base, Some(ip)).unwrap(), Some(ip));
        assert!(source_address(&cfg, &base, Some(std::net::Ipv4Addr::UNSPECIFIED)).is_err());
    }

    #[test]
    fn intentional_ethernet_mode_and_loopback_do_not_require_wlan() {
        let mut cfg = AppConfig::default();
        let base = Url::parse("http://portal.test/").unwrap();
        assert_eq!(source_address(&cfg, &base, None).unwrap(), None);
        cfg.wifi.ssid = "Campus_WiFi".into();
        for base in ["http://127.0.0.1/", "http://[::1]/", "http://localhost/"] {
            assert_eq!(
                source_address(&cfg, &Url::parse(base).unwrap(), None).unwrap(),
                None
            );
        }
    }

    #[test]
    fn cancellation_prevents_any_new_http_request() {
        let server = Server::start(vec![]);
        let mut client = server.client();
        client.set_cancellation(Arc::new(AtomicBool::new(true)));
        assert!(client.login().unwrap_err().to_string().contains("取消"));
        assert!(client.probe(None).unwrap_err().to_string().contains("取消"));
        assert!(server.finish().is_empty());
    }

    #[test]
    fn cancellation_after_gateway_response_stops_redirect_chain() {
        let flag = Arc::new(AtomicBool::new(false));
        let server = Server::start_with_cancellation(
            vec![Route::get(
                "/probe",
                "<script>location.href='/eportal/index.jsp'</script>",
            )],
            Some((0, flag.clone())),
        );
        let mut client = server.client();
        client.set_cancellation(flag);
        assert!(client.login().unwrap_err().to_string().contains("取消"));
        assert_eq!(server.finish().len(), 1);
    }
}
