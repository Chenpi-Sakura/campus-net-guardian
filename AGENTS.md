# Campus Net Guardian — 项目规范

## 目标与架构

- 当前交付形态是 Windows Rust 原生桌面应用：托盘常驻、紧凑面板、登录时自启。不得新增 Windows Service、管理员权限依赖、浏览器认证路径或本地管理端口。
- `native/src/auth.rs` 实现 HTTP 认证，`daemon.rs` 串行执行守护 / 手动登录 / WiFi 切换，`platform.rs` 封装 Windows WLAN 与用户启动目录，`ui.rs` 通过消息与后台通信，不在 UI 线程联网或等待 WiFi。
- 仓库与发布包仅包含 Rust 实现，不保留 Python 源码或脚本。旧版性能对比材料仅可存在本地忽略目录，不提交或上传。
- 配置兼容根目录 `config.example.yaml` 的 YAML；GUI 仅持久化白名单覆盖项到用户目录 `gui_settings.json`，不得回写 YAML 或把账号 / 门户地址放入 GUI 覆盖层。
- 原生图标使用 `logo.png` 和 `assets/app.ico`，主色 `#037EFE`；PNG 与 ICO 同源，按真实 DPI 载入标题栏 / 任务栏 / 托盘，不放大小位图或膨胀透明通道。
- 面板显示状态、最近登录、下次探测和当前 SSID；高频动作仅立即登录、暂停 / 恢复，详细配置放同一窗口内的设置页，点击返回回到状态页，不另开窗口。关闭隐藏到托盘，退出需确认。浅色 / 深色遵循系统设置。

## 隐私与安全

- 真实账密、校园名、SSID、MAC、门户 / NAS / 用户 IP、sessionId、ticket、Cookie 不得进入代码、文档、测试、提交消息或 Release。真实值仅保留本地 gitignored 配置 / GUI 设置。
- 测试全部使用合成值，不从真实环境复制。原始 HAR 必须先脱敏，不得直接提交。
- HTTP 禁用系统代理；认证每次动态绑定 WLAN IPv4。凭据只提交到配置的 CAS origin；禁止向不可信重定向目标转发凭据，禁止日志打印原始 HTTP 响应 / 完整 URL / 会话。
- WLAN profile 仅写当前用户，密码不落临时 XML；自启仅操作用户启动目录快捷方式。
- 内存测量使用显式合成模式，不读真实账密、不联网、不更改 WiFi / 自启 / 用户设置。报告注明场景、采样方式与限制，不编造运行结果。
- 新增依赖先确认维护状况、兼容 license 与安全公告；提交 Cargo.lock，发布前运行隐私扫描与依赖审计。

## 工程与验证

- 改动保持范围明确，遵循既有模块边界；公开 API 使用中文文档注释。异常必须有可见失败反馈，不静默吞错或以默认成功掩盖故障。
- 配置范围需验证；后台网络操作串行，暂停和退出可响应，退出取消后续认证步骤，重试计数仅在真实恢复后清零，时间间隔使用单调时钟。
- 必须通过 `cargo fmt --all -- --check`、`cargo test --workspace --locked`、`cargo clippy --workspace --all-targets --locked -- -D warnings` 和 release 构建。认证 / 状态机修复补回归测试。UI 需实际截图检查，跨 DPI 资源要验证。
- 认证、并发、Windows FFI 等高风险改动优先独立审查实际 diff，修复阻断问题后复验。审查者默认只读，主 Agent 负责修复和验收。
- 不自动编写进度、交接或重复代码真相的文档。维护具有持续价值的配置 / 协议 / 操作契约；长说明使用中文。

## Git 与发布

- 主分支 `main`，开发必须在临时分支 `<type>-yymmdd-<desc>`，不得直接在 main 开发提交；同一任务使用现有分支闭环。
- 合并前展示并审查实际 `git diff main..<branch>`，必要验证通过后 squash 合并；主分支一个完整任务一个 commit，格式 `<type>(<scope>): <summary>`；保留开发分支。
- 不覆盖已有未提交修改、不执行强制推送 / hard reset / clean、不改写共享历史。
- GitHub 交付目标 `https://github.com/Chenpi-Sakura/campus-net-guardian.git`；保留已配置 GitCode 远端。推送前扫描工作树和全部可达历史的已知真实敏感值。
- Release 只包含经测试的 Windows exe、配置模板、README、LICENSE 和校验和，不包含本地配置、日志、HAR 或构建缓存。仅在明确授权发布时发布；区分实现、测试、本地合并、远端推送与公开 Release 的实际状态。
