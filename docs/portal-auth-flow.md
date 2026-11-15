# 校园网门户认证链路（逆向实证版）

> 本文档基于 **2026-09-16 某高校校园网实网环境的完整抓包 + 实机验证**，
> 记录 `campus-net-guardian` 认证核心的工作原理。下述约 1 秒耗时为迁移前的历史实网观察，并非 Rust 新版的实测耗时。
>
> 与早期假设（如"密码必须加密""sessionId 就是 SESSION cookie"）相冲突处，
> **以本文档为准**。

---

## 1. 结论速览

| 项目 | 实测结论 |
|---|---|
| 认证是否必须浏览器 | **否**。requests 路径 1 秒内完成全链路 |
| 密码是否需要前端加密 | **否**。CAS POST 明文密码被服务端接受 |
| reCAPTCHA 是否硬性校验 | **否**。无 token 仍返回 302 + ticket |
| 真正的 sessionId | **网关拦截链产生的 12 位 hex `flowSessionId`** |
| CAS `SESSION` cookie 能否当 sessionId | **不能**。eportal 返回 `code=400 找不到用户` |
| 认证成功判据 | `getCurrentNode` 到 `finish` **且** `userOnline=true` **且** HTTPS 外网可达 |

当前实测一次完整认证耗时 **约 1 秒**（不含 WiFi 关联）。

---

## 2. 网络拓扑与前提

```
[笔记本]
  ├─ eth (Remote NDIS / 网线)   192.168.137.10   ← 备用通道
  └─ WLAN (<SSID>)           10.20.30.40   ← 认证目标（默认路由）
                                   gw 10.20.30.1
[校园网关 <NAS_MODEL>]
  └─ portal / eportal / cas-sso  <PORTAL_HOST>
```

**关键前提**：

1. **认证流量必须绑定 WLAN 源 IP**（当前实现通过 HTTP 客户端的本地地址绑定）。双网卡同时在线时，否则默认路由可能把
   portal 请求从 eth 发出，网关认不出 WiFi 会话。
   - **必须动态解析**：WiFi 切换 / DHCP 续租后 WLAN 的 IPv4 会变
     （实测从 `10.20.30.41` → `10.20.30.42`）。硬编码旧 IP 会导致
     `[WinError 10049] 地址无效`，认证全挂。
     Rust 版每次认证或探测重新解析 WLAN IPv4，并为 HTTP 客户端绑定源地址；没有地址时拒绝误走默认网卡。
2. **认证流量必须绕过系统代理**。Clash / V2Ray 等把请求从 `127.0.0.1:7897`
   转发，源 IP 与路由全乱，实测导致所有 eportal 接口 `code=400`。
   Rust HTTP 客户端统一禁用系统代理。
3. **HTTP 80 默认放行、HTTPS 443 未认证不放行**。这是校园网"部分放行"状态：
   HTTP 能出网但 HTTPS 超时，表现为"浏览器打不开网页"。

---

## 3. 完整认证流程（逐步实证）

### 步骤 0：触发网关拦截，获取真正的 flowSessionId ★核心

未完全认证时，网关对**非白名单 HTTP 站点**做劫持：

```http
GET http://www.sina.com.cn/ HTTP/1.1
```

返回 `200` + 一段 JS 跳转页：

```html
<html><head>
<script type="text/javascript">
location.href="http://<PORTAL_HOST>/eportal/index.jsp
  ?wlanuserip=10.20.30.40&wlanacname=<NAS_MODEL>&mac=aabbccddeeff
  &url=http://www.sina.com.cn/"
</script>
</head>
<body>Authentication is required. Click here ...</body></html>
```

跟进该 `index.jsp`：

```http
GET http://<PORTAL_HOST>/eportal/index.jsp?wlanuserip=...&wlanacname=...&mac=...&url=...
```

返回 `302`：

```http
HTTP/1.1 302
Set-Cookie: JSESSIONID=__JSESSIONID__; Path=/eportal; HttpOnly
Location: http://<PORTAL_HOST>/portal/portal-main
  ?sessionId=<FLOW_SESSION_ID>   ← ★ 12 位 hex，真正的 eportal sessionId
  &userIp=10.20.30.40
  &userMac=aabbccddeeff
  &nasIp=<NAS_IP>
  &customPageId=<PAGE_ID>
  &redirectUrl=http://www.sina.com.cn/
  &samPortalRedirectUrl=http://123.123.123.123
  &terminalType=pc&accessTime=...
```

> **实测**：该 `sessionId` 即 HAR 中出现的 `flowSessionId`（同格式、同角色）。
>
> **重要**：eportal 接口**纯凭 sessionId 识别用户，与 cookie 无关**——
> 新 Session 不带任何 cookie 调 `getCurrentNode` 仍返回 `code=200`。

**为什么不能直接访问 portal 拿 sessionId？**
portal 的入口页在网关白名单内，不触发拦截，其 `Set-Cookie: SESSION=<uuid>`
是 **cas-sso 的会话**，与 eportal 的 flowSessionId 无关（用它调 eportal 必 400）。

### 步骤 1：CAS 登录

```http
GET  /cas-sso/login?flowSessionId=<sid>&...        → 取 execution token + croypto
POST /cas-sso/login  (username + password + execution + flowSessionId)
                                                    → 302 Location: /portal/assets/auth-success.html?ticket=ST-xxxx
GET  /portal/assets/auth-success.html?ticket=...   → 200
```

- 用 `flowSessionId=<sid>` 串联 CAS 与 eportal 会话。
- 实测明文密码被接受（依照门户契约直接提交）。

### 步骤 2：eportal 工作流（真正拿网络权限）

```
POST /eportal/workFlow/getCurrentNode   {"sessionId": <sid>, "flowKey": "portal_auth"}
    → code=200, currentNodePath="serviceSelection"（或 "finish"=已在线）
POST /eportal/network/serviceSelection  {"sessionId": <sid>}
    → code=200, data=[{value:"cmcc"},{value:"ctcc"},{value:"unicom"}]
POST /eportal/network/serviceLogin      {"sessionId": <sid>, "service": "cmcc"}
    → code=200, data.authResult="success"    ← 运营商登录成功
POST /eportal/workFlow/getCurrentNode   （再次）
    → code=200, currentNodePath="finish"     ← 工作流完成
```

### 步骤 3：双重验证

```
POST /eportal/network/userOnline  {"sessionId": <sid>}  → portal_online=True
GET  https://www.baidu.com/ （直连，绕过代理）           → 200 + 真实内容 → internet=True
```

必须两者皆真才算认证成功：
- 仅 `userOnline=true` 不足以证明外网通（portal 状态可能滞后）；
- HTTP 会被 captive portal 假 200 欺骗，**必须用 HTTPS + 内容长度/关键词**验证。

---

## 4. 报错对照表

| 现象 | 根因 | 处置 |
|---|---|---|
| eportal `code=400 没有查找到当前sessionId对应的用户信息` | sessionId 用了 SESSION cookie / 拦截链未走 | 走步骤 0 拿 12 位 hex flowSessionId |
| eportal 请求超时 / 连接失败 | 流量走了 eth 或系统代理 | 绑源 IP + `trust_env=False` |
| HTTP 通、HTTPS 全超时 | 未认证的"部分放行"状态 | 正常，继续完成 eportal 工作流 |
| `未检测到网关拦截` | 探测站被放行 / WiFi 已在线 | 换一个非白名单 HTTP 探测站（`portal.probe_url`） |
| CAS 302 但 eportal 仍 400 | flowSessionId 未贯穿 CAS | 确认 CAS 用 `flowSessionId=<sid>` |

---

## 5. 代码映射

当前实现位于 `native/src/`：`auth.rs` 处理网关拦截、CAS 表单、票据回调、eportal 工作流与双重在线验证；`platform.rs` 通过 Windows WLAN / IP Helper API 解析当前无线源 IPv4；`daemon.rs` 串行调度探测、认证、退避与网络切换。

HTTP 客户端不使用系统代理，凭据 POST 不自动重定向，门户跳转必须匹配配置 origin。新版仓库只保留 Rust 实现。

---

## 6. 运维注意事项

1. **探测站 `probe_url` 需为非白名单 HTTP 站**。当前用 `http://www.sina.com.cn/`。
   若某天拿不到拦截页，换任意外网 HTTP 站（如 163 / qq / 一个自建 HTTP）。
2. **系统代理需对认证流量关闭**。代码已内建绕过；
   若使用 TUN 模式代理，需将门户地址与探测流量设为直连规则。