//! 仅在目标变化时请求重绘的短交互动画。

use super::Message;
use iced::{
    Border, Color, Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    widget::{button, space},
};
use iced_anim::{Animated, transition::Easing};
use std::{
    cell::Cell,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT_ANCHOR: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq)]
struct Anchor {
    id: u64,
    bounds: Rectangle,
    revision: u64,
}

thread_local! {
    static POPUP_ANCHOR: Cell<Option<Anchor>> = const { Cell::new(None) };
    static POPUP_ABOVE: Cell<bool> = const { Cell::new(false) };
    static CLOSE_BROADCAST: Cell<bool> = const { Cell::new(false) };
}

/// 当前原生菜单是否由可用空间自动放置在控件上方。
pub(super) fn popup_above() -> bool {
    POPUP_ABOVE.with(Cell::get)
}

fn mix(base: Color, target: Color, progress: f32) -> Color {
    Color::from_rgba(
        base.r + (target.r - base.r) * progress,
        base.g + (target.g - base.g) * progress,
        base.b + (target.b - base.b) * progress,
        base.a + (target.a - base.a) * progress,
    )
}

/// 用原生按钮事件配合短过渡呈现开关，不创建常驻动画计时器。
pub(super) fn animated_switch<'a>(
    enabled: bool,
    on_toggle: fn(bool) -> Message,
) -> Element<'a, Message> {
    let control = button(space().width(36).height(20))
        .on_press(on_toggle(!enabled))
        .padding(0)
        .width(36)
        .height(20)
        .style(|theme: &Theme, status| button::Style {
            background: None,
            border: Border {
                color: Color {
                    a: 0.25,
                    ..theme.palette().primary
                },
                width: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    1.0
                } else {
                    0.0
                },
                radius: 10.0.into(),
            },
            ..Default::default()
        })
        .into();
    Element::new(Switch { enabled, control })
}

struct Switch<'a> {
    enabled: bool,
    control: Element<'a, Message>,
}

struct SwitchState {
    progress: Animated<f32>,
}

impl SwitchState {
    fn new(enabled: bool) -> Self {
        Self {
            progress: transition(if enabled { 1.0 } else { 0.0 }, 140),
        }
    }

    fn retarget(&mut self, enabled: bool) {
        let target = if enabled { 1.0 } else { 0.0 };
        if *self.progress.target() != target {
            self.progress.set_target(target);
        }
    }

    fn value(&self) -> f32 {
        self.progress.value().clamp(0.0, 1.0)
    }
}

impl Widget<Message, Theme, Renderer> for Switch<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(36.0), Length::Fixed(20.0))
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SwitchState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SwitchState::new(self.enabled))
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.control)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.state
            .downcast_mut::<SwitchState>()
            .retarget(self.enabled);
        tree.diff_children(std::slice::from_ref(&self.control));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self
            .control
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child])
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<SwitchState>();
            if state.progress.is_animating() {
                state.progress.tick(*now);
                if state.progress.is_animating() {
                    shell.request_redraw();
                }
            }
        }
        self.control.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        if bounds.intersection(viewport).is_none() {
            return;
        }
        let progress = tree.state.downcast_ref::<SwitchState>().value();
        let palette = theme.palette();
        let off = mix(
            theme.extended_palette().background.weak.color,
            palette.text,
            0.16,
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    radius: 10.0.into(),
                    ..Default::default()
                },
                snap: false,
                ..Default::default()
            },
            mix(off, palette.primary, progress),
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x + 2.0 + 16.0 * progress,
                    y: bounds.y + 2.0,
                    width: 16.0,
                    height: 16.0,
                },
                border: Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                snap: false,
                ..Default::default()
            },
            mix(
                palette.text,
                theme.extended_palette().primary.base.text,
                progress,
            ),
        );
        self.control.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.children().next().unwrap(),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.control.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.control.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }
}

enum Kind {
    Page { key: u8, theme: Theme },
    Reveal(bool),
    Anchor(u64),
}

struct Flow<'a> {
    kind: Kind,
    content: Element<'a, Message>,
}

struct State {
    key: u8,
    progress: Animated<f32>,
    theme: Option<Animated<Theme>>,
    overlay_present: bool,
    overlay_progress: Animated<f32>,
    overlay_pointer_stale: bool,
    overlay_geometry: PopupGeometry,
    anchor_id: u64,
    overlay_anchor_id: Option<u64>,
    closing: Option<Closing>,
}

#[derive(Default)]
struct PopupGeometry {
    clip: Rectangle,
    above: bool,
    menu: Rectangle,
    viewport: Size,
}

// 仅缓存有界指针交互，不保存输入文字、原生 Overlay 引用或可打印的凭据。
struct PointerEvent {
    event: Event,
    cursor: mouse::Cursor,
}
enum CloseIntent {
    Dismiss,
    Select,
    Outside(PointerEvent),
}
struct Closing {
    intent: CloseIntent,
    release: Option<PointerEvent>,
    anchor: Anchor,
    menu: Rectangle,
    viewport: Size,
    ready: bool,
}

impl Closing {
    fn cancel(&mut self) {
        self.intent = CloseIntent::Dismiss;
        self.release = None;
    }
}

fn without_capture(shell: &mut Shell<'_, Message>, deliver: impl FnOnce(&mut Shell<'_, Message>)) {
    let mut messages = Vec::new();
    {
        let mut local = Shell::new(&mut messages);
        deliver(&mut local);
        shell.request_redraw_at(local.redraw_request());
        shell.request_input_method(local.input_method());
        if local.is_layout_invalid() {
            shell.invalidate_layout();
        }
        if local.are_widgets_invalid() {
            shell.invalidate_widgets();
        }
    }
    for message in messages {
        shell.publish(message);
    }
}

fn pointer_press(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(iced::touch::Event::FingerPressed { .. })
    )
}

fn matching_release(press: &Event, release: &Event) -> bool {
    match (press, release) {
        (
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ) => true,
        (
            Event::Touch(iced::touch::Event::FingerPressed { id, .. }),
            Event::Touch(
                iced::touch::Event::FingerLifted { id: released, .. }
                | iced::touch::Event::FingerLost { id: released, .. },
            ),
        ) => id == released,
        _ => false,
    }
}

fn escape(event: &Event) -> bool {
    matches!(
        event,
        Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            ..
        })
    )
}

fn cancel_window_event(event: &Event) -> bool {
    matches!(
        event,
        Event::Window(
            iced::window::Event::Unfocused
                | iced::window::Event::CloseRequested
                | iced::window::Event::Resized(_)
                | iced::window::Event::Rescaled(_)
        )
    )
}

impl State {
    fn open_overlay(&mut self, anchor_id: u64) {
        if !self.overlay_present || self.overlay_anchor_id != Some(anchor_id) {
            self.overlay_present = true;
            self.overlay_anchor_id = Some(anchor_id);
            self.overlay_progress = transition(0.0, 120).to(1.0);
            self.overlay_pointer_stale = true;
            self.overlay_geometry = PopupGeometry::default();
            self.closing = None;
        }
    }
}

fn transition<T: iced_anim::Animate>(value: T, millis: u64) -> Animated<T> {
    Animated::transition(
        value,
        Easing::EASE_OUT.with_duration(Duration::from_millis(millis)),
    )
}

/// 切页仅淡入颜色，并在同一子树内过渡主题，不移动内容或重建内层动画。
pub(super) fn page<'a>(
    key: u8,
    theme: Theme,
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    Element::new(Flow {
        kind: Kind::Page { key, theme },
        content,
    })
}

/// 以内容实际测得的高度展开或收起，宽度和换行变化时无需固定高度。
pub(super) fn reveal<'a>(
    expanded: bool,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(Flow {
        kind: Kind::Reveal(expanded),
        content: content.into(),
    })
}

/// 为原生弹层记录控件位置，保留原生 PickList 的布局、事件与自动上下定位。
pub(super) fn popup_anchor<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    popup_anchor_revision(content, 0)
}

/// 动态选项列表变化时取消延迟选择，避免同数量替换后提交旧索引。
pub(super) fn popup_anchor_revision<'a>(
    content: impl Into<Element<'a, Message>>,
    revision: u64,
) -> Element<'a, Message> {
    Element::new(Flow {
        kind: Kind::Anchor(revision),
        content: content.into(),
    })
}

fn fade_theme(theme: &Theme, progress: f32) -> Theme {
    let amount = progress.clamp(0.0, 1.0);
    let mut palette = theme.palette();
    let background = palette.background;
    for color in [
        &mut palette.text,
        &mut palette.primary,
        &mut palette.success,
        &mut palette.warning,
        &mut palette.danger,
    ] {
        *color = mix(background, *color, amount);
    }
    let mut extended = *theme.extended_palette();
    for pair in [
        &mut extended.background.base,
        &mut extended.background.weakest,
        &mut extended.background.weaker,
        &mut extended.background.weak,
        &mut extended.background.neutral,
        &mut extended.background.strong,
        &mut extended.background.stronger,
        &mut extended.background.strongest,
        &mut extended.primary.base,
        &mut extended.primary.weak,
        &mut extended.primary.strong,
        &mut extended.secondary.base,
        &mut extended.secondary.weak,
        &mut extended.secondary.strong,
        &mut extended.success.base,
        &mut extended.success.weak,
        &mut extended.success.strong,
        &mut extended.warning.base,
        &mut extended.warning.weak,
        &mut extended.warning.strong,
        &mut extended.danger.base,
        &mut extended.danger.weak,
        &mut extended.danger.strong,
    ] {
        pair.color = mix(background, pair.color, amount);
        pair.text = mix(background, pair.text, amount);
    }
    Theme::custom_with_fn("Page fade", palette, move |_| extended)
}

impl Flow<'_> {
    fn interactive(&self) -> bool {
        !matches!(self.kind, Kind::Reveal(false))
    }

    #[allow(clippy::too_many_arguments)]
    fn close_children(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        struct Guard(bool);
        impl Drop for Guard {
            fn drop(&mut self) {
                CLOSE_BROADCAST.with(|flag| flag.set(self.0));
            }
        }
        let _guard = Guard(CLOSE_BROADCAST.with(|flag| flag.replace(true)));
        // 第一遍也会退出原生中键自动滚动；第二遍保证关闭事件到达所有 PickList。
        for _ in 0..2 {
            without_capture(shell, |local| {
                self.content.as_widget_mut().update(
                    &mut tree.children[0],
                    &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                    layout.children().next().unwrap(),
                    mouse::Cursor::Unavailable,
                    renderer,
                    clipboard,
                    local,
                    viewport,
                )
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn replay_pointer(
        &mut self,
        pointer: PointerEvent,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        without_capture(shell, |local| {
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                &pointer.event,
                layout.children().next().unwrap(),
                pointer.cursor,
                renderer,
                clipboard,
                local,
                viewport,
            )
        });
    }
}

impl Widget<Message, Theme, Renderer> for Flow<'_> {
    fn size(&self) -> Size<Length> {
        let size = self.content.as_widget().size();
        match self.kind {
            Kind::Page { .. } | Kind::Anchor(_) => size,
            Kind::Reveal(_) => Size::new(size.width, Length::Shrink),
        }
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        let (key, progress, theme) = match &self.kind {
            Kind::Page { key, theme } => (*key, 1.0, Some(transition(theme.clone(), 160))),
            Kind::Reveal(expanded) => (0, if *expanded { 1.0 } else { 0.0 }, None),
            Kind::Anchor(_) => (0, 1.0, None),
        };
        tree::State::new(State {
            key,
            progress: transition(progress, 120),
            theme,
            overlay_present: false,
            overlay_progress: transition(1.0, 120),
            overlay_pointer_stale: false,
            overlay_geometry: PopupGeometry::default(),
            anchor_id: if matches!(self.kind, Kind::Anchor(_)) {
                NEXT_ANCHOR.fetch_add(1, Ordering::Relaxed)
            } else {
                0
            },
            overlay_anchor_id: None,
            closing: None,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        match &self.kind {
            Kind::Page { key, theme } => {
                if state.key != *key {
                    state.key = *key;
                    state.progress = transition(0.0, 120).to(1.0);
                    state.closing = None;
                    state.overlay_present = false;
                    state.overlay_anchor_id = None;
                    state.overlay_progress.settle_at(1.0);
                }
                let current = state
                    .theme
                    .get_or_insert_with(|| transition(theme.clone(), 160));
                if current.target() != theme {
                    current.set_target(theme.clone());
                }
            }
            Kind::Reveal(expanded) => {
                let target = if *expanded { 1.0 } else { 0.0 };
                if *state.progress.target() != target {
                    state.progress.set_target(target);
                }
            }
            Kind::Anchor(_) => {}
        }
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let progress = tree
            .state
            .downcast_ref::<State>()
            .progress
            .value()
            .clamp(0.0, 1.0);
        let child_limits = match self.kind {
            Kind::Page { .. } | Kind::Anchor(_) => *limits,
            Kind::Reveal(_) => layout::Limits::new(
                Size::new(limits.min().width, 0.0),
                Size::new(limits.max().width, f32::INFINITY),
            ),
        };
        let node =
            self.content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, &child_limits);
        let size = node.size();
        match self.kind {
            Kind::Page { .. } | Kind::Anchor(_) => layout::Node::with_children(size, vec![node]),
            Kind::Reveal(_) => layout::Node::with_children(
                Size::new(size.width, size.height * progress),
                vec![node],
            ),
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if CLOSE_BROADCAST.with(Cell::get) {
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout.children().next().unwrap(),
                mouse::Cursor::Unavailable,
                renderer,
                clipboard,
                shell,
                viewport,
            );
            return;
        }
        if matches!(self.kind, Kind::Page { .. }) {
            let popup_active = {
                let state = tree.state.downcast_ref::<State>();
                state.overlay_present || state.closing.is_some()
            };
            // 菜单内部的滚轮已由 overlay 捕获；到达 Page 的外部滚轮先清菜单，
            // 随后仍原样交给正文滚动，避免锚点移出视口后残留悬空菜单。
            // 无菜单时不合成点击，保留文本框对窗口失焦/恢复及缩放的原生焦点处理。
            let cancel = popup_active
                && (cancel_window_event(event)
                    || matches!(event, Event::Mouse(mouse::Event::WheelScrolled { .. })));
            let ready = tree
                .state
                .downcast_ref::<State>()
                .closing
                .as_ref()
                .is_some_and(|closing| closing.ready);
            if cancel || ready {
                let pending = tree.state.downcast_mut::<State>().closing.take();
                self.close_children(tree, layout, renderer, clipboard, shell, viewport);
                {
                    let state = tree.state.downcast_mut::<State>();
                    state.overlay_present = false;
                    state.overlay_anchor_id = None;
                }
                if !cancel
                    && let Some(Closing {
                        intent: CloseIntent::Outside(press),
                        release,
                        ..
                    }) = pending
                {
                    self.replay_pointer(press, tree, layout, renderer, clipboard, shell, viewport);
                    if let Some(release) = release {
                        self.replay_pointer(
                            release, tree, layout, renderer, clipboard, shell, viewport,
                        );
                    }
                }
                shell.request_redraw();
            } else if popup_active && pointer_press(event) {
                self.close_children(tree, layout, renderer, clipboard, shell, viewport);
            }
        }
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<State>();
            if state.progress.is_animating() {
                state.progress.tick(*now);
                if matches!(self.kind, Kind::Reveal(_)) {
                    shell.invalidate_layout();
                }
                if state.progress.is_animating() {
                    shell.request_redraw();
                }
            }
            if let Some(theme) = &mut state.theme
                && theme.is_animating()
            {
                theme.tick(*now);
                if theme.is_animating() {
                    shell.request_redraw();
                }
            }
        }
        if !self.interactive() {
            return;
        }
        if let Some(visible) = layout.bounds().intersection(viewport) {
            let cursor = if cursor.is_over(visible) {
                cursor
            } else {
                mouse::Cursor::Unavailable
            };
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout.children().next().unwrap(),
                cursor,
                renderer,
                clipboard,
                shell,
                &visible,
            );
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let Some(visible) = layout.bounds().intersection(viewport) else {
            return;
        };
        let state = tree.state.downcast_ref::<State>();
        let theme = state
            .theme
            .as_ref()
            .map_or(theme, |animation| animation.value());
        let faded = if matches!(self.kind, Kind::Page { .. }) && *state.progress.value() < 1.0 {
            Some(fade_theme(theme, *state.progress.value()))
        } else {
            None
        };
        let theme = faded.as_ref().unwrap_or(theme);
        let draw = |renderer: &mut Renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout.children().next().unwrap(),
                cursor,
                &visible,
            );
        };
        if matches!(self.kind, Kind::Reveal(_)) && *state.progress.value() < 1.0 {
            renderer.with_layer(visible, draw);
        } else {
            draw(renderer);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if !self.interactive() || !cursor.is_over(layout.bounds()) {
            return mouse::Interaction::None;
        }
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.interactive() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout.children().next().unwrap(),
                renderer,
                operation,
            );
        }
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        if !self.interactive() {
            return None;
        }
        let page = matches!(self.kind, Kind::Page { .. });
        if page {
            POPUP_ANCHOR.with(|anchor| anchor.set(None));
        }
        let state = tree.state.downcast_mut::<State>();
        let native = self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        );
        if !page {
            if let Kind::Anchor(revision) = self.kind
                && native.is_some()
            {
                // 多个旧原生状态同时 open 时也只收集一个；下一次关闭广播会清理全部。
                if POPUP_ANCHOR.with(Cell::get).is_some() {
                    return None;
                }
                POPUP_ANCHOR.with(|anchor| {
                    anchor.set(Some(Anchor {
                        id: state.anchor_id,
                        bounds: layout.bounds() + translation,
                        revision,
                    }))
                });
            }
            return native;
        }
        match native {
            Some(inner) => {
                // 提示浮层沿用原生事件处理，只有显式标记的下拉菜单参与菜单动画。
                let Some(anchor) = POPUP_ANCHOR.with(Cell::get) else {
                    state.overlay_present = false;
                    state.overlay_anchor_id = None;
                    state.closing = None;
                    return Some(inner);
                };
                state.open_overlay(anchor.id);
                Some(overlay::Element::new(Box::new(Popup {
                    inner,
                    progress: &mut state.overlay_progress,
                    pointer_stale: &mut state.overlay_pointer_stale,
                    theme: state.theme.as_ref().map(|theme| theme.value().clone()),
                    anchor,
                    geometry: &mut state.overlay_geometry,
                    closing: &mut state.closing,
                })))
            }
            None => {
                state.overlay_present = false;
                state.overlay_progress.settle_at(1.0);
                state.overlay_pointer_stale = false;
                state.overlay_geometry = PopupGeometry::default();
                state.overlay_anchor_id = None;
                // 原生选择可能在本次 overlay.update 内先关闭；保留终帧状态给随后 Page.update 清理。
                if let Some(closing) = &mut state.closing {
                    if !closing.ready {
                        closing.cancel();
                    }
                    closing.ready = true;
                }
                POPUP_ABOVE.with(|above| above.set(false));
                None
            }
        }
    }
}

// 只调整原生弹层的布局节点，所有输入和命中测试使用相同坐标。
// 原生控件收起后立即释放弹层，不保留已经失效的控件借用来模拟关闭动画。
struct Popup<'a> {
    inner: overlay::Element<'a, Message, Theme, Renderer>,
    progress: &'a mut Animated<f32>,
    pointer_stale: &'a mut bool,
    theme: Option<Theme>,
    anchor: Anchor,
    // Iced 绘制时重新创建 Overlay 实例并复用旧 layout，几何必须和动画一起存入 Tree。
    geometry: &'a mut PopupGeometry,
    closing: &'a mut Option<Closing>,
}

fn popup_bounds(node: &layout::Node, viewport: Size) -> Option<Rectangle> {
    if node.size() != viewport {
        return Some(node.bounds());
    }
    node.children()
        .iter()
        .find_map(|child| popup_bounds(child, viewport))
}

fn popup_placement(menu: Rectangle, anchor: Rectangle, viewport: Size) -> (Rectangle, bool) {
    let below = (viewport.height - anchor.y - anchor.height).max(0.0);
    let above = menu.height > below;
    let y = if above {
        (anchor.y - menu.height).max(0.0)
    } else {
        anchor.y + anchor.height
    };
    (Rectangle { y, ..menu }, above)
}

impl overlay::Overlay<Message, Theme, Renderer> for Popup<'_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.inner.as_overlay_mut().layout(renderer, bounds);
        let native_menu = popup_bounds(&node, bounds).unwrap_or_else(|| node.bounds());
        let (menu, above) = popup_placement(native_menu, self.anchor.bounds, bounds);
        if let Some(closing) = self.closing.as_mut()
            && (closing.anchor != self.anchor || closing.menu != menu || closing.viewport != bounds)
        {
            closing.cancel();
        }
        self.geometry.above = above;
        self.geometry.menu = menu;
        self.geometry.viewport = bounds;
        POPUP_ABOVE.with(|above| above.set(self.geometry.above));
        let progress = self.progress.value().clamp(0.0, 1.0);
        let height = menu.height * progress;
        self.geometry.clip = Rectangle {
            x: menu.x,
            y: if self.geometry.above {
                menu.y + menu.height - height
            } else {
                menu.y
            },
            width: menu.width,
            height,
        };
        let offset = (1.0 - progress) * if self.geometry.above { 2.0 } else { -2.0 };
        node.translate(Vector::new(0.0, menu.y - native_menu.y + offset))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;
        if self.geometry.clip.height <= 0.0 {
            return;
        }
        let draw = |renderer: &mut Renderer| {
            self.inner.as_overlay().draw(
                renderer,
                self.theme.as_ref().unwrap_or(theme),
                style,
                layout,
                cursor,
            )
        };
        if *self.progress.value() < 1.0 {
            renderer.with_layer(self.geometry.clip, draw);
        } else {
            draw(renderer);
        }
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let cursor = match event {
            Event::Touch(
                iced::touch::Event::FingerPressed { position, .. }
                | iced::touch::Event::FingerLifted { position, .. },
            ) => mouse::Cursor::Available(*position),
            _ => cursor,
        };
        if self.closing.is_none() && (pointer_press(event) || escape(event)) {
            let inside = cursor.is_over(self.geometry.clip);
            let option = inside
                && self
                    .inner
                    .as_overlay()
                    .mouse_interaction(layout, cursor, renderer)
                    == mouse::Interaction::Pointer;
            // 滚动条按下仍交原生控件，只有选项/菜单外点击开始收起。
            if option || !inside || escape(event) {
                let intent = if escape(event) || cursor.is_over(self.anchor.bounds) {
                    CloseIntent::Dismiss
                } else if option {
                    if let Some(position) = cursor.position() {
                        without_capture(shell, |local| {
                            self.inner.as_overlay_mut().update(
                                &Event::Mouse(mouse::Event::CursorMoved { position }),
                                layout,
                                cursor,
                                renderer,
                                clipboard,
                                local,
                            )
                        });
                    }
                    CloseIntent::Select
                } else {
                    CloseIntent::Outside(PointerEvent {
                        event: event.clone(),
                        cursor,
                    })
                };
                *self.closing = Some(Closing {
                    intent,
                    release: None,
                    anchor: self.anchor,
                    menu: self.geometry.menu,
                    viewport: self.geometry.viewport,
                    ready: false,
                });
                *self.progress = transition(*self.progress.value(), 100).to(0.0);
                shell.request_redraw();
                shell.capture_event();
                return;
            }
        }
        if let Some(closing) = self.closing.as_mut() {
            if escape(event) || cancel_window_event(event) {
                closing.cancel();
            }
            if let CloseIntent::Outside(press) = &closing.intent
                && closing.release.is_none()
                && matching_release(&press.event, event)
            {
                closing.release = Some(PointerEvent {
                    event: event.clone(),
                    cursor,
                });
            }
            if matches!(
                event,
                Event::Mouse(_) | Event::Touch(_) | Event::Keyboard(_)
            ) {
                shell.capture_event();
                return;
            }
        }
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event
            && self.progress.is_animating()
        {
            *self.pointer_stale = true;
            self.progress.tick(*now);
            shell.invalidate_layout();
            if self.progress.is_animating() {
                shell.request_redraw();
            }
        }
        if let Some(closing) = self.closing.as_mut() {
            if *self.progress.value() <= 0.0 && !closing.ready {
                if matches!(closing.intent, CloseIntent::Select) {
                    closing.intent = CloseIntent::Dismiss;
                    // 不按缩至零的clip过滤，不再刷新hover；原生缓存索引就是开始收起时的选择。
                    without_capture(shell, |local| {
                        self.inner.as_overlay_mut().update(
                            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                            layout,
                            mouse::Cursor::Available(self.geometry.menu.center()),
                            renderer,
                            clipboard,
                            local,
                        )
                    });
                }
                closing.ready = true;
            }
            self.inner.as_overlay_mut().update(
                event,
                layout,
                mouse::Cursor::Unavailable,
                renderer,
                clipboard,
                shell,
            );
            return;
        }
        let cursor = if cursor.is_over(self.geometry.clip) {
            cursor
        } else {
            mouse::Cursor::Unavailable
        };
        self.inner
            .as_overlay_mut()
            .update(event, layout, cursor, renderer, clipboard, shell);
        if matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. })) {
            *self.pointer_stale = false;
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if !cursor.is_over(self.geometry.clip) {
            return mouse::Interaction::None;
        }
        self.inner
            .as_overlay()
            .mouse_interaction(layout, cursor, renderer)
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.inner
            .as_overlay_mut()
            .operate(layout, renderer, operation);
    }

    fn overlay<'a>(
        &'a mut self,
        layout: Layout<'a>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.inner.as_overlay_mut().overlay(layout, renderer)
    }

    fn index(&self) -> f32 {
        self.inner.as_overlay().index()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn native_tooltip_keeps_noise_button_clickable_while_open() {
        let mut cache = iced_runtime::user_interface::Cache::new();
        let mut renderer = Renderer::new(iced::Font::DEFAULT, iced::Pixels(13.0));
        let mut messages = Vec::new();
        let cursor = mouse::Cursor::Available(iced::Point::new(40., 18.));
        for event in [
            Event::Mouse(mouse::Event::CursorMoved {
                position: iced::Point::new(40., 18.),
            }),
            Event::Window(iced::window::Event::RedrawRequested(
                Instant::now() + Duration::from_secs(3),
            )),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ] {
            let control =
                iced::widget::button("中")
                    .width(120)
                    .height(36)
                    .on_press(Message::Running(
                        super::super::running_panel::Message::Noise(crate::running::Noise::Medium),
                    ));
            let tooltip = iced::widget::tooltip(
                control,
                "最大模拟偏移：10 米",
                iced::widget::tooltip::Position::Top,
            );
            let mut ui = iced_runtime::UserInterface::build(
                page(0, Theme::Light, tooltip.into()),
                Size::new(420., 520.),
                cache,
                &mut renderer,
            );
            ui.update(
                &[event],
                cursor,
                &mut renderer,
                &mut iced::advanced::clipboard::Null,
                &mut messages,
            );
            ui.draw(
                &mut renderer,
                &Theme::Light,
                &renderer::Style::default(),
                cursor,
            );
            cache = ui.into_cache();
        }
        assert!(messages.iter().any(|message| matches!(
            message,
            Message::Running(super::super::running_panel::Message::Noise(
                crate::running::Noise::Medium
            ))
        )));
    }

    fn native_picker(top: f32, pair: bool) -> Element<'static, Message> {
        const OPTIONS: &[&str] = &["Synthetic A", "Synthetic B"];
        let picker = || {
            popup_anchor(
                iced::widget::pick_list(OPTIONS, Some(OPTIONS[0]), |value| {
                    Message::Ssid(value.to_owned())
                })
                .width(174)
                .padding([7, 10])
                .text_size(13),
            )
        };
        let content: Element<'static, Message> = if pair {
            iced::widget::row![picker(), picker()].spacing(12).into()
        } else {
            picker()
        };
        page(
            0,
            Theme::Light,
            iced::widget::container(content)
                .padding(iced::Padding {
                    top,
                    left: 16.0,
                    ..Default::default()
                })
                .into(),
        )
    }

    // 保持与原生 Widget::update 相同的测试输入，避免隐藏布局或事件坐标。
    #[allow(clippy::too_many_arguments)]
    fn base_event(
        root: &mut Element<'_, Message>,
        tree: &mut Tree,
        base: &layout::Node,
        renderer: &Renderer,
        viewport: Rectangle,
        cursor: mouse::Cursor,
        event: Event,
        messages: &mut Vec<Message>,
    ) {
        let mut clipboard = iced::advanced::clipboard::Null;
        let mut shell = Shell::new(messages);
        root.as_widget_mut().update(
            tree,
            &event,
            Layout::new(base),
            cursor,
            renderer,
            &mut clipboard,
            &mut shell,
            &viewport,
        );
    }

    fn exercise_native_popup(top: f32, expect_above: bool) {
        let viewport = Rectangle::with_size(Size::new(420.0, 520.0));
        let limits = layout::Limits::new(Size::ZERO, viewport.size());
        let mut renderer = Renderer::new(iced::Font::DEFAULT, iced::Pixels(13.0));
        let mut root = native_picker(top, false);
        let mut tree = Tree::new(&root);
        let mut base = root.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let mut messages = Vec::new();
        let control = iced::Point::new(30.0, top + 15.0);
        base_event(
            &mut root,
            &mut tree,
            &base,
            &renderer,
            viewport,
            mouse::Cursor::Available(control),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            &mut messages,
        );
        {
            let mut popup = root
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap();
            let node = popup.as_overlay_mut().layout(&renderer, viewport.size());
            let mut clipboard = iced::advanced::clipboard::Null;
            let mut shell = Shell::new(&mut messages);
            popup.as_overlay_mut().update(
                &Event::Window(iced::window::Event::RedrawRequested(
                    Instant::now() + Duration::from_millis(60),
                )),
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &renderer,
                &mut clipboard,
                &mut shell,
            );
            assert!(shell.is_layout_invalid());
        }
        // 对照运行时：动画请求重新 layout，然后 draw 会重新创建 Popup 并复用缓存 node。
        base = root.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let cached = {
            let mut popup = root
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap();
            popup.as_overlay_mut().layout(&renderer, viewport.size())
        };
        let geometry = &tree.state.downcast_ref::<State>().overlay_geometry;
        assert!(geometry.clip.height > 0.0);
        assert_eq!(geometry.above, expect_above);
        let cursor = mouse::Cursor::Available(iced::Point::new(
            geometry.clip.x + 12.0,
            geometry.clip.y + geometry.clip.height / 2.0,
        ));
        {
            let mut fresh = root
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap();
            // 此处故意不调用 fresh.layout：修复前临时实例 clip=0，以下命中断言会失败且 draw 完全隐藏。
            assert_eq!(
                fresh
                    .as_overlay()
                    .mouse_interaction(Layout::new(&cached), cursor, &renderer),
                mouse::Interaction::Pointer
            );
            fresh.as_overlay().draw(
                &mut renderer,
                &Theme::Light,
                &renderer::Style::default(),
                Layout::new(&cached),
                cursor,
            );
            let mut clipboard = iced::advanced::clipboard::Null;
            let mut shell = Shell::new(&mut messages);
            fresh.as_overlay_mut().update(
                &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Layout::new(&cached),
                cursor,
                &renderer,
                &mut clipboard,
                &mut shell,
            );
            assert!(shell.is_event_captured());
        }
        assert!(messages.is_empty(), "选择应在收起完成后仅提交一次");
        let initial = *tree.state.downcast_ref::<State>().overlay_progress.value();
        {
            let mut fresh = root
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap();
            let mut clipboard = iced::advanced::clipboard::Null;
            let mut shell = Shell::new(&mut messages);
            fresh.as_overlay_mut().update(
                &Event::Window(iced::window::Event::RedrawRequested(
                    Instant::now() + Duration::from_millis(25),
                )),
                Layout::new(&cached),
                cursor,
                &renderer,
                &mut clipboard,
                &mut shell,
            );
        }
        let middle = *tree.state.downcast_ref::<State>().overlay_progress.value();
        assert!(middle > 0.0 && middle < initial);
        assert!(messages.is_empty());
        let final_event = Event::Window(iced::window::Event::RedrawRequested(
            Instant::now() + Duration::from_millis(200),
        ));
        {
            let mut fresh = root
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap();
            let mut clipboard = iced::advanced::clipboard::Null;
            let mut shell = Shell::new(&mut messages);
            fresh.as_overlay_mut().update(
                &final_event,
                Layout::new(&cached),
                cursor,
                &renderer,
                &mut clipboard,
                &mut shell,
            );
            assert!(!shell.is_event_captured(), "终帧必须可进入页面清理");
        }
        assert!(messages.iter().any(
            |message| matches!(message, Message::Ssid(value) if value.starts_with("Synthetic "))
        ));
        assert!(
            root.as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO
                )
                .is_none()
        );
        base_event(
            &mut root,
            &mut tree,
            &base,
            &renderer,
            viewport,
            cursor,
            final_event,
            &mut messages,
        );
        assert_eq!(messages.len(), 1);
        assert!(!tree.state.downcast_ref::<State>().overlay_present);
    }

    #[test]
    fn native_popup_survives_cached_layout_recreation_and_prefers_available_below() {
        // 上方空间更多，但下方足够容纳完整的两项菜单，应朝下。
        exercise_native_popup(300.0, false);
    }

    #[test]
    fn native_popup_uses_above_when_bottom_cannot_fit_the_menu() {
        exercise_native_popup(470.0, true);
    }

    #[test]
    fn native_anchor_move_keeps_progress_and_switching_open_fields_restarts_it() {
        let viewport = Rectangle::with_size(Size::new(420.0, 520.0));
        let limits = layout::Limits::new(Size::ZERO, viewport.size());
        let renderer = Renderer::new(iced::Font::DEFAULT, iced::Pixels(13.0));
        let mut root = native_picker(16.0, true);
        let mut tree = Tree::new(&root);
        let base = root.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let mut messages = Vec::new();
        base_event(
            &mut root,
            &mut tree,
            &base,
            &renderer,
            viewport,
            mouse::Cursor::Available(iced::Point::new(30.0, 30.0)),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            &mut messages,
        );
        drop(
            root.as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap(),
        );
        let (first_id, middle) = {
            let state = tree.state.downcast_mut::<State>();
            state
                .overlay_progress
                .tick(Instant::now() + Duration::from_millis(60));
            (state.overlay_anchor_id, *state.overlay_progress.value())
        };
        assert!(middle > 0.0 && middle < 1.0);
        let mut moved = native_picker(22.0, true);
        tree.diff(moved.as_widget());
        let base = moved.as_widget_mut().layout(&mut tree, &renderer, &limits);
        drop(
            moved
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap(),
        );
        let state = tree.state.downcast_ref::<State>();
        assert_eq!(state.overlay_anchor_id, first_id);
        assert_eq!(*state.overlay_progress.value(), middle);
        base_event(
            &mut moved,
            &mut tree,
            &base,
            &renderer,
            viewport,
            mouse::Cursor::Available(iced::Point::new(220.0, 36.0)),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            &mut messages,
        );
        // 未在两次 Some 之间查询 None，仍须识别另一个控件。
        drop(
            moved
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&base),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .unwrap(),
        );
        let state = tree.state.downcast_ref::<State>();
        assert_ne!(state.overlay_anchor_id, first_id);
        assert_eq!(*state.overlay_progress.value(), 0.0);
        assert_eq!(state.overlay_geometry.clip.height, 0.0);
    }

    fn runtime_fixture(
        key: u8,
        revision: u64,
        input: &'static str,
        wrapped: bool,
    ) -> Element<'static, Message> {
        let picker = |options: &'static [&'static str], revision| {
            popup_anchor_revision(
                iced::widget::pick_list(options, Some(options[0]), |value| {
                    Message::Ssid(value.to_owned())
                })
                .width(174)
                .padding([7, 10])
                .text_size(13),
                revision,
            )
        };
        let content = iced::widget::container(iced::widget::column![
            picker(&["Upper A", "Upper B"], revision),
            space().height(120),
            picker(&["Lower A", "Lower B"], 0),
            space().height(120),
            iced::widget::button("Synthetic action")
                .width(174)
                .height(36)
                .on_press(Message::Login),
            space().height(12),
            iced::widget::text_input("Synthetic account", input)
                .on_input(Message::Username)
                .width(174)
                .size(13)
                .padding([7, 10]),
        ])
        .padding(16)
        .into();
        if wrapped {
            page(key, Theme::Light, content)
        } else {
            content
        }
    }

    // 真实 runtime 负责 overlay 事件、失效重排、重新借用和 base 事件顺序；
    // 每次都经 Cache 重建并 draw，覆盖实际应用 Tick 后重建视图的生命周期。
    struct RuntimeScene {
        cache: Option<iced_runtime::user_interface::Cache>,
        renderer: Renderer,
        messages: Vec<Message>,
        key: u8,
        revision: u64,
        input: &'static str,
        wrapped: bool,
    }

    impl RuntimeScene {
        fn new() -> Self {
            Self {
                cache: Some(iced_runtime::user_interface::Cache::new()),
                renderer: Renderer::new(iced::Font::DEFAULT, iced::Pixels(13.0)),
                messages: Vec::new(),
                key: 0,
                revision: 0,
                input: "",
                wrapped: true,
            }
        }

        fn step(
            &mut self,
            events: &[Event],
            cursor: mouse::Cursor,
        ) -> iced_runtime::user_interface::State {
            let mut ui = iced_runtime::UserInterface::build(
                runtime_fixture(self.key, self.revision, self.input, self.wrapped),
                Size::new(420.0, 520.0),
                self.cache.take().unwrap(),
                &mut self.renderer,
            );
            let (state, _) = ui.update(
                events,
                cursor,
                &mut self.renderer,
                &mut iced::advanced::clipboard::Null,
                &mut self.messages,
            );
            ui.draw(
                &mut self.renderer,
                &Theme::Light,
                &renderer::Style::default(),
                cursor,
            );
            self.cache = Some(ui.into_cache());
            state
        }

        fn press(&mut self, x: f32, y: f32) {
            self.step(
                &[Event::Mouse(mouse::Event::ButtonPressed(
                    mouse::Button::Left,
                ))],
                mouse::Cursor::Available(iced::Point::new(x, y)),
            );
        }

        fn release(&mut self, x: f32, y: f32) {
            self.step(
                &[Event::Mouse(mouse::Event::ButtonReleased(
                    mouse::Button::Left,
                ))],
                mouse::Cursor::Available(iced::Point::new(x, y)),
            );
        }

        fn tick(&mut self) -> iced_runtime::user_interface::State {
            self.step(
                &[Event::Window(iced::window::Event::RedrawRequested(
                    Instant::now() + Duration::from_millis(250),
                ))],
                mouse::Cursor::Unavailable,
            )
        }

        fn type_x(&mut self) {
            let key = iced::keyboard::Key::Character("x".into());
            self.step(
                &[Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key: iced::keyboard::key::Physical::Code(
                        iced::keyboard::key::Code::KeyX,
                    ),
                    location: iced::keyboard::Location::Standard,
                    modifiers: iced::keyboard::Modifiers::empty(),
                    text: Some("x".into()),
                    repeat: false,
                })],
                mouse::Cursor::Unavailable,
            );
        }

        fn open(&mut self, y: f32) {
            self.press(30.0, y);
            self.release(30.0, y);
            self.tick();
        }

        fn interaction(&mut self, y: f32) -> mouse::Interaction {
            match self.step(&[], mouse::Cursor::Available(iced::Point::new(30.0, y))) {
                iced_runtime::user_interface::State::Updated {
                    mouse_interaction, ..
                } => mouse_interaction,
                _ => panic!("合成界面不应过期"),
            }
        }
    }

    #[test]
    fn runtime_select_waits_for_close_and_drains_after_native_overlay_disappears() {
        let mut scene = RuntimeScene::new();
        scene.open(30.0);
        assert_eq!(scene.interaction(95.0), mouse::Interaction::Pointer);
        scene.press(30.0, 95.0);
        scene.release(30.0, 95.0);
        assert!(scene.messages.is_empty());
        // 收起期间移动/再次点击不改变已冻结选项或增加第二个动作。
        scene.step(
            &[Event::Mouse(mouse::Event::CursorMoved {
                position: iced::Point::new(30.0, 65.0),
            })],
            mouse::Cursor::Available(iced::Point::new(30.0, 65.0)),
        );
        scene.press(30.0, 65.0);
        scene.tick();
        assert_eq!(scene.messages.len(), 1);
        assert!(matches!(&scene.messages[0], Message::Ssid(value) if value == "Upper B"));
        assert_eq!(scene.interaction(95.0), mouse::Interaction::None);
        assert!(matches!(
            scene.tick(),
            iced_runtime::user_interface::State::Updated {
                redraw_request: iced::window::RedrawRequest::Wait,
                ..
            }
        ));
    }

    #[test]
    fn runtime_switches_lower_to_upper_and_back_without_retaining_hidden_menus() {
        let mut scene = RuntimeScene::new();
        scene.open(180.0);
        for (control, option, expected, old_option) in [
            (30.0, 95.0, "Upper B", 245.0),
            (180.0, 245.0, "Lower B", 95.0),
            (30.0, 95.0, "Upper B", 245.0),
        ] {
            scene.press(30.0, control);
            scene.release(30.0, control);
            scene.tick(); // 旧菜单收起、清理，再重放一次新字段点击。
            scene.tick(); // 新菜单展开。
            assert_eq!(scene.interaction(option), mouse::Interaction::Pointer);
            assert_eq!(scene.interaction(old_option), mouse::Interaction::None);
            scene.press(30.0, option);
            scene.release(30.0, option);
            scene.tick();
            assert!(
                matches!(scene.messages.last(), Some(Message::Ssid(value)) if value == expected)
            );
            assert_eq!(scene.interaction(option), mouse::Interaction::None);
            assert_eq!(scene.interaction(old_option), mouse::Interaction::None);
            scene.open(control);
        }
        assert_eq!(scene.messages.len(), 3);
    }

    #[test]
    fn runtime_same_anchor_closes_and_outside_button_replays_one_matched_pair() {
        let mut scene = RuntimeScene::new();
        scene.open(30.0);
        scene.press(30.0, 30.0);
        scene.release(30.0, 30.0);
        scene.tick();
        assert_eq!(scene.interaction(95.0), mouse::Interaction::None);
        assert!(scene.messages.is_empty());

        scene.open(30.0);
        scene.press(30.0, 335.0);
        scene.release(30.0, 335.0);
        // 第一对之外的重复点击不会累积多个按钮动作。
        scene.press(30.0, 335.0);
        scene.release(30.0, 335.0);
        scene.tick();
        assert_eq!(scene.messages.len(), 1);
        assert!(matches!(scene.messages[0], Message::Login));

        scene.messages.clear();
        scene.open(30.0);
        scene.press(30.0, 335.0);
        scene.release(300.0, 400.0);
        scene.tick();
        assert!(scene.messages.is_empty(), "释放到按钮外不能误触发");
    }

    #[test]
    fn runtime_stale_revision_window_focus_and_page_cancel_pending_actions() {
        let mut scene = RuntimeScene::new();
        scene.open(30.0);
        scene.press(30.0, 95.0);
        scene.revision += 1;
        scene.tick();
        assert!(scene.messages.is_empty());
        assert_eq!(scene.interaction(95.0), mouse::Interaction::None);

        scene.open(30.0);
        scene.press(30.0, 335.0);
        scene.release(30.0, 335.0);
        scene.step(
            &[Event::Window(iced::window::Event::Unfocused)],
            mouse::Cursor::Unavailable,
        );
        scene.tick();
        assert!(scene.messages.is_empty());

        scene.open(30.0);
        scene.press(30.0, 335.0);
        scene.release(30.0, 335.0);
        scene.key += 1;
        scene.tick();
        assert!(scene.messages.is_empty());
    }

    #[test]
    fn runtime_escape_cancels_selection_and_external_wheel_clears_open_menu() {
        let mut scene = RuntimeScene::new();
        scene.open(30.0);
        scene.press(30.0, 95.0);
        let key = iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape);
        scene.step(
            &[Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: iced::keyboard::key::Physical::Code(
                    iced::keyboard::key::Code::Escape,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: iced::keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            })],
            mouse::Cursor::Unavailable,
        );
        scene.tick();
        assert!(scene.messages.is_empty());
        assert_eq!(scene.interaction(95.0), mouse::Interaction::None);

        scene.open(30.0);
        scene.step(
            &[Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            })],
            mouse::Cursor::Available(iced::Point::new(300.0, 350.0)),
        );
        assert_eq!(scene.interaction(95.0), mouse::Interaction::None);
        assert!(scene.messages.is_empty());
    }

    #[test]
    fn runtime_open_menu_survives_idle_view_rebuilds_without_continuous_redraw() {
        let mut scene = RuntimeScene::new();
        scene.open(30.0);
        for seconds in 1..=10 {
            let state = scene.step(
                &[Event::Window(iced::window::Event::RedrawRequested(
                    Instant::now() + Duration::from_secs(seconds),
                ))],
                mouse::Cursor::Unavailable,
            );
            assert!(matches!(
                state,
                iced_runtime::user_interface::State::Updated {
                    redraw_request: iced::window::RedrawRequest::Wait,
                    ..
                }
            ));
            assert_eq!(scene.interaction(95.0), mouse::Interaction::Pointer);
        }
        scene.press(30.0, 95.0);
        scene.release(30.0, 95.0);
        scene.tick();
        assert_eq!(scene.messages.len(), 1);
    }

    #[test]
    fn runtime_window_events_without_menu_preserve_native_text_input_focus() {
        let mut scene = RuntimeScene::new();
        scene.press(30.0, 380.0);
        scene.release(30.0, 380.0);
        for event in [
            iced::window::Event::Resized(Size::new(420.0, 520.0)),
            iced::window::Event::Rescaled(1.5),
            iced::window::Event::Unfocused,
            iced::window::Event::Focused,
        ] {
            scene.step(&[Event::Window(event)], mouse::Cursor::Unavailable);
        }
        scene.type_x();
        assert_eq!(scene.messages.len(), 1);
        assert!(matches!(&scene.messages[0], Message::Username(value) if value == "x"));
    }

    #[test]
    fn runtime_without_menu_preserves_native_double_click_word_selection() {
        for wrapped in [false, true] {
            let mut scene = RuntimeScene::new();
            scene.input = "synthetic";
            scene.wrapped = wrapped;
            // 同时验证无 Flow 包装的原生对照和实际 Page 包装：双击必须选中整词。
            for _ in 0..2 {
                scene.press(30.0, 380.0);
                scene.release(30.0, 380.0);
            }
            scene.type_x();
            assert_eq!(scene.messages.len(), 1);
            assert!(matches!(&scene.messages[0], Message::Username(value) if value == "x"));
        }
    }

    #[test]
    fn switch_progress_has_intermediate_frame_and_repeated_target_does_not_restart() {
        let mut state = SwitchState::new(false);
        assert!(!state.progress.is_animating());
        state.retarget(true);
        let start = Instant::now();
        state.progress.tick(start + Duration::from_millis(70));
        let middle = state.value();
        assert!(middle > 0.0 && middle < 1.0);
        state.retarget(true);
        assert_eq!(state.value(), middle);
        state.progress.tick(start + Duration::from_millis(150));
        assert_eq!(state.value(), 1.0);
        assert!(!state.progress.is_animating());
    }

    #[test]
    fn switch_can_reverse_while_between_endpoints() {
        let mut state = SwitchState::new(false);
        state.retarget(true);
        state
            .progress
            .tick(Instant::now() + Duration::from_millis(70));
        let middle = state.value();
        state.retarget(false);
        assert_eq!(state.value(), middle);
        let reverse_start = Instant::now();
        state
            .progress
            .tick(reverse_start + Duration::from_millis(20));
        assert!(state.value() > 0.0 && state.value() < middle);
        state
            .progress
            .tick(reverse_start + Duration::from_millis(150));
        assert_eq!(state.value(), 0.0);
        assert!(!state.progress.is_animating());
    }

    #[test]
    fn rebuilt_switch_widget_preserves_tree_progress() {
        let initial = animated_switch(false, Message::Debug);
        let mut tree = Tree::new(&initial);
        let enabled = animated_switch(true, Message::Debug);
        tree.diff(enabled.as_widget());
        let middle = {
            let state = tree.state.downcast_mut::<SwitchState>();
            state
                .progress
                .tick(Instant::now() + Duration::from_millis(70));
            state.value()
        };
        assert!(middle > 0.0 && middle < 1.0);
        let reconstructed = animated_switch(true, Message::Debug);
        tree.diff(reconstructed.as_widget());
        let state = tree.state.downcast_ref::<SwitchState>();
        assert_eq!(state.value(), middle);
        assert!(state.progress.is_animating());
        assert_eq!(*state.progress.target(), 1.0);
        assert_eq!(tree.children.len(), 1);
    }
}
