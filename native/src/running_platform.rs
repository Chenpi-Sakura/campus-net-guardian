//! 校园跑平台接入：登录、运动规则与记录提交；凭据与令牌仅在本机配置和守护进程内传递。

use crate::{
    config::RunningPlatformConfig,
    running::{Preview, TrackPoint},
};
use anyhow::{Result, bail};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// 平台登录结果与运动规则摘要；不含令牌与个人身份细节。
#[derive(Clone)]
pub struct PlatformLoginInfo {
    /// 平台用户 ID。
    pub user_id: i64,
    /// 当前学期标识。
    pub semester_year: String,
    /// 学校运动范围多边形文本。
    pub site_bound: String,
    /// 平台报告的单次运动时间下限。
    pub once_time_min: u32,
    /// 平台报告的单次运动时间上限。
    pub once_time_max: u32,
}

/// 平台记录提交结果。
#[derive(Clone)]
pub struct PlatformSubmitInfo {
    /// 平台生成的运动记录 ID。
    pub record_id: i64,
    /// 平台返回的结果状态码。
    pub result_status: String,
    /// 平台返回的结果描述。
    pub result_desc: String,
    /// 平台返回的超速警告次数。
    pub over_speed_warn: String,
    /// 平台返回的警告内容。
    pub warn_content: String,
}

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];
const MD5_K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// MD5 摘要，小写十六进制；用于平台密码散列与请求签名。
fn md5_hex(input: &[u8]) -> String {
    let mut a: u32 = 0x6745_2301;
    let mut b: u32 = 0xefcd_ab89;
    let mut c: u32 = 0x98ba_dcfe;
    let mut d: u32 = 0x1032_5476;
    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in message.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (index, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        let (mut aa, mut bb, mut cc, mut dd) = (a, b, c, d);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((bb & cc) | (!bb & dd), i),
                16..=31 => ((dd & bb) | (!dd & cc), (5 * i + 1) % 16),
                32..=47 => (bb ^ cc ^ dd, (3 * i + 5) % 16),
                _ => (cc ^ (bb | !dd), (7 * i) % 16),
            };
            let temp = dd;
            dd = cc;
            cc = bb;
            let sum = aa.wrapping_add(f).wrapping_add(MD5_K[i]).wrapping_add(m[g]);
            bb = bb.wrapping_add(sum.rotate_left(MD5_S[i]));
            aa = temp;
        }
        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }
    let mut out = String::with_capacity(32);
    for word in [a, b, c, d] {
        for byte in word.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

/// 平台请求签名：按键序拼接参数、appkey、密钥与请求体后取大写 MD5。
fn sign(
    params: &BTreeMap<String, String>,
    body: Option<&str>,
    app_key: &str,
    app_secret: &str,
) -> String {
    let mut base = String::new();
    for (key, value) in params {
        base.push_str(key);
        base.push_str(value);
    }
    base.push_str(app_key);
    base.push_str(app_secret);
    if let Some(body) = body {
        base.push_str(body);
    }
    let stripped: String = base
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '~' | '!' | '(' | ')' | '\''))
        .collect();
    if stripped == base {
        md5_hex(base.as_bytes()).to_uppercase()
    } else {
        let encoded: String = url::form_urlencoded::byte_serialize(stripped.as_bytes()).collect();
        format!("{}encodeutf8", md5_hex(encoded.as_bytes()).to_uppercase())
    }
}

const GCJ_A: f64 = 6_378_245.0;
const GCJ_EE: f64 = 0.006_693_421_622_965_943;

fn out_of_china(lon: f64, lat: f64) -> bool {
    !(72.004..=137.8347).contains(&lon) || !(0.8293..=55.8271).contains(&lat)
}
fn gcj_lat_offset(x: f64, y: f64) -> f64 {
    let mut result = -100.0 + 2.0 * x + 3.0 * y + 0.2 * y * y + 0.1 * x * y + 0.2 * x.abs().sqrt();
    result += (20.0 * (6.0 * x * std::f64::consts::PI).sin()
        + 20.0 * (2.0 * x * std::f64::consts::PI).sin())
        * 2.0
        / 3.0;
    result += (20.0 * (y * std::f64::consts::PI).sin()
        + 40.0 * (y / 3.0 * std::f64::consts::PI).sin())
        * 2.0
        / 3.0;
    result += (160.0 * (y / 12.0 * std::f64::consts::PI).sin()
        + 320.0 * (y * std::f64::consts::PI / 30.0).sin())
        * 2.0
        / 3.0;
    result
}
fn gcj_lon_offset(x: f64, y: f64) -> f64 {
    let mut result = 300.0 + x + 2.0 * y + 0.1 * x * x + 0.1 * x * y + 0.1 * x.abs().sqrt();
    result += (20.0 * (6.0 * x * std::f64::consts::PI).sin()
        + 20.0 * (2.0 * x * std::f64::consts::PI).sin())
        * 2.0
        / 3.0;
    result += (20.0 * (x * std::f64::consts::PI).sin()
        + 40.0 * (x / 3.0 * std::f64::consts::PI).sin())
        * 2.0
        / 3.0;
    result += (150.0 * (x / 12.0 * std::f64::consts::PI).sin()
        + 300.0 * (x / 30.0 * std::f64::consts::PI).sin())
        * 2.0
        / 3.0;
    result
}

/// WGS84 坐标转 GCJ-02；中国境外坐标原样返回。
fn wgs84_to_gcj02(lon: f64, lat: f64) -> (f64, f64) {
    if out_of_china(lon, lat) {
        return (lon, lat);
    }
    let mut d_lat = gcj_lat_offset(lon - 105.0, lat - 35.0);
    let mut d_lon = gcj_lon_offset(lon - 105.0, lat - 35.0);
    let rad = lat / 180.0 * std::f64::consts::PI;
    let mut magic = rad.sin();
    magic = 1.0 - GCJ_EE * magic * magic;
    let sqrt_magic = magic.sqrt();
    d_lat =
        (d_lat * 180.0) / ((GCJ_A * (1.0 - GCJ_EE)) / (magic * sqrt_magic) * std::f64::consts::PI);
    d_lon = (d_lon * 180.0) / (GCJ_A / sqrt_magic * rad.cos() * std::f64::consts::PI);
    (lon + d_lon, lat + d_lat)
}

#[derive(Deserialize)]
struct CampusSource {
    campus_id: String,
    origin_wgs84: [f64; 2],
    meters_per_longitude_degree: f64,
    meters_per_latitude_degree: f64,
}

/// 内置校区局部米坐标的地理投影锚点。
#[derive(Clone, Copy)]
pub(crate) struct Projection {
    lon0: f64,
    lat0: f64,
    sx: f64,
    sy: f64,
}

/// 按校区 ID 查找投影锚点；外部路线目录没有地理锚点，返回 None。
pub(crate) fn campus_projection(campus_id: &str) -> Option<Projection> {
    let sources: Vec<CampusSource> =
        serde_json::from_str(include_str!("../../assets/campus/sources.json")).ok()?;
    sources
        .iter()
        .find(|source| source.campus_id == campus_id)
        .map(|source| Projection {
            lon0: source.origin_wgs84[0],
            lat0: source.origin_wgs84[1],
            sx: source.meters_per_longitude_degree,
            sy: source.meters_per_latitude_degree,
        })
}

fn next_random(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// 将本地轨迹序列化为平台的「经度,纬度-毫秒时间戳-精度」字符串数组 JSON。
pub(crate) fn track_points_json(
    points: &[TrackPoint],
    duration_seconds: u32,
    projection: &Projection,
    end_ms: u64,
    seed: u64,
) -> Result<String> {
    if points.len() < 2 {
        bail!("轨迹点不足，无法提交");
    }
    let duration_ms = u64::from(duration_seconds) * 1000;
    let start_ms = end_ms.saturating_sub(duration_ms);
    let mut random = seed | 1;
    let mut entries = Vec::with_capacity(points.len());
    for point in points {
        let lon = projection.lon0 + point.position.x / projection.sx;
        let lat = projection.lat0 + point.position.y / projection.sy;
        let (lon, lat) = wgs84_to_gcj02(lon, lat);
        let timestamp = start_ms + point.elapsed_millis.min(duration_ms);
        let accuracy = (next_random(&mut random) % 100) as f64 / 10.0;
        entries.push(format!("{lon:.6},{lat:.6}-{timestamp}-{accuracy:.1}"));
    }
    serde_json::to_string(&entries).map_err(|_| anyhow::anyhow!("无法序列化轨迹"))
}

/// Unix 秒对应的北京时间（UTC+8）日期，格式 yyyy-MM-dd。
fn beijing_date(unix_seconds: u64) -> String {
    let days = ((unix_seconds + 8 * 3600) / 86400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}")
}

/// 生成平台记录提交体；距离单位为米，时间单位为秒，日期取北京时间。
pub(crate) fn record_body(
    contract: &RunningPlatformConfig,
    info: &PlatformLoginInfo,
    preview: &Preview,
    track_points: &str,
    now: SystemTime,
) -> Result<serde_json::Value> {
    let unix = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("系统时间异常"))?
        .as_secs();
    let distance = preview.distance_m.round();
    if !distance.is_finite() || !(0.0..=1_000_000.0).contains(&distance) {
        bail!("预览距离无效，无法提交");
    }
    Ok(serde_json::json!({
        "againRunStatus": "0",
        "againRunTime": 0,
        "appVersions": contract.app_version,
        "brand": contract.brand,
        "mobileType": contract.mobile_type,
        "sysVersions": contract.sys_version,
        "trackPoints": track_points,
        "distanceTimeStatus": "1",
        "innerSchool": "1",
        "runDistance": distance as i64,
        "runTime": preview.duration_seconds,
        "userId": info.user_id,
        "vocalStatus": "1",
        "yearSemester": info.semester_year,
        "recordDate": beijing_date(unix),
        "realityTrackPoints": format!("{}--", info.site_bound),
    }))
}

fn sanitize_message(message: &str) -> String {
    let cleaned: String = message
        .chars()
        .filter(|ch| !ch.is_control())
        .take(200)
        .collect();
    if cleaned.is_empty() {
        "未知错误".into()
    } else {
        cleaned
    }
}

fn parse_response(text: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("平台响应格式无效"))?;
    let code = value.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 10000 {
        let message = value
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("未知错误");
        bail!("平台返回错误：{}", sanitize_message(message));
    }
    Ok(value
        .get("response")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

/// 平台 HTTP 客户端；令牌仅保存在内存，不写入配置或日志。
pub struct PlatformClient {
    http: reqwest::blocking::Client,
    contract: RunningPlatformConfig,
    token: String,
}
impl PlatformClient {
    /// 校验契约完整性后创建客户端；未配置或字段不全时拒绝。
    pub fn new(contract: RunningPlatformConfig) -> Result<Self> {
        contract.require_complete()?;
        Self::build(contract)
    }
    fn build(contract: RunningPlatformConfig) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .cookie_store(true)
            .user_agent("okhttp/3.12.0")
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| anyhow::anyhow!("无法创建平台 HTTP 客户端"))?;
        Ok(Self {
            http,
            contract,
            token: String::new(),
        })
    }
    #[cfg(test)]
    fn new_unchecked(contract: RunningPlatformConfig) -> Result<Self> {
        Self::build(contract)
    }
    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.contract.base_url.trim().trim_end_matches('/'),
            path
        )
    }
    fn check_cancel(cancel: &AtomicBool) -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            bail!("平台任务已取消");
        }
        Ok(())
    }
    fn finish(
        response: reqwest::blocking::Response,
        cancel: &AtomicBool,
    ) -> Result<serde_json::Value> {
        Self::check_cancel(cancel)?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|_| anyhow::anyhow!("无法读取平台响应"))?;
        if !status.is_success() {
            bail!("平台服务返回异常状态（HTTP {}）", status.as_u16());
        }
        parse_response(&text)
    }
    fn post(&self, path: &str, body: &str, cancel: &AtomicBool) -> Result<serde_json::Value> {
        Self::check_cancel(cancel)?;
        let signature = sign(
            &BTreeMap::new(),
            Some(body),
            &self.contract.app_key,
            &self.contract.app_secret,
        );
        let response = self
            .http
            .post(self.url(path))
            .header("sign", signature)
            .header("token", &self.token)
            .header("appkey", &self.contract.app_key)
            .header("Content-Type", "application/json")
            .body(body.to_owned())
            .send()
            .map_err(|_| anyhow::anyhow!("平台网络请求失败，请检查网络连接"))?;
        Self::finish(response, cancel)
    }
    fn get(
        &self,
        path: &str,
        params: &BTreeMap<String, String>,
        cancel: &AtomicBool,
    ) -> Result<serde_json::Value> {
        Self::check_cancel(cancel)?;
        let signature = sign(
            params,
            None,
            &self.contract.app_key,
            &self.contract.app_secret,
        );
        let response = self
            .http
            .get(self.url(path))
            .header("sign", signature)
            .header("token", &self.token)
            .header("appkey", &self.contract.app_key)
            .header("Content-Type", "application/json; charset=UTF-8")
            .send()
            .map_err(|_| anyhow::anyhow!("平台网络请求失败，请检查网络连接"))?;
        Self::finish(response, cancel)
    }
    /// 登录并获取运动规则与学校范围；任一步骤失败则不保留会话。
    pub fn login(
        &mut self,
        phone: &str,
        password: &str,
        cancel: &AtomicBool,
    ) -> Result<PlatformLoginInfo> {
        if phone.len() != 11 || !phone.bytes().all(|byte| byte.is_ascii_digit()) {
            bail!("校园跑手机号必须为 11 位数字");
        }
        if password.is_empty() {
            bail!("校园跑密码未填写");
        }
        let body = serde_json::json!({
            "appVersion": self.contract.app_version,
            "brand": self.contract.brand,
            "deviceToken": self.contract.device_token,
            "deviceType": self.contract.device_type,
            "mobileType": self.contract.mobile_type,
            "password": md5_hex(password.as_bytes()),
            "sysVersion": self.contract.sys_version,
            "userPhone": phone,
        })
        .to_string();
        let user = self.post("v1/auth/login/password", &body, cancel)?;
        let token = user
            .pointer("/oauthToken/token")
            .and_then(|v| v.as_str())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| anyhow::anyhow!("平台登录响应缺少令牌"))?;
        self.token = token.to_owned();
        let mut user_id = user.get("userId").and_then(|v| v.as_i64()).unwrap_or(-1);
        if user_id < 0 {
            let queried = self.get("v1/auth/query/token", &BTreeMap::new(), cancel)?;
            user_id = queried.get("userId").and_then(|v| v.as_i64()).unwrap_or(-1);
        }
        if user_id < 0 {
            bail!("无法获取平台用户 ID");
        }
        let mut params = BTreeMap::new();
        params.insert("schoolId".to_owned(), self.contract.school_id.clone());
        let standard = self.get(
            &format!(
                "v1/unirun/query/runStandard?schoolId={}",
                self.contract.school_id
            ),
            &params,
            cancel,
        )?;
        let semester_year = standard
            .get("semesterYear")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let once_time_min = standard
            .get("boyOnceTimeMin")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32;
        let once_time_max = standard
            .get("boyOnceTimeMax")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32;
        let bounds = self.get(
            &format!(
                "v1/unirun/querySchoolBound?schoolId={}",
                self.contract.school_id
            ),
            &params,
            cancel,
        )?;
        let site_bound = bounds
            .as_array()
            .and_then(|list| list.first())
            .and_then(|bound| bound.get("siteBound"))
            .and_then(|v| v.as_str())
            .filter(|bound| !bound.is_empty())
            .ok_or_else(|| anyhow::anyhow!("平台未返回学校运动范围"))?;
        Ok(PlatformLoginInfo {
            user_id,
            semester_year,
            site_bound: site_bound.to_owned(),
            once_time_min,
            once_time_max,
        })
    }
    /// 提交运动记录；请求体由 record_body 生成。
    pub fn submit(
        &self,
        body: &serde_json::Value,
        cancel: &AtomicBool,
    ) -> Result<PlatformSubmitInfo> {
        let response = self.post("v1/unirun/save/run/record/new", &body.to_string(), cancel)?;
        let text_field = |name: &str| {
            response
                .get(name)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        Ok(PlatformSubmitInfo {
            record_id: response
                .get("recordId")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            result_status: text_field("resultStatus"),
            result_desc: text_field("resultDesc"),
            over_speed_warn: text_field("overSpeedWarn"),
            warn_content: text_field("warnContent"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::running::Point;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread,
    };

    const KEY: &str = "synthetic-key";
    const SECRET: &str = "synthetic-secret";

    #[test]
    fn md5_matches_known_vectors_including_utf8() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        assert_eq!(
            md5_hex("校园跑".as_bytes()),
            "908aba5c3353c41e56140c6361bbb549"
        );
        assert_eq!(
            md5_hex(b"synthetic-password"),
            "6ee7966bfda254813a6a475e009f6780"
        );
    }

    #[test]
    fn sign_matches_independently_computed_references() {
        let login_body = "{\"appVersion\":\"1.8.0\",\"brand\":\"SyntheticBrand\",\"deviceToken\":\"\",\"deviceType\":\"1\",\"mobileType\":\"SyntheticModel\",\"password\":\"6ee7966bfda254813a6a475e009f6780\",\"sysVersion\":\"14\",\"userPhone\":\"13800000000\"}";
        assert_eq!(
            sign(&BTreeMap::new(), Some(login_body), KEY, SECRET),
            "657806940926E99D2A5B718A99E03CB4"
        );
        let mut params = BTreeMap::new();
        params.insert("schoolId".to_owned(), "1000".to_owned());
        assert_eq!(
            sign(&params, None, KEY, SECRET),
            "A74F1A0C0C031A29CA6D4EE5A4A7861C"
        );
        assert_eq!(
            sign(&BTreeMap::new(), None, KEY, SECRET),
            "B36A864565056C5960E165D5B068DF15"
        );
        assert_eq!(
            sign(&BTreeMap::new(), Some("hello world!"), KEY, SECRET),
            "3B166CE07C4D2C69CEBD3DBF3B317429encodeutf8"
        );
        assert_eq!(
            sign(&BTreeMap::new(), Some("中!"), KEY, SECRET),
            "B0F9A67961F93D13BE8E17C522838DEEencodeutf8"
        );
    }

    #[test]
    fn gcj02_shifts_chengdu_point_and_keeps_foreign_points() {
        let (lon, lat) = wgs84_to_gcj02(103.98, 30.58);
        let shift = ((lon - 103.98) * 95832.0).hypot((lat - 30.58) * 111320.0);
        assert!((100.0..800.0).contains(&shift), "偏移量异常：{shift}");
        assert_eq!(wgs84_to_gcj02(2.35, 48.85), (2.35, 48.85));
        let (back_lon, back_lat) = (
            lon * 2.0 - wgs84_to_gcj02(lon, lat).0,
            lat * 2.0 - wgs84_to_gcj02(lon, lat).1,
        );
        let error = ((back_lon - 103.98) * 95832.0).hypot((back_lat - 30.58) * 111320.0);
        assert!(error < 5.0, "近似逆变换误差过大：{error}");
    }

    #[test]
    fn beijing_date_handles_midnight_boundary() {
        assert_eq!(beijing_date(0), "1970-01-01");
        assert_eq!(beijing_date(1_794_758_399), "2026-11-15");
        assert_eq!(beijing_date(1_794_758_400), "2026-11-16");
        assert_eq!(beijing_date(1_794_787_200), "2026-11-16");
    }

    fn sample_projection() -> Projection {
        Projection {
            lon0: 103.98,
            lat0: 30.58,
            sx: 95832.0,
            sy: 111320.0,
        }
    }
    fn sample_preview() -> Preview {
        Preview {
            points: vec![
                TrackPoint {
                    position: Point { x: 0.0, y: 0.0 },
                    elapsed_millis: 0,
                },
                TrackPoint {
                    position: Point { x: 100.0, y: 0.0 },
                    elapsed_millis: 300_000,
                },
                TrackPoint {
                    position: Point { x: 100.0, y: 100.0 },
                    elapsed_millis: 600_000,
                },
            ],
            reference: Vec::new(),
            distance_m: 200.0,
            duration_seconds: 600,
            seed: 42,
            map: None,
        }
    }

    #[test]
    fn track_points_roundtrip_to_local_coordinates_with_ordered_timestamps() {
        let projection = sample_projection();
        let end_ms = 1_794_787_200_000u64;
        let json =
            track_points_json(&sample_preview().points, 600, &projection, end_ms, 42).unwrap();
        let entries: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(entries.len(), 3);
        let mut previous = 0u64;
        for (index, entry) in entries.iter().enumerate() {
            let (location, rest) = entry.split_once('-').unwrap();
            let (timestamp, accuracy) = rest.split_once('-').unwrap();
            let timestamp: u64 = timestamp.parse().unwrap();
            let accuracy: f64 = accuracy.parse().unwrap();
            assert!(timestamp >= previous);
            previous = timestamp;
            assert!((0.0..10.0).contains(&accuracy));
            let (lon, lat) = location.split_once(',').unwrap();
            let (lon, lat) = (lon.parse::<f64>().unwrap(), lat.parse::<f64>().unwrap());
            let wgs_lon = lon * 2.0 - wgs84_to_gcj02(lon, lat).0;
            let wgs_lat = lat * 2.0 - wgs84_to_gcj02(lon, lat).1;
            let x = (wgs_lon - projection.lon0) * projection.sx;
            let y = (wgs_lat - projection.lat0) * projection.sy;
            let expected = sample_preview().points[index].position;
            assert!((x - expected.x).abs() < 2.0 && (y - expected.y).abs() < 2.0);
        }
        let first: u64 = entries[0].split('-').nth(1).unwrap().parse().unwrap();
        let last: u64 = entries[2].split('-').nth(1).unwrap().parse().unwrap();
        assert_eq!(first, end_ms - 600_000);
        assert_eq!(last, end_ms);
    }

    #[test]
    fn builtin_campuses_have_projection_anchors_and_unknown_ids_do_not() {
        assert!(campus_projection("demo-a").is_some());
        assert!(campus_projection("demo-b").is_some());
        assert!(campus_projection("external-file").is_none());
    }

    #[test]
    fn record_body_carries_session_fields_and_defaults() {
        let contract = RunningPlatformConfig {
            brand: "SyntheticBrand".into(),
            mobile_type: "SyntheticModel".into(),
            sys_version: "14".into(),
            ..Default::default()
        };
        let info = PlatformLoginInfo {
            user_id: 42,
            semester_year: "2026-2027-1".into(),
            site_bound: "104.0,30.0;104.1,30.1".into(),
            once_time_min: 600,
            once_time_max: 3600,
        };
        let body = record_body(
            &contract,
            &info,
            &sample_preview(),
            "[]",
            UNIX_EPOCH + Duration::from_secs(1_794_787_200),
        )
        .unwrap();
        assert_eq!(body["userId"], 42);
        assert_eq!(body["runDistance"], 200);
        assert_eq!(body["runTime"], 600);
        assert_eq!(body["yearSemester"], "2026-2027-1");
        assert_eq!(body["recordDate"], "2026-11-16");
        assert_eq!(body["realityTrackPoints"], "104.0,30.0;104.1,30.1--");
        assert_eq!(body["againRunStatus"], "0");
        assert_eq!(body["distanceTimeStatus"], "1");
        assert_eq!(body["innerSchool"], "1");
        assert_eq!(body["vocalStatus"], "1");
        assert_eq!(body["appVersions"], "1.8.0");
    }

    #[test]
    fn unconfigured_or_invalid_contract_is_rejected_before_network() {
        assert!(PlatformClient::new(RunningPlatformConfig::default()).is_err());
        let mut broken = RunningPlatformConfig {
            base_url: "http://platform.test".into(),
            app_key: KEY.into(),
            app_secret: SECRET.into(),
            school_id: "1000".into(),
            brand: "SyntheticBrand".into(),
            mobile_type: "SyntheticModel".into(),
            sys_version: "14".into(),
            ..Default::default()
        };
        assert!(PlatformClient::new(broken.clone()).is_err());
        broken.base_url = "https://platform.test".into();
        let mut incomplete = broken.clone();
        incomplete.brand.clear();
        assert!(
            PlatformClient::new(incomplete).is_err(),
            "缺设备信息应拒绝登录"
        );
        assert!(PlatformClient::new(broken).is_ok());
    }

    #[test]
    fn cancelled_login_never_touches_network() {
        let contract = RunningPlatformConfig {
            base_url: "https://platform.invalid".into(),
            app_key: KEY.into(),
            app_secret: SECRET.into(),
            school_id: "1000".into(),
            brand: "SyntheticBrand".into(),
            mobile_type: "SyntheticModel".into(),
            sys_version: "14".into(),
            ..Default::default()
        };
        let mut client = PlatformClient::new_unchecked(contract).unwrap();
        let error = client
            .login("13800000000", "synthetic-password", &AtomicBool::new(true))
            .err()
            .unwrap();
        assert!(error.to_string().contains("取消"));
    }

    struct MockRequest {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: String,
    }
    fn read_request(stream: &mut TcpStream) -> MockRequest {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte).unwrap();
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
            assert!(buf.len() < 1_048_576, "请求头过大");
        }
        let text = String::from_utf8(buf).unwrap();
        let head = text.split_once("\r\n\r\n").unwrap().0;
        let mut lines = head.lines();
        let mut request_line = lines.next().unwrap().split_whitespace();
        let method = request_line.next().unwrap().to_owned();
        let path = request_line.next().unwrap().to_owned();
        let mut headers = BTreeMap::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
            }
        }
        let length: usize = headers
            .get("content-length")
            .map(|value| value.parse().unwrap())
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).unwrap();
        MockRequest {
            method,
            path,
            headers,
            body: String::from_utf8(body).unwrap(),
        }
    }
    fn respond(stream: &mut TcpStream, json: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            json.len(),
            json
        );
        stream.write_all(response.as_bytes()).unwrap();
    }
    fn contract_for(addr: std::net::SocketAddr) -> RunningPlatformConfig {
        RunningPlatformConfig {
            base_url: format!("http://{addr}"),
            app_key: KEY.into(),
            app_secret: SECRET.into(),
            school_id: "1000".into(),
            brand: "SyntheticBrand".into(),
            mobile_type: "SyntheticModel".into(),
            sys_version: "14".into(),
            ..Default::default()
        }
    }

    #[test]
    fn login_and_submit_flow_matches_contract_against_synthetic_server() {
        const LOGIN_BODY: &str = "{\"appVersion\":\"1.8.0\",\"brand\":\"SyntheticBrand\",\"deviceToken\":\"\",\"deviceType\":\"1\",\"mobileType\":\"SyntheticModel\",\"password\":\"6ee7966bfda254813a6a475e009f6780\",\"sysVersion\":\"14\",\"userPhone\":\"13800000000\"}";
        const SITE_BOUND: &str = "104.0,30.0;104.1,30.0;104.1,30.1";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let problems = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_problems = problems.clone();
        let server = thread::spawn(move || {
            let check = |condition: bool, message: &str| {
                if !condition {
                    server_problems.lock().unwrap().push(message.to_owned());
                }
            };
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            check(request.method == "POST", "登录方法错误");
            check(request.path == "/v1/auth/login/password", "登录路径错误");
            check(request.body == LOGIN_BODY, "登录请求体与契约不符");
            check(
                request.headers.get("sign").map(String::as_str)
                    == Some("657806940926E99D2A5B718A99E03CB4"),
                "登录签名错误",
            );
            check(
                request.headers.get("appkey").map(String::as_str) == Some(KEY),
                "appkey 错误",
            );
            check(
                request.headers.get("token").map(String::as_str) == Some(""),
                "登录令牌应为空",
            );
            check(
                request.headers.get("content-type").map(String::as_str) == Some("application/json"),
                "登录 Content-Type 错误",
            );
            respond(
                &mut stream,
                "{\"code\":10000,\"msg\":\"ok\",\"response\":{\"userId\":42,\"oauthToken\":{\"token\":\"synthetic-token\",\"refreshToken\":\"synthetic-refresh\"}}}",
            );
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            check(request.method == "GET", "规则方法错误");
            check(
                request.path == "/v1/unirun/query/runStandard?schoolId=1000",
                "规则路径错误",
            );
            check(
                request.headers.get("sign").map(String::as_str)
                    == Some("A74F1A0C0C031A29CA6D4EE5A4A7861C"),
                "规则签名错误",
            );
            check(
                request.headers.get("token").map(String::as_str) == Some("synthetic-token"),
                "规则令牌错误",
            );
            respond(
                &mut stream,
                "{\"code\":10000,\"msg\":\"ok\",\"response\":{\"standardId\":1,\"schoolId\":1000,\"boyOnceTimeMin\":600,\"boyOnceTimeMax\":3600,\"semesterYear\":\"2026-2027-1\"}}",
            );
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            check(
                request.path == "/v1/unirun/querySchoolBound?schoolId=1000",
                "学校范围路径错误",
            );
            check(
                request.headers.get("sign").map(String::as_str)
                    == Some("A74F1A0C0C031A29CA6D4EE5A4A7861C"),
                "学校范围签名错误",
            );
            respond(
                &mut stream,
                "{\"code\":10000,\"msg\":\"ok\",\"response\":[{\"siteName\":\"合成校区\",\"siteBound\":\"104.0,30.0;104.1,30.0;104.1,30.1\",\"boundCenter\":\"104.05,30.05\"}]}",
            );
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            check(request.method == "POST", "提交方法错误");
            check(
                request.path == "/v1/unirun/save/run/record/new",
                "提交路径错误",
            );
            let expected_sign = sign(&BTreeMap::new(), Some(&request.body), KEY, SECRET);
            check(
                request.headers.get("sign").map(String::as_str) == Some(expected_sign.as_str()),
                "提交签名与请求体不一致",
            );
            check(
                request.headers.get("token").map(String::as_str) == Some("synthetic-token"),
                "提交令牌错误",
            );
            let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
            check(body["userId"] == 42, "提交用户 ID 错误");
            check(body["yearSemester"] == "2026-2027-1", "提交学期错误");
            check(
                body["realityTrackPoints"] == format!("{SITE_BOUND}--"),
                "提交学校范围错误",
            );
            check(body["runDistance"] == 200, "提交距离错误");
            check(body["runTime"] == 600, "提交时间错误");
            check(body["recordDate"] == "2026-11-16", "提交日期错误");
            let track: Vec<String> =
                serde_json::from_str(body["trackPoints"].as_str().unwrap()).unwrap();
            check(track.len() == 3, "提交轨迹点数错误");
            respond(
                &mut stream,
                "{\"code\":10000,\"msg\":\"ok\",\"response\":{\"recordId\":777,\"resultStatus\":\"1\",\"resultDesc\":\"合成成功\",\"overSpeedWarn\":\"0\",\"warnContent\":\"\"}}",
            );
        });
        let contract = contract_for(addr);
        let mut client = PlatformClient::new_unchecked(contract.clone()).unwrap();
        let info = client
            .login("13800000000", "synthetic-password", &AtomicBool::new(false))
            .unwrap();
        assert_eq!(info.user_id, 42);
        assert_eq!(info.semester_year, "2026-2027-1");
        assert_eq!(info.site_bound, SITE_BOUND);
        assert_eq!((info.once_time_min, info.once_time_max), (600, 3600));
        let preview = sample_preview();
        let track = track_points_json(
            &preview.points,
            preview.duration_seconds,
            &campus_projection("demo-a").unwrap(),
            1_794_787_200_000,
            preview.seed,
        )
        .unwrap();
        let body = record_body(
            &contract,
            &info,
            &preview,
            &track,
            UNIX_EPOCH + Duration::from_secs(1_794_787_200),
        )
        .unwrap();
        let result = client.submit(&body, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.record_id, 777);
        assert_eq!(result.result_status, "1");
        assert_eq!(result.result_desc, "合成成功");
        server.join().unwrap();
        let problems = problems.lock().unwrap();
        assert!(problems.is_empty(), "服务端契约校验失败：{problems:?}");
    }

    #[test]
    fn platform_error_code_surfaces_message_and_missing_token_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            respond(
                &mut stream,
                "{\"code\":10001,\"msg\":\"密码错误\",\"response\":null}",
            );
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            respond(
                &mut stream,
                "{\"code\":10000,\"msg\":\"ok\",\"response\":{\"userId\":42}}",
            );
        });
        let mut client = PlatformClient::new_unchecked(contract_for(addr)).unwrap();
        let error = client
            .login("13800000000", "synthetic-password", &AtomicBool::new(false))
            .err()
            .unwrap();
        assert!(error.to_string().contains("密码错误"));
        assert!(!error.to_string().contains("10001"));
        let error = client
            .login("13800000000", "synthetic-password", &AtomicBool::new(false))
            .err()
            .unwrap();
        assert!(error.to_string().contains("缺少令牌"));
        server.join().unwrap();
    }
}
