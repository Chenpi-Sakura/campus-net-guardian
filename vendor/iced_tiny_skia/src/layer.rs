use crate::Primitive;
use crate::core::renderer::Quad;
use crate::core::{
    self, Background, Color, Point, Rectangle, Svg, Transformation,
};
use crate::graphics::damage;
use crate::graphics::layer;
use crate::graphics::text::{Editor, Paragraph, Text};
use crate::graphics::{self, Image};

use std::sync::Arc;

pub type Stack = layer::Stack<Layer>;

#[derive(Debug, Clone)]
pub struct Layer {
    pub bounds: Rectangle,
    pub quads: Vec<(Quad, Background)>,
    pub primitives: Vec<Item<Primitive>>,
    pub images: Vec<Image>,
    pub text: Vec<Item<Text>>,
}

impl Layer {
    pub fn draw_quad(
        &mut self,
        mut quad: Quad,
        background: Background,
        transformation: Transformation,
    ) {
        quad.bounds = quad.bounds * transformation;
        self.quads.push((quad, background));
    }

    pub fn draw_paragraph(
        &mut self,
        paragraph: &Paragraph,
        position: Point,
        color: Color,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        let paragraph = Text::Paragraph {
            paragraph: paragraph.downgrade(),
            position,
            color,
            clip_bounds,
            transformation,
        };

        self.text.push(Item::Live(paragraph));
    }

    pub fn draw_editor(
        &mut self,
        editor: &Editor,
        position: Point,
        color: Color,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        let editor = Text::Editor {
            editor: editor.downgrade(),
            position,
            color,
            clip_bounds,
            transformation,
        };

        self.text.push(Item::Live(editor));
    }

    pub fn draw_text(
        &mut self,
        text: core::Text,
        position: Point,
        color: Color,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        let text = Text::Cached {
            content: text.content,
            bounds: Rectangle::new(position, text.bounds) * transformation,
            color,
            size: text.size * transformation.scale_factor(),
            line_height: text.line_height.to_absolute(text.size)
                * transformation.scale_factor(),
            font: text.font,
            align_x: text.align_x,
            align_y: text.align_y,
            shaping: text.shaping,
            clip_bounds: clip_bounds * transformation,
        };

        self.text.push(Item::Live(text));
    }

    pub fn draw_text_raw(
        &mut self,
        raw: graphics::text::Raw,
        transformation: Transformation,
    ) {
        let raw = Text::Raw {
            raw,
            transformation,
        };

        self.text.push(Item::Live(raw));
    }

    pub fn draw_text_group(
        &mut self,
        text: Vec<Text>,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        self.text
            .push(Item::Group(text, clip_bounds, transformation));
    }

    pub fn draw_text_cache(
        &mut self,
        text: Arc<[Text]>,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        self.text
            .push(Item::Cached(text, clip_bounds, transformation));
    }

    pub fn draw_image(&mut self, image: Image, transformation: Transformation) {
        match image {
            Image::Raster {
                image,
                bounds,
                clip_bounds,
            } => {
                self.draw_raster(image, bounds, clip_bounds, transformation);
            }
            Image::Vector {
                svg,
                bounds,
                clip_bounds,
            } => {
                self.draw_svg(svg, bounds, clip_bounds, transformation);
            }
        }
    }

    pub fn draw_raster(
        &mut self,
        image: core::Image,
        bounds: Rectangle,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        let image = Image::Raster {
            image: core::Image {
                border_radius: image.border_radius
                    * transformation.scale_factor(),
                ..image
            },
            bounds: bounds * transformation,
            clip_bounds: clip_bounds * transformation,
        };

        self.images.push(image);
    }

    pub fn draw_svg(
        &mut self,
        svg: Svg,
        bounds: Rectangle,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        let svg = Image::Vector {
            svg,
            bounds: bounds * transformation,
            clip_bounds: clip_bounds * transformation,
        };

        self.images.push(svg);
    }

    pub fn draw_primitive_group(
        &mut self,
        primitives: Vec<Primitive>,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        self.primitives.push(Item::Group(
            primitives,
            clip_bounds * transformation,
            transformation,
        ));
    }

    pub fn draw_primitive_cache(
        &mut self,
        primitives: Arc<[Primitive]>,
        clip_bounds: Rectangle,
        transformation: Transformation,
    ) {
        self.primitives.push(Item::Cached(
            primitives,
            clip_bounds * transformation,
            transformation,
        ));
    }

    pub fn damage(previous: &Self, current: &Self) -> Vec<Rectangle> {
        if previous.bounds != current.bounds {
            return vec![previous.bounds, current.bounds];
        }

        let mut damage = damage::list(
            &previous.quads,
            &current.quads,
            |(quad, _)| {
                crate::engine::quad_visual_bounds(quad)
                    .intersection(&current.bounds)
                    .into_iter()
                    .collect()
            },
            |(quad_a, background_a), (quad_b, background_b)| {
                quad_a == quad_b && background_a == background_b
            },
        );

        let text = damage::diff(
            &previous.text,
            &current.text,
            |item| {
                item.as_slice()
                    .iter()
                    .filter_map(text_damage_bounds)
                    .map(|bounds| bounds * item.transformation())
                    .collect()
            },
            |text_a, text_b| {
                damage::list(
                    text_a.as_slice(),
                    text_b.as_slice(),
                    |text| {
                        text_damage_bounds(text)
                            .into_iter()
                            .map(|bounds| bounds * text_a.transformation())
                            .collect()
                    },
                    |text_a, text_b| text_a == text_b,
                )
            },
        );

        let primitives = damage::list(
            &previous.primitives,
            &current.primitives,
            |item| match item {
                Item::Live(primitive) => vec![primitive.visible_bounds()],
                Item::Group(primitives, group_bounds, transformation) => {
                    primitives
                        .as_slice()
                        .iter()
                        .map(Primitive::visible_bounds)
                        .map(|bounds| bounds * *transformation)
                        .filter_map(|bounds| bounds.intersection(group_bounds))
                        .collect()
                }
                Item::Cached(_primitives, bounds, _transformation) => {
                    vec![*bounds]
                }
            },
            |primitive_a, primitive_b| match (primitive_a, primitive_b) {
                (
                    Item::Cached(cache_a, bounds_a, transformation_a),
                    Item::Cached(cache_b, bounds_b, transformation_b),
                ) => {
                    Arc::ptr_eq(cache_a, cache_b)
                        && bounds_a == bounds_b
                        && transformation_a == transformation_b
                }
                _ => false,
            },
        );

        let images = damage::list(
            &previous.images,
            &current.images,
            |image| vec![image.bounds().expand(1.0)],
            Image::eq,
        );

        damage.extend(text);
        damage.extend(primitives);
        damage.extend(images);
        damage
    }
}

fn text_damage_bounds(text: &Text) -> Option<Rectangle> {
    match text {
        Text::Cached {
            bounds,
            clip_bounds,
            align_x,
            align_y,
            line_height,
            ..
        } => {
            // Direct fill_text stores an anchor; the cached draw path applies
            // alignment later. Use the available layout width conservatively
            // because the draw path aligns using the measured text width.
            let x = match align_x {
                core::text::Alignment::Default
                | core::text::Alignment::Left
                | core::text::Alignment::Justified => bounds.x,
                core::text::Alignment::Center => bounds.x - bounds.width / 2.0,
                core::text::Alignment::Right => bounds.x - bounds.width,
            };
            let height = bounds.height.max(f32::from(*line_height));
            let y = match align_y {
                core::alignment::Vertical::Top => bounds.y,
                core::alignment::Vertical::Center => bounds.y - height / 2.0,
                core::alignment::Vertical::Bottom => bounds.y - height,
            };
            Rectangle::new(
                Point::new(x, y),
                core::Size::new(bounds.width, height),
            )
            .expand(1.0)
            .intersection(clip_bounds)
        }
        // Include fractional pixel coverage and glyph antialiasing outside
        // layout bounds, matching the margin already used for quad damage.
        _ => text.visible_bounds().map(|bounds| bounds.expand(1.0)),
    }
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            bounds: Rectangle::INFINITE,
            quads: Vec::new(),
            primitives: Vec::new(),
            text: Vec::new(),
            images: Vec::new(),
        }
    }
}

impl graphics::Layer for Layer {
    fn with_bounds(bounds: Rectangle) -> Self {
        Self {
            bounds,
            ..Self::default()
        }
    }

    fn bounds(&self) -> Rectangle {
        self.bounds
    }

    fn flush(&mut self) {}

    fn resize(&mut self, bounds: Rectangle) {
        self.bounds = bounds;
    }

    fn reset(&mut self) {
        self.bounds = Rectangle::INFINITE;

        self.quads.clear();
        self.primitives.clear();
        self.text.clear();
        self.images.clear();
    }

    fn start(&self) -> usize {
        if !self.quads.is_empty() {
            return 1;
        }

        if !self.primitives.is_empty() {
            return 2;
        }

        if !self.images.is_empty() {
            return 3;
        }

        if !self.text.is_empty() {
            return 4;
        }

        usize::MAX
    }

    fn end(&self) -> usize {
        if !self.text.is_empty() {
            return 4;
        }

        if !self.images.is_empty() {
            return 3;
        }

        if !self.primitives.is_empty() {
            return 2;
        }

        if !self.quads.is_empty() {
            return 1;
        }

        0
    }

    fn merge(&mut self, layer: &mut Self) {
        self.quads.append(&mut layer.quads);
        self.primitives.append(&mut layer.primitives);
        self.text.append(&mut layer.text);
        self.images.append(&mut layer.images);
    }
}

#[derive(Debug, Clone)]
pub enum Item<T> {
    Live(T),
    Group(Vec<T>, Rectangle, Transformation),
    Cached(Arc<[T]>, Rectangle, Transformation),
}

impl<T> Item<T> {
    pub fn transformation(&self) -> Transformation {
        match self {
            Item::Live(_) => Transformation::IDENTITY,
            Item::Group(_, _, transformation)
            | Item::Cached(_, _, transformation) => *transformation,
        }
    }

    pub fn clip_bounds(&self) -> Rectangle {
        match self {
            Item::Live(_) => Rectangle::INFINITE,
            Item::Group(_, clip_bounds, _)
            | Item::Cached(_, clip_bounds, _) => *clip_bounds,
        }
    }

    pub fn as_slice(&self) -> &[T] {
        match self {
            Item::Live(item) => std::slice::from_ref(item),
            Item::Group(group, _, _) => group.as_slice(),
            Item::Cached(cache, _, _) => cache,
        }
    }
}

#[cfg(test)]
mod damage_tests {
    use super::*;
    use crate::core::{Font, Pixels, Size, alignment, text};

    fn cached(
        content: &str,
        position: Point,
        align_y: alignment::Vertical,
    ) -> Layer {
        cached_aligned(content, position, text::Alignment::Default, align_y)
    }

    fn cached_aligned(
        content: &str,
        position: Point,
        align_x: text::Alignment,
        align_y: alignment::Vertical,
    ) -> Layer {
        let text = Text::Cached {
            content: content.into(),
            bounds: Rectangle::new(position, Size::new(40.0, 16.0)),
            color: Color::BLACK,
            size: Pixels(13.0),
            line_height: Pixels(20.0),
            font: Font::DEFAULT,
            align_x,
            align_y,
            shaping: text::Shaping::Basic,
            clip_bounds: Rectangle::with_size(Size::new(1000.0, 1000.0)),
        };
        Layer {
            text: vec![Item::Live(text)],
            ..Layer::default()
        }
    }

    fn covers(region: Rectangle, expected: Rectangle) -> bool {
        region.x <= expected.x
            && region.y <= expected.y
            && region.x + region.width >= expected.x + expected.width
            && region.y + region.height >= expected.y + expected.height
    }

    #[test]
    fn fractional_text_changes_damage_antialiased_edges() {
        let position = Point::new(10.5, 20.25);
        let damage = Layer::damage(
            &cached("old", position, alignment::Vertical::Top),
            &cached("new", position, alignment::Vertical::Top),
        );
        let expected =
            Rectangle::new(position, Size::new(40.0, 20.0)).expand(1.0);
        assert!(damage.into_iter().any(|region| covers(region, expected)));
    }

    #[test]
    fn centered_cached_text_damages_old_and_new_drawn_positions() {
        let old = Point::new(10.5, 100.25);
        let new = Point::new(10.5, 75.75);
        let damage = Layer::damage(
            &cached("label", old, alignment::Vertical::Center),
            &cached("label", new, alignment::Vertical::Center),
        );
        for anchor in [old, new] {
            let drawn = Rectangle::new(
                Point::new(anchor.x, anchor.y - 10.0),
                Size::new(40.0, 20.0),
            )
            .expand(1.0);
            assert!(damage.iter().any(|region| covers(*region, drawn)));
        }
    }

    #[test]
    fn cached_alignment_keeps_top_and_bottom_anchors_distinct() {
        let position = Point::new(20.0, 100.0);
        for (alignment, top) in [
            (alignment::Vertical::Top, 100.0),
            (alignment::Vertical::Center, 90.0),
            (alignment::Vertical::Bottom, 80.0),
        ] {
            let layer = cached("label", position, alignment);
            let bounds = text_damage_bounds(&layer.text[0].as_slice()[0])
                .expect("visible text");
            assert_eq!(bounds.y, top - 1.0);
            assert_eq!(bounds.height, 22.0);
        }
    }

    #[test]
    fn cached_horizontal_alignment_covers_old_and_new_measured_draw_bounds() {
        let old = Point::new(100.5, 100.25);
        let new = Point::new(160.25, 75.75);
        for alignment in [
            text::Alignment::Default,
            text::Alignment::Left,
            text::Alignment::Justified,
            text::Alignment::Center,
            text::Alignment::Right,
        ] {
            let damage = Layer::damage(
                &cached_aligned(
                    "old label",
                    old,
                    alignment,
                    alignment::Vertical::Center,
                ),
                &cached_aligned(
                    "new label",
                    new,
                    alignment,
                    alignment::Vertical::Center,
                ),
            );
            for anchor in [old, new] {
                for measured_width in [40.0, 24.0, 8.0] {
                    // Match Pipeline::draw_cached's measured-width anchor
                    // calculation, without assuming text fills its layout.
                    let x = match alignment {
                        text::Alignment::Center => {
                            anchor.x - measured_width / 2.0
                        }
                        text::Alignment::Right => anchor.x - measured_width,
                        _ => anchor.x,
                    };
                    let drawn = Rectangle::new(
                        Point::new(x, anchor.y - 10.0),
                        Size::new(measured_width, 20.0),
                    )
                    .expand(1.0);
                    assert!(
                        damage.iter().any(|region| covers(*region, drawn)),
                        "missing {alignment:?} text damage at {anchor:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn changed_added_and_removed_quads_damage_their_full_shadows() {
        let viewport = Rectangle::with_size(Size::new(160.0, 160.0));
        let mut old = Layer {
            bounds: viewport,
            ..Layer::default()
        };
        old.quads.push((
            Quad {
                bounds: Rectangle::new(
                    Point::new(40.0, 40.0),
                    Size::new(20.0, 18.0),
                ),
                shadow: core::Shadow {
                    color: Color::BLACK,
                    offset: core::Vector::new(-7.0, 3.0),
                    blur_radius: 8.0,
                },
                ..Quad::default()
            },
            Background::Color(Color::WHITE),
        ));
        let mut new = old.clone();
        new.quads[0].0.bounds.x = 80.0;
        new.quads[0].0.bounds.y = 75.0;
        new.quads[0].0.shadow.offset = core::Vector::new(6.0, -4.0);
        new.quads[0].0.shadow.blur_radius = 10.0;
        let empty = Layer {
            bounds: viewport,
            ..Layer::default()
        };
        let old_visual =
            Rectangle::new(Point::new(24.0, 34.0), Size::new(38.0, 36.0));
        let new_visual =
            Rectangle::new(Point::new(75.0, 60.0), Size::new(42.0, 40.0));
        for (previous, current, expected) in [
            (&old, &new, vec![old_visual, new_visual]),
            (&old, &empty, vec![old_visual]),
            (&empty, &new, vec![new_visual]),
        ] {
            let damage = Layer::damage(previous, current);
            for visual in expected {
                assert!(damage.iter().any(|region| covers(*region, visual)));
            }
        }
    }
}
