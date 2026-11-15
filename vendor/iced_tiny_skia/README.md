# 本地兼容补丁

基于 crates.io 官方 `iced_tiny_skia 0.14.1`，原始源码提交 `0ecf60664df7b8ac7d7aef5f7279d5323027f693`，crate 校验和 `c267596d742714b1853cc10c3983a367762816fc4836bd3b79f76ce76787d6f8`。保留原有依赖与 MIT 许可，仅维护以下五个补丁：

- [iced #3384](https://github.com/iced-rs/iced/pull/3384)：在本 crate 的损伤计算中修正 Cached 文字垂直锚点，并以 layout 宽度保守覆盖水平 Center/Right 锚点，不修改共享 `iced_graphics` 的段落逻辑。
- [iced #3486](https://github.com/iced-rs/iced/pull/3486)：文字损伤边界增加 1 个逻辑像素，覆盖小数坐标和抗锯齿边缘。
- [iced #3368](https://github.com/iced-rs/iced/issues/3368)：同一 draw 内复用相同裁剪遮罩；缓存归 Engine 所有，每次 Renderer::draw 开始清空，避免跨帧、跨窗口或截图误用旧遮罩。
- 本项目离屏像素回归：Cached 文字始终使用 layer 与文字局部 viewport 的交集遮罩，避免把 viewport 自身的包含关系当成字形完全可见，修复滚动后菜单标签越过内容区域的问题。
- 官方源码 `src/engine.rs::draw_quad` 与 `src/layer.rs::Layer::damage` 的本地阴影修复：损伤与裁剪范围包含 blur/offset 及抗锯齿边缘，阴影使用当前 damage/layer 遮罩，signed 像素坐标保留负偏移；防止局部重绘叠黑及菜单移动/关闭后的残影。

升级到包含等价修复的官方版本并通过本地回归测试与 Windows 滚动截图验证后，移除目录及工作区的 patch/exclude 配置。
