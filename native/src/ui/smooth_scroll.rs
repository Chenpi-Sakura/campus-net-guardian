//! 仅为离散滚轮增加短缓动，滚动范围和事件消费仍由原生 Scrollable 决定。

use super::Message;
use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Id, Operation, Tree, operation, tree},
    },
    keyboard, window,
};
use std::time::{Duration, Instant};

// iced_widget 0.14.2 scrollable.rs 的 Lines 转换使用每行 60 个逻辑像素。
const LINE_PIXELS: f32 = 60.0;
const IMMEDIATE: f32 = 0.22;
const TAU_SECONDS: f32 = 0.040;
const MAX_DURATION: Duration = Duration::from_millis(140);
const STOP_DISTANCE: f32 = 0.25;

/// 包装原生 Scrollable，不更改其布局、滚动状态、触控板或菜单事件。
pub(super) fn wrap<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Smooth {
        content: content.into(),
    })
}

#[derive(Clone, Default)]
struct Motion {
    remaining: Vector,
    last: Option<Instant>,
    deadline: Option<Instant>,
}

impl Motion {
    fn active(&self) -> bool {
        self.deadline.is_some()
    }

    fn stop(&mut self) {
        *self = Self::default();
    }

    fn input(&mut self, delta: Vector, now: Instant) -> Vector {
        for (remaining, input) in [
            (&mut self.remaining.x, delta.x),
            (&mut self.remaining.y, delta.y),
        ] {
            if input != 0.0 && input * *remaining < 0.0 {
                *remaining = 0.0;
            }
        }
        let elapsed = self.advance(now);
        let immediate = delta * IMMEDIATE;
        self.remaining = self.remaining + delta - immediate;
        if self.remaining.x != 0.0 || self.remaining.y != 0.0 {
            self.last = Some(now);
            self.deadline = Some(now + MAX_DURATION);
        }
        elapsed + immediate
    }

    fn advance(&mut self, now: Instant) -> Vector {
        let Some(deadline) = self.deadline else {
            return Vector::ZERO;
        };
        if now >= deadline || self.remaining.x.abs().max(self.remaining.y.abs()) <= STOP_DISTANCE {
            let remaining = self.remaining;
            self.stop();
            return remaining;
        }
        let elapsed = now.saturating_duration_since(self.last.unwrap_or(now));
        self.last = Some(now);
        let step = self.remaining * (1.0 - (-elapsed.as_secs_f32() / TAU_SECONDS).exp());
        self.remaining -= step;
        step
    }

    fn constrain(&mut self, requested: Vector, before: Geometry, after: Geometry) {
        if reached_boundary(
            before.translation.x,
            after.translation.x,
            after.content.width - after.bounds.width,
            requested.x,
        ) {
            self.remaining.x = 0.0;
        }
        if reached_boundary(
            before.translation.y,
            after.translation.y,
            after.content.height - after.bounds.height,
            requested.y,
        ) {
            self.remaining.y = 0.0;
        }
        if self.remaining.x == 0.0 && self.remaining.y == 0.0 {
            self.stop();
        }
    }
}

fn reached_boundary(before: f32, after: f32, extent: f32, requested: f32) -> bool {
    if requested == 0.0 {
        return false;
    }
    if extent <= 0.0 {
        return true;
    }
    // Iced 的公开 translation 已 round；位置差有最多 1px 量化误差，
    // 不能按 requested 与位移差判断消费量。只有对应 extent 边缘才清尾。
    let lower = after <= 0.5;
    let upper = after >= extent - 0.5;
    (lower && after < before)
        || (upper && after > before)
        || ((lower || upper) && after == before && requested.abs() > 1.0)
}

#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    bounds: Rectangle,
    content: Rectangle,
    translation: Vector,
}

impl Geometry {
    fn same_viewport(self, other: Self) -> bool {
        self.bounds == other.bounds && self.content == other.content
    }

    fn accepted_step(self, after: Self, requested: Vector) -> bool {
        self.same_viewport(after)
            && (self.translation != after.translation
                || requested.x.abs().max(requested.y.abs()) <= 1.0)
    }
}

#[derive(Default)]
struct Probe(Option<Geometry>);

impl Operation for Probe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.0.is_none() {
            operate(self);
        }
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
        _state: &mut dyn operation::Scrollable,
    ) {
        if self.0.is_none() {
            self.0 = Some(Geometry {
                bounds,
                content,
                translation,
            });
        }
    }
}

fn geometry(
    content: &mut Element<'_, Message>,
    tree: &mut Tree,
    layout: Layout<'_>,
    renderer: &Renderer,
) -> Option<Geometry> {
    let mut probe = Probe::default();
    content
        .as_widget_mut()
        .operate(tree, layout, renderer, &mut probe);
    probe.0
}

#[derive(Default)]
struct State {
    motion: Motion,
    origin: Option<Point>,
    target: Option<Geometry>,
    modifiers: keyboard::Modifiers,
}

struct Smooth<'a> {
    content: Element<'a, Message>,
}

// 形参对应原生 Widget::update；合成滚轮不能把 RedrawRequested 标为已消费。
#[allow(clippy::too_many_arguments)]
fn deliver(
    content: &mut Element<'_, Message>,
    tree: &mut Tree,
    event: &Event,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
    renderer: &Renderer,
    clipboard: &mut dyn Clipboard,
    shell: &mut Shell<'_, Message>,
    viewport: &Rectangle,
    capture: bool,
) -> bool {
    let mut messages = Vec::new();
    let captured;
    {
        let mut local = Shell::new(&mut messages);
        content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, &mut local, viewport,
        );
        captured = local.is_event_captured();
        if capture {
            shell.merge(local, |message| message);
        } else {
            shell.request_redraw_at(local.redraw_request());
            shell.request_input_method(local.input_method());
            if local.is_layout_invalid() {
                shell.invalidate_layout();
            }
            if local.are_widgets_invalid() {
                shell.invalidate_widgets();
            }
        }
    }
    for message in messages {
        shell.publish(message);
    }
    captured
}

impl Widget<Message, Theme, Renderer> for Smooth<'_> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
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
        let state = tree.state.downcast_mut::<State>();
        if let Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
            state.modifiers = *modifiers;
        }
        if let Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x, y },
        }) = event
            && state.modifiers.is_empty()
            && !shell.is_event_captured()
            && cursor.is_over(layout.bounds())
            && cursor.is_over(*viewport)
            && x.is_finite()
            && y.is_finite()
        {
            let before = geometry(&mut self.content, &mut tree.children[0], layout, renderer);
            if let Some(before) = before {
                let mut motion = state.motion.clone();
                let step = motion.input(Vector::new(*x, *y) * LINE_PIXELS, Instant::now());
                let wheel = Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Pixels {
                        x: step.x,
                        y: step.y,
                    },
                });
                let captured = deliver(
                    &mut self.content,
                    &mut tree.children[0],
                    &wheel,
                    layout,
                    cursor,
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                    true,
                );
                let after = geometry(&mut self.content, &mut tree.children[0], layout, renderer);
                if captured {
                    if let Some(after) = after
                        && before.accepted_step(after, step)
                    {
                        motion.constrain(step, before, after);
                        state.motion = motion;
                        state.origin = cursor.position();
                        state.target = Some(after);
                        if state.motion.active() {
                            shell.request_redraw();
                        }
                    } else {
                        state.motion.stop();
                    }
                    return;
                }
            }
            state.motion.stop();
        } else if !matches!(
            event,
            Event::Window(window::Event::RedrawRequested(_))
                | Event::Mouse(mouse::Event::CursorMoved { .. })
                | Event::Mouse(mouse::Event::CursorEntered)
                | Event::Mouse(mouse::Event::CursorLeft)
        ) {
            state.motion.stop();
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if let Event::Window(window::Event::RedrawRequested(now)) = event
            && state.motion.active()
        {
            let before = geometry(&mut self.content, &mut tree.children[0], layout, renderer);
            let Some((before, target, origin)) = before
                .zip(state.target)
                .zip(state.origin)
                .map(|((before, target), origin)| (before, target, origin))
            else {
                state.motion.stop();
                return;
            };
            if !before.same_viewport(target) || before.translation != target.translation {
                state.motion.stop();
                return;
            }
            let step = state.motion.advance(*now);
            if step != Vector::ZERO {
                let wheel = Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Pixels {
                        x: step.x,
                        y: step.y,
                    },
                });
                let captured = deliver(
                    &mut self.content,
                    &mut tree.children[0],
                    &wheel,
                    layout,
                    mouse::Cursor::Available(origin),
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                    false,
                );
                let after = geometry(&mut self.content, &mut tree.children[0], layout, renderer);
                if let Some(after) =
                    after.filter(|after| captured && before.accepted_step(*after, step))
                {
                    state.motion.constrain(step, before, after);
                    state.target = Some(after);
                } else {
                    state.motion.stop();
                }
            }
            if state.motion.active() {
                shell.request_redraw();
            }
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
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
        // operate 也用于只读查询；外部实际修改 offset 时由下一帧 geometry 检测。
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        let native = self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        );
        if native.is_some() {
            tree.state.downcast_mut::<State>().motion.stop();
        }
        native
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_steps_conserve_wheel_distance_and_stop_without_a_timer() {
        let now = Instant::now();
        let mut motion = Motion::default();
        let mut total = motion.input(Vector::new(0.0, -60.0), now);
        assert!(total.y < 0.0 && total.y > -60.0);
        for milliseconds in [8, 21, 37, 60, 95, 140] {
            total += motion.advance(now + Duration::from_millis(milliseconds));
        }
        assert!((total.y + 60.0).abs() < 0.001);
        assert!(!motion.active());
        assert_eq!(motion.advance(now + Duration::from_secs(2)), Vector::ZERO);
    }

    #[test]
    fn consecutive_inputs_coalesce_without_losing_distance() {
        let now = Instant::now();
        let mut motion = Motion::default();
        let first = motion.input(Vector::new(0.0, -60.0), now);
        let second = motion.input(Vector::new(0.0, -120.0), now + Duration::from_millis(25));
        let final_step = motion.advance(now + Duration::from_secs(1));
        assert!((first.y + second.y + final_step.y + 180.0).abs() < 0.001);
        assert!(!motion.active());
    }

    #[test]
    fn reversal_cancels_old_tail_and_moves_in_the_new_direction_immediately() {
        let now = Instant::now();
        let mut motion = Motion::default();
        let _ = motion.input(Vector::new(0.0, -60.0), now);
        let reversed = motion.input(Vector::new(0.0, 60.0), now + Duration::from_millis(5));
        assert!(reversed.y > 0.0);
        let remaining = motion.advance(now + Duration::from_secs(1));
        assert!((reversed.y + remaining.y - 60.0).abs() < 0.001);
    }

    #[test]
    fn native_boundary_or_direct_manipulation_stops_remaining_motion() {
        let now = Instant::now();
        let mut motion = Motion::default();
        let first = motion.input(Vector::new(0.0, -60.0), now);
        let edge = Geometry {
            bounds: Rectangle::with_size(Size::new(200.0, 120.0)),
            content: Rectangle::with_size(Size::new(200.0, 1000.0)),
            translation: Vector::new(0.0, 880.0),
        };
        motion.constrain(first, edge, edge);
        assert!(!motion.active());
        assert_eq!(motion.advance(now + MAX_DURATION), Vector::ZERO);
        let _ = motion.input(Vector::new(0.0, -60.0), now);
        motion.stop();
        assert!(!motion.active());
    }

    struct NativeScrollable {
        element: Element<'static, Message>,
        tree: Tree,
        renderer: Renderer,
        node: layout::Node,
        viewport: Rectangle,
    }

    impl NativeScrollable {
        fn new(offset: f32) -> Self {
            use iced::{
                Font, Pixels,
                widget::{scrollable, space},
            };
            let mut element = wrap(
                scrollable(space().width(200).height(1000))
                    .width(200)
                    .height(120),
            );
            let renderer = Renderer::new(Font::DEFAULT, Pixels(14.0));
            let mut tree = Tree::new(&element);
            let viewport = Rectangle::with_size(Size::new(200.0, 120.0));
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, viewport.size()),
            );
            struct Start(f32);
            impl Operation for Start {
                fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                    operate(self);
                }

                fn scrollable(
                    &mut self,
                    _id: Option<&Id>,
                    _bounds: Rectangle,
                    _content: Rectangle,
                    _translation: Vector,
                    state: &mut dyn operation::Scrollable,
                ) {
                    state.scroll_to(operation::scrollable::AbsoluteOffset {
                        x: None,
                        y: Some(self.0),
                    });
                }
            }
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut Start(offset),
            );
            Self {
                element,
                tree,
                renderer,
                node,
                viewport,
            }
        }

        fn send(&mut self, event: Event) -> bool {
            let (captured, relayout) = {
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                self.element.as_widget_mut().update(
                    &mut self.tree,
                    &event,
                    Layout::new(&self.node),
                    mouse::Cursor::Available(Point::new(50.0, 50.0)),
                    &self.renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &self.viewport,
                );
                (shell.is_event_captured(), shell.is_layout_invalid())
            };
            if relayout {
                self.node = self.element.as_widget_mut().layout(
                    &mut self.tree,
                    &self.renderer,
                    &layout::Limits::new(Size::ZERO, self.viewport.size()),
                );
            }
            captured
        }

        fn wheel(&mut self, lines: f32) {
            assert!(self.send(Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: lines },
            })));
        }

        fn offset(&mut self) -> f32 {
            // 通过真正的只读 operate 查询，不能终止排队的平滑滚动。
            let mut probe = Probe::default();
            self.element.as_widget_mut().operate(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                &mut probe,
            );
            probe.0.expect("native scrollable").translation.y
        }

        fn frames(&mut self) {
            let now = Instant::now();
            for milliseconds in [4, 8, 16, 32, 48, 64, 96, 120, 160, 200] {
                self.send(Event::Window(window::Event::RedrawRequested(
                    now + Duration::from_millis(milliseconds),
                )));
                let _ = self.offset();
            }
            assert!(!self.tree.state.downcast_ref::<State>().motion.active());
        }
    }

    #[test]
    fn native_scrollable_delivers_full_positive_and_negative_wheel_lines() {
        for (start, lines) in [(0.0, -1.0), (120.0, 1.0)] {
            let mut native = NativeScrollable::new(start);
            native.wheel(lines);
            assert!(native.tree.state.downcast_ref::<State>().motion.active());
            native.frames();
            let expected = start - lines * LINE_PIXELS;
            assert!((native.offset() - expected).abs() <= 1.0);
        }
    }

    #[test]
    fn native_scrollable_keeps_coalesced_wheels_across_read_only_operations() {
        let mut native = NativeScrollable::new(0.0);
        native.wheel(-1.0);
        let _ = native.offset();
        native.wheel(-1.0);
        native.frames();
        assert!((native.offset() - 120.0).abs() <= 1.0);
    }

    #[test]
    fn native_fractional_step_can_remain_below_one_rendered_pixel() {
        let mut native = NativeScrollable::new(100.0);
        native.wheel(-0.02);
        assert_eq!(native.offset(), 100.0);
        assert!(native.tree.state.downcast_ref::<State>().motion.active());
        native.frames();
        assert!((native.offset() - 101.2).abs() <= 1.0);
    }
}
