//! 固定在滚动视口顶部的细渐变，不改变内容布局、滚动位置或事件坐标。

use super::Message;
use iced::{
    Color, Element, Event, Gradient, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    gradient,
};

/// 为滚动视口顶部加八像素淡出，保留右侧滚动条区域。
pub(super) fn wrap<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Edge {
        content: content.into(),
    })
}

struct Edge<'a> {
    content: Element<'a, Message>,
}

impl Widget<Message, Theme, Renderer> for Edge<'_> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }
    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }
    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }
    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
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
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
        let bounds = layout.bounds();
        let edge = Rectangle {
            width: (bounds.width - 11.0).max(0.0),
            height: bounds.height.min(8.0),
            ..bounds
        };
        if let Some(visible) = edge.intersection(viewport) {
            let color = theme.palette().background;
            let gradient = gradient::Linear::new(iced::Radians::PI)
                .add_stop(0.0, color)
                .add_stop(1.0, Color { a: 0.0, ..color });
            renderer.with_layer(visible, |renderer| {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: edge,
                        snap: false,
                        ..Default::default()
                    },
                    Gradient::Linear(gradient),
                );
            });
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
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }
    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}
