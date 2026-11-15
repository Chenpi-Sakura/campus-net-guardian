# 动画组件补丁

基于 crates.io `iced_anim 0.3.1`，使用其 MIT 许可。控件与动画实现保持上游代码。

仅将 `widgets` 隐式启用的 SVG 支持拆成显式 `svg` feature；本项目只使用按钮及标量动画，不需要 SVG 解码链及其额外字体解析依赖。恢复该 feature 时需要重新审计依赖。

上游提供独立 feature 后，删除此目录及工作区的对应 patch，使用正式版本。
