# Campus Net Guardian

使用 Rust 编写的 Windows 校园网认证守护工具。原生桌面界面、低内存占用，无需安装 Python 或浏览器运行环境。

## 功能

- **自动认证**：完成网关、CAS 和 eportal 门户认证，支持运营商选择。
- **断线重连**：定时检查连接状态，掉线后自动重试，并通过退避策略避免频繁请求。
- **备用网络**：校园网不可用时切换备用 WiFi，可选择自动尝试切回校园网。
- **托盘常驻**：关闭面板后隐藏到托盘，随时查看状态、立即登录或暂停 / 恢复守护。
- **同窗设置**：在原窗口内切换状态页与设置页，支持系统浅色 / 深色主题。
- **登录时自启**：通过当前用户的启动目录快捷方式启动，无需管理员权限或 Windows Service。
- **本地配置与日志**：账号保存在本机配置文件中，日志不输出密码、票据或会话原文。

## 使用

从 [GitHub Releases](https://github.com/Chenpi-Sakura/campus-net-guardian/releases) 下载 Windows 版本，解压后运行 `campus-guardian.exe`。

首次使用时，在设置页点击「打开配置文件」，填写校园网账号、门户地址和主网 SSID 后重启应用。主网 WiFi 需事先在 Windows 中保存连接配置。完整字段说明见 [配置模板](config.example.yaml)。

配置文件查找顺序为：启动参数 `--config <路径>`、当前目录、exe 所在目录、用户目录 `%LOCALAPPDATA%/campus-guardian/` 下的 `config.yaml`。没有配置文件时会自动创建模板。

点击窗口关闭按钮仅隐藏面板；彻底退出请使用托盘菜单并确认。

## 设置

| 设置项 | 用途 |
|---|---|
| 登录时自启 | 开启或关闭登录 Windows 后自动运行 |
| 备用网络 | 启用备用 WiFi，并填写名称与密码 |
| 切回校园网 | 选择是否定期尝试恢复主网 |
| 探测间隔 | 调整检查连接的频率 |
| 掉线判定 | 调整连续失败多少次后进行重连 |
| 调试日志 | 切换日常与调试日志 |

设置页的日常选项另存于用户目录的 `gui_settings.json`，不回写 YAML。账号、门户地址、主网 SSID、超时和退避策略仍由 `config.yaml` 管理。原有配置和 GUI 设置文件可以继续使用。

日志默认位于用户目录的 `guardian.log`，也可以在 YAML 中指定位置。配置与 GUI 覆盖文件可能含有敏感信息，请勿上传。

## 命令行

```powershell
.\campus-guardian.exe --gui
.\campus-guardian.exe --gui --hidden
.\campus-guardian.exe --config .\config.yaml --once
.\campus-guardian.exe --probe
.\campus-guardian.exe --check
.\campus-guardian.exe --enable-autostart
.\campus-guardian.exe --disable-autostart
```

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
