# Campus Net Guardian

Windows 校园网认证守护工具。Rust 原生桌面版，无 Python、WebView 或管理员权限依赖。

- 校园网门户认证：网关拦截链 → CAS → eportal → 运营商选择，认证与 HTTPS 外网检查同时通过才算在线。
- 托盘常驻、状态面板与同窗设置页、立即登录、暂停 / 恢复、登录 Windows 时自启。
- 主网不可用时切换备用 WiFi，支持按实际经过的秒数定期尝试恢复校园网。
- 原生 Windows WLAN API、GDI 绘制与系统控件，隐藏到托盘后不保留浏览器进程。
- 图标主色 `#037EFE`，标题栏、托盘和 exe 均包含 16–256 像素独立图像。

## 使用

从 [GitHub Releases](https://github.com/Chenpi-Sakura/campus-net-guardian/releases) 下载 Windows x64 包，解压后运行 `campus-guardian.exe`。不需要安装 Python。

配置文件按以下顺序寻找：显式 `--config <路径>`、当前目录 `config.yaml`、exe 同目录 `config.yaml`、`%LOCALAPPDATA%/campus-guardian/config.yaml`。首次使用会在用户目录创建模板；在设置中打开配置文件，填写本地账号、门户地址和主网 SSID 后重启。主网 WiFi 需已在 Windows 保存连接配置。

关闭面板会隐藏到托盘，退出走托盘菜单并确认。登录时自启仅创建当前用户启动目录快捷方式，不注册系统服务。

### 配置

`config.example.yaml` 提供完整契约；原有 Python 版 `config.yaml` 和 GUI 设置覆盖文件可以继续使用。

| 设置窗口 | 取值 |
|---|---|
| 登录时自启 | 开 / 关 |
| 备用网络 | 开 / 关、SSID 与密码 |
| 切回校园网 | 不自动 / 每 5 分钟 / 每 15 分钟 |
| 探测间隔 | 15 / 30 / 60 / 120 秒 |
| 掉线判定 | 连续失败 2 / 3 / 5 次 |
| 调试日志 | INFO / DEBUG |

日常选项存入 `%LOCALAPPDATA%/campus-guardian/gui_settings.json`，不回写 YAML。账号、门户地址、主网 SSID、超时与退避策略仍在 YAML 配置。环境变量通过 `env_overrides` 显式映射，支持账号、密码、运营商和日志级别；`CG_BIND_INTERFACE_IP` 可显式指定源 IPv4。

认证流量绕过系统代理，并在每次操作时重新绑定 WLAN IPv4；配置主网 SSID 却没有可用 WLAN 地址时会停止认证，防止误走其他网卡。

日志默认保存在 `%LOCALAPPDATA%/campus-guardian/guardian.log`，也支持 YAML 指定日志位置、大小与备份数量。日志只记录中性状态，不输出密码、票据、MAC、门户 URL 或会话原文。配置和 GUI 覆盖文件含有本地敏感信息，请勿上传。

### 命令行

```powershell
.\campus-guardian.exe --gui
.\campus-guardian.exe --gui --hidden
.\campus-guardian.exe --config .\config.yaml --once
.\campus-guardian.exe --probe
.\campus-guardian.exe --check
.\campus-guardian.exe --enable-autostart
.\campus-guardian.exe --disable-autostart
```

旧版 Windows Service 安装 / 卸载参数不再由新版支持。新版不注册服务，也不包含 Python 代码。

## 开发

需要 Rust stable；Windows 可使用 MSVC 工具链，或搭配 MinGW 的 GNU 工具链。

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --locked
```

```
native/src/   配置、认证、Windows 平台适配、后台状态机、原生 UI
assets/       Windows 图标资源
scripts/      图标生成、性能测量、隐私检查与发布
docs/portal-auth-flow.md  认证协议契约
```

核心认证测试使用本地合成 HTTP 服务，状态机使用假网络后端，不访问真实校园网。离线测试不能替代特定校园网环境中的实机验收。

## 内存对比

```powershell
.\scripts\measure_memory.ps1
```

对比时需通过脚本参数提供外部旧版基线程序；新版仓库不附带 Python 代码。测量旧 Python 与 Rust 的可见面板、托盘两种场景，默认各重复 3 次、预热 10 秒后采样 20 次，记录整个进程树的 Working Set 和 Private Bytes，中位数和峰值保存到 `output/memory-comparison.json`。两端均使用合成在线状态，不读取真实账密或切换网络；结果表示界面常驻基线，不代表真实认证过程峰值。

## 发布

```powershell
.\scripts\release.ps1 -PackageOnly
# 已验证、提交、创建并推送对应 tag 后：
.\scripts\release.ps1 -NoBuild -Publish -NotesFile <发布说明文件>
```

发布脚本只打包 exe、配置模板、README 和 LICENSE，并生成 SHA256；本地配置、GUI 设置、日志与 HAR 不在白名单中。GitHub Actions 对 tag 执行格式、测试、Clippy、构建与打包验证。

本项目采用 MIT License。
