# CUIT Helper

使用 Rust 和 Iced 编写的 Windows 校园网认证守护工具。解压即可运行，提供可调整大小的桌面界面与托盘守护。

## 功能

- **自动认证**：完成网关、CAS 和 eportal 门户认证，支持运营商选择。
- **断线重连**：定时检查连接状态，掉线后自动重试，并通过退避策略避免频繁请求。
- **备用网络**：校园网不可用时切换备用 WiFi，可选择自动尝试切回校园网。
- **托盘常驻**：关闭面板后隐藏到托盘，随时查看状态、立即登录或暂停 / 恢复守护。
- **同窗设置**：在原窗口内切换状态页与设置页，自动保存有效修改；提供 Claude / ChatGPT 风格，可跟随系统或手动选择浅色 / 深色。
- **登录时自启**：通过当前用户的启动目录快捷方式启动，无需管理员权限或 Windows Service。
- **本地配置与日志**：账号保存在本机配置文件中，日志不输出密码、票据或会话原文。

## 使用

从 [GitHub Releases](https://github.com/Chenpi-Sakura/campus-net-guardian/releases) 下载 Windows 版本，解压后运行 `CUIT Helper.exe`。

首次使用时，在设置页「主网络」中填写主 WiFi 名称、校园账号和密码并选择运营商，展开「门户参数」设置认证入口、认证接口和连通性探测地址。修改自动保存至本机 YAML 配置，重启后生效。主网 WiFi 需事先在 Windows 中连接并保存。完整字段说明见 [配置模板](config.example.yaml)。

配置文件查找顺序为：启动参数 `--config <路径>`、当前目录、exe 所在目录、用户目录 `%LOCALAPPDATA%/campus-guardian/` 下的 `config.yaml`。没有配置文件时会自动创建模板。

点击窗口关闭按钮仅隐藏面板；彻底退出请使用托盘菜单并确认。

## 设置

| 设置项 | 用途 |
|---|---|
| 主网络 | 填写主 WiFi、校园账号和密码，选择运营商 |
| 门户参数 | 展开后设置认证入口、认证接口和连通性探测地址 |
| 登录时自启 | 开启或关闭登录 Windows 后自动运行 |
| 备用网络 | 启用备用 WiFi，并填写名称与密码 |
| 切回校园网 | 选择是否定期尝试恢复主网 |
| 探测间隔 | 调整检查连接的频率 |
| 掉线判定 | 调整连续失败多少次后进行重连 |
| 调试日志 | 切换日常与调试日志 |
| 外观 | 选择设计风格与明暗模式，自定义强调色、背景色、面板色和文字色 |

设置页自动保存有效修改，无需点击保存。主网络项写入本机 YAML，重启后生效；GUI 偏好另存于用户目录的 `gui_settings.json`。文字输入停顿后保存，不完整或无效设置会提示原因并保留原值。超时、退避等高级参数仍通过「打开配置文件」管理。原有配置和 GUI 设置文件可以继续使用。

每种设计风格的浅色与深色配色独立保存；自定义颜色可即时预览，也可恢复当前主题的默认配色。运行时图标与控件强调色保持一致。

日志默认位于用户目录的 `guardian.log`，也可以在 YAML 中指定位置。配置与 GUI 覆盖文件可能含有敏感信息，请勿上传。

## 命令行

```powershell
& '.\CUIT Helper.exe' --gui
& '.\CUIT Helper.exe' --gui --hidden
& '.\CUIT Helper.exe' --preview
& '.\CUIT Helper.exe' --config .\config.yaml --once
& '.\CUIT Helper.exe' --probe
& '.\CUIT Helper.exe' --check
& '.\CUIT Helper.exe' --enable-autostart
& '.\CUIT Helper.exe' --disable-autostart
```

`--preview` 使用合成状态预览界面，不读取真实配置、不认证或切换 WiFi，也不保存设置或修改自启。

认证流量会绕过系统代理并动态绑定 WLAN 地址；指定校园网 SSID 却没有可用无线地址时，会停止认证以避免误走其他网卡。

## 开发

使用 Rust stable。Windows 可搭配 MSVC 工具链，或使用带 MinGW 的 GNU 工具链。

```powershell
cargo build --release --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

应用代码位于 `native/src/`，图标位于 `assets/`，发布与许可证工具位于 `scripts/`。认证协议说明见 [门户认证流程](docs/portal-auth-flow.md)。

本项目采用 [MIT License](LICENSE)。
