use crate::Primitive;
use crate::core::renderer::Quad;
use crate::core::{
    Background, Color, Gradient, Rectangle, Size, Transformation, Vector,
};
use crate::graphics::{Image, Text};
use crate::text;

#[derive(Debug)]
pub struct Engine {
    text_pipeline: text::Pipeline,
    clip_mask: ClipMaskCache,

    #[cfg(feature = "image")]
    pub(crate) raster_pipeline: crate::raster::Pipeline,
    #[cfg(feature = "svg")]
    pub(crate) vector_pipeline: crate::vector::Pipeline,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            text_pipeline: text::Pipeline::new(),
            clip_mask: ClipMaskCache::default(),
            #[cfg(feature = "image")]
            raster_pipeline: crate::raster::Pipeline::new(),
            #[cfg(feature = "svg")]
            vector_pipeline: crate::vector::Pipeline::new(),
        }
    }

    pub fn reset_clip_mask(&mut self) {
        self.clip_mask.reset();
    }

    pub fn adjust_clip_mask(
        &mut self,
        mask: &mut tiny_skia::Mask,
        bounds: Rectangle,
    ) {
        let _ = self.clip_mask.adjust(mask, bounds);
    }

    pub fn draw_quad(
        &mut self,
        quad: &Quad,
        background: &Background,
        transformation: Transformation,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: &mut tiny_skia::Mask,
        clip_bounds: Rectangle,
    ) {
        let physical_bounds = quad.bounds * transformation;
        let visual_bounds = quad_visual_bounds(quad) * transformation;

        if !clip_bounds.intersects(&visual_bounds) {
            return;
        }

        let clip_mask = if visual_bounds.is_within(&clip_bounds) {
            None
        } else {
            self.adjust_clip_mask(clip_mask, clip_bounds);
            Some(clip_mask as &_)
        };

        let transform = into_transform(transformation);

        // Make sure the border radius is not larger than the bounds
        let border_width = quad
            .border
            .width
            .min(quad.bounds.width / 2.0)
            .min(quad.bounds.height / 2.0);

        let mut fill_border_radius = <[f32; 4]>::from(quad.border.radius);

        for radius in &mut fill_border_radius {
            *radius = (*radius)
                .min(quad.bounds.width / 2.0)
                .min(quad.bounds.height / 2.0);
        }

        let path = rounded_rectangle(quad.bounds, fill_border_radius);

        let shadow = quad.shadow;

        if let Some(shadow_bounds) = quad_shadow_bounds(quad) {
            let shadow_bounds = shadow_bounds * transformation;

            let radii = fill_border_radius
                .into_iter()
                .map(|radius| radius * transformation.scale_factor())
                .collect::<Vec<_>>();
            let x = shadow_bounds.x.floor() as i32;
            let y = shadow_bounds.y.floor() as i32;
            let right = (shadow_bounds.x + shadow_bounds.width).ceil() as i32;
            let bottom = (shadow_bounds.y + shadow_bounds.height).ceil() as i32;
            let width =
                u32::try_from(i64::from(right) - i64::from(x)).unwrap_or(0);
            let height =
                u32::try_from(i64::from(bottom) - i64::from(y)).unwrap_or(0);
            let half_width = physical_bounds.width / 2.0;
            let half_height = physical_bounds.height / 2.0;
            let blur =
                shadow.blur_radius.max(0.0) * transformation.scale_factor();

            let colors = (0..height)
                .flat_map(|row| {
                    (0..width).map(move |column| {
                        (
                            (i64::from(x) + i64::from(column)) as f32,
                            (i64::from(y) + i64::from(row)) as f32,
                        )
                    })
                })
                .filter_map(|(x, y)| {
                    tiny_skia::Size::from_wh(half_width, half_height).map(
                        |size| {
                            let shadow_distance = rounded_box_sdf(
                                Vector::new(
                                    x - physical_bounds.position().x
                                        - (shadow.offset.x
                                            * transformation.scale_factor())
                                        - half_width,
                                    y - physical_bounds.position().y
                                        - (shadow.offset.y
                                            * transformation.scale_factor())
                                        - half_height,
                                ),
                                size,
                                &radii,
                            )
                            .max(0.0);
                            let shadow_alpha = if blur > 0.0 {
                                1.0 - smoothstep(-blur, blur, shadow_distance)
                            } else if shadow_distance == 0.0 {
                                1.0
                            } else {
                                0.0
                            };

                            let mut color = into_color(shadow.color);
                            color.apply_opacity(shadow_alpha);

                            color.to_color_u8().premultiply()
                        },
                    )
                })
                .collect();

            if let Some(pixmap) = tiny_skia::IntSize::from_wh(width, height)
                .and_then(|size| {
                    tiny_skia::Pixmap::from_vec(
                        bytemuck::cast_vec(colors),
                        size,
                    )
                })
            {
                pixels.draw_pixmap(
                    x,
                    y,
                    pixmap.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    tiny_skia::Transform::default(),
                    clip_mask,
                );
            }
        }

        pixels.fill_path(
            &path,
            &tiny_skia::Paint {
                shader: match background {
                    Background::Color(color) => {
                        tiny_skia::Shader::SolidColor(into_color(*color))
                    }
                    Background::Gradient(Gradient::Linear(linear)) => {
                        let (start, end) =
                            linear.angle.to_distance(&quad.bounds);

                        let stops: Vec<tiny_skia::GradientStop> = linear
                            .stops
                            .into_iter()
                            .flatten()
                            .map(|stop| {
                                tiny_skia::GradientStop::new(
                                    stop.offset,
                                    tiny_skia::Color::from_rgba(
                                        stop.color.b,
                                        stop.color.g,
                                        stop.color.r,
                                        stop.color.a,
                                    )
                                    .expect("Create color"),
                                )
                            })
                            .collect();

                        tiny_skia::LinearGradient::new(
                            tiny_skia::Point {
                                x: start.x,
                                y: start.y,
                            },
                            tiny_skia::Point { x: end.x, y: end.y },
                            if stops.is_empty() {
                                vec![tiny_skia::GradientStop::new(
                                    0.0,
                                    tiny_skia::Color::BLACK,
                                )]
                            } else {
                                stops
                            },
                            tiny_skia::SpreadMode::Pad,
                            tiny_skia::Transform::identity(),
                        )
                        .expect("Create linear gradient")
                    }
                },
                anti_alias: true,
                ..tiny_skia::Paint::default()
            },
            tiny_skia::FillRule::EvenOdd,
            transform,
            clip_mask,
        );

        if border_width > 0.0 {
            // Border path is offset by half the border width
            let border_bounds = Rectangle {
                x: quad.bounds.x + border_width / 2.0,
                y: quad.bounds.y + border_width / 2.0,
                width: quad.bounds.width - border_width,
                height: quad.bounds.height - border_width,
            };

            // Make sure the border radius is correct
            let mut border_radius = <[f32; 4]>::from(quad.border.radius);
            let mut is_simple_border = true;

            for radius in &mut border_radius {
                *radius = if *radius == 0.0 {
                    // Path should handle this fine
                    0.0
                } else if *radius > border_width / 2.0 {
                    *radius - border_width / 2.0
                } else {
                    is_simple_border = false;
                    0.0
                }
                .min(border_bounds.width / 2.0)
                .min(border_bounds.height / 2.0);
            }

            // Stroking a path works well in this case
            if is_simple_border {
                let border_path =
                    rounded_rectangle(border_bounds, border_radius);

                pixels.stroke_path(
                    &border_path,
                    &tiny_skia::Paint {
                        shader: tiny_skia::Shader::SolidColor(into_color(
                            quad.border.color,
                        )),
                        anti_alias: true,
                        ..tiny_skia::Paint::default()
                    },
                    &tiny_skia::Stroke {
                        width: border_width,
                        ..tiny_skia::Stroke::default()
                    },
                    transform,
                    clip_mask,
                );
            } else {
                // Draw corners that have too small border radii as having no border radius,
                // but mask them with the rounded rectangle with the correct border radius.
                let mut temp_pixmap = tiny_skia::Pixmap::new(
                    quad.bounds.width as u32,
                    quad.bounds.height as u32,
                )
                .unwrap();

                let mut quad_mask = tiny_skia::Mask::new(
                    quad.bounds.width as u32,
                    quad.bounds.height as u32,
                )
                .unwrap();

                let zero_bounds = Rectangle {
                    x: 0.0,
                    y: 0.0,
                    width: quad.bounds.width,
                    height: quad.bounds.height,
                };
                let path = rounded_rectangle(zero_bounds, fill_border_radius);

                quad_mask.fill_path(
                    &path,
                    tiny_skia::FillRule::EvenOdd,
                    true,
                    transform,
                );
                let path_bounds = Rectangle {
                    x: border_width / 2.0,
                    y: border_width / 2.0,
                    width: quad.bounds.width - border_width,
                    height: quad.bounds.height - border_width,
                };

                let border_radius_path =
                    rounded_rectangle(path_bounds, border_radius);

                temp_pixmap.stroke_path(
                    &border_radius_path,
                    &tiny_skia::Paint {
                        shader: tiny_skia::Shader::SolidColor(into_color(
                            quad.border.color,
                        )),
                        anti_alias: true,
                        ..tiny_skia::Paint::default()
                    },
                    &tiny_skia::Stroke {
                        width: border_width,
                        ..tiny_skia::Stroke::default()
                    },
                    transform,
                    Some(&quad_mask),
                );

                pixels.draw_pixmap(
                    quad.bounds.x as i32,
                    quad.bounds.y as i32,
                    temp_pixmap.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    transform,
                    clip_mask,
                );
            }
        }
    }

    pub fn draw_text(
        &mut self,
        text: &Text,
        transformation: Transformation,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: &mut tiny_skia::Mask,
        clip_bounds: Rectangle,
    ) {
        match text {
            Text::Paragraph {
                paragraph,
                position,
                color,
                clip_bounds: local_clip_bounds,
                transformation: local_transformation,
            } => {
                let transformation = transformation * *local_transformation;
                let Some(clip_bounds) = clip_bounds
                    .intersection(&(*local_clip_bounds * transformation))
                else {
                    return;
                };

                let physical_bounds =
                    Rectangle::new(*position, paragraph.min_bounds)
                        * transformation;

                if !clip_bounds.intersects(&physical_bounds) {
                    return;
                }

                let clip_mask = match physical_bounds.is_within(&clip_bounds) {
                    true => None,
                    false => {
                        self.adjust_clip_mask(clip_mask, clip_bounds);
                        Some(clip_mask as &_)
                    }
                };

                self.text_pipeline.draw_paragraph(
                    paragraph,
                    *position,
                    *color,
                    pixels,
                    clip_mask,
                    transformation,
                );
            }
            Text::Editor {
                editor,
                position,
                color,
                clip_bounds: local_clip_bounds,
                transformation: local_transformation,
            } => {
                let transformation = transformation * *local_transformation;
                let Some(clip_bounds) = clip_bounds
                    .intersection(&(*local_clip_bounds * transformation))
                else {
                    return;
                };

                let physical_bounds =
                    Rectangle::new(*position, editor.bounds) * transformation;

                if !clip_bounds.intersects(&physical_bounds) {
                    return;
                }

                let clip_mask = match physical_bounds.is_within(&clip_bounds) {
                    true => None,
                    false => {
                        self.adjust_clip_mask(clip_mask, clip_bounds);
                        Some(clip_mask as &_)
                    }
                };

                self.text_pipeline.draw_editor(
                    editor,
                    *position,
                    *color,
                    pixels,
                    clip_mask,
                    transformation,
                );
            }
            Text::Cached {
                content,
                bounds,
                color,
                size,
                line_height,
                font,
                align_x,
                align_y,
                shaping,
                clip_bounds: local_clip_bounds,
            } => {
                let Some(clip_bounds) = clip_bounds
                    .intersection(&(*local_clip_bounds * transformation))
                else {
                    return;
                };

                // Cached text bounds are anchors, not measured glyph extents.
                // A viewport contained in the layer does not prove that all
                // glyphs are inside it (e.g. a scrolled pick-list label).
                self.adjust_clip_mask(clip_mask, clip_bounds);

                self.text_pipeline.draw_cached(
                    content,
                    *bounds,
                    *color,
                    *size,
                    *line_height,
                    *font,
                    *align_x,
                    *align_y,
                    *shaping,
                    pixels,
                    Some(clip_mask),
                    transformation,
                );
            }
            Text::Raw {
                raw,
                transformation: local_transformation,
            } => {
                let Some(buffer) = raw.buffer.upgrade() else {
                    return;
                };

                let transformation = transformation * *local_transformation;
                let (width, height) = buffer.size();

                let physical_bounds = Rectangle::new(
                    raw.position,
                    Size::new(
                        width.unwrap_or(clip_bounds.width),
                        height.unwrap_or(clip_bounds.height),
                    ),
                ) * transformation;

                if !clip_bounds.intersects(&physical_bounds) {
                    return;
                }

                let clip_mask = (!physical_bounds.is_within(&clip_bounds))
                    .then_some(clip_mask as &_);

                self.text_pipeline.draw_raw(
                    &buffer,
                    raw.position,
                    raw.color,
                    pixels,
                    clip_mask,
                    transformation,
                );
            }
        }
    }

    pub fn draw_primitive(
        &mut self,
        primitive: &Primitive,
        transformation: Transformation,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: &mut tiny_skia::Mask,
        clip_bounds: Rectangle,
    ) {
        match primitive {
            Primitive::Fill { path, paint, rule } => {
                let physical_bounds = {
                    let bounds = path.bounds();

                    Rectangle {
                        x: bounds.x(),
                        y: bounds.y(),
                        width: bounds.width(),
                        height: bounds.height(),
                    } * transformation
                };

                if !clip_bounds.intersects(&physical_bounds) {
                    return;
                }

                let clip_mask = (!physical_bounds.is_within(&clip_bounds))
                    .then_some(clip_mask as &_);

                pixels.fill_path(
                    path,
                    paint,
                    *rule,
                    into_transform(transformation),
                    clip_mask,
                );
            }
            Primitive::Stroke {
                path,
                paint,
                stroke,
            } => {
                let physical_bounds = {
                    let bounds = path.bounds();

                    Rectangle {
                        x: bounds.x() - stroke.width / 2.0,
                        y: bounds.y() - stroke.width / 2.0,
                        width: bounds.width() + stroke.width,
                        height: bounds.height() + stroke.width,
                    } * transformation
                };

                if !clip_bounds.intersects(&physical_bounds) {
                    return;
                }

                let clip_mask = (!physical_bounds.is_within(&clip_bounds))
                    .then_some(clip_mask as &_);

                pixels.stroke_path(
                    path,
                    paint,
                    stroke,
                    into_transform(transformation),
                    clip_mask,
                );
            }
        }
    }

    pub fn draw_image(
        &mut self,
        image: &Image,
        _transformation: Transformation,
        _pixels: &mut tiny_skia::PixmapMut<'_>,
        _clip_mask: &mut tiny_skia::Mask,
        _clip_bounds: Rectangle,
    ) {
        match image {
            #[cfg(feature = "image")]
            Image::Raster { image, bounds, .. } => {
                let physical_bounds = *bounds * _transformation;

                if !_clip_bounds.intersects(&physical_bounds) {
                    return;
                }

                let clip_mask = (!physical_bounds.is_within(&_clip_bounds))
                    .then_some(_clip_mask as &_);

                let center = physical_bounds.center();
                let radians = f32::from(image.rotation);

                let transform = into_transform(_transformation).post_rotate_at(
                    radians.to_degrees(),
                    center.x,
                    center.y,
                );

                self.raster_pipeline.draw(
                    &image.handle,
                    image.filter_method,
                    *bounds,
                    image.opacity,
                    _pixels,
                    transform,
                    clip_mask,
                );
            }
            #[cfg(feature = "svg")]
            Image::Vector { svg, bounds, .. } => {
                let physical_bounds = *bounds * _transformation;

                if !_clip_bounds.intersects(&physical_bounds) {
                    return;
                }

                let clip_mask = (!physical_bounds.is_within(&_clip_bounds))
                    .then_some(_clip_mask as &_);

                let center = physical_bounds.center();
                let radians = f32::from(svg.rotation);

                let transform = into_transform(_transformation).post_rotate_at(
                    radians.to_degrees(),
                    center.x,
                    center.y,
                );

                self.vector_pipeline.draw(
                    &svg.handle,
                    svg.color,
                    *bounds,
                    svg.opacity,
                    _pixels,
                    transform,
                    clip_mask,
                );
            }
            #[cfg(not(feature = "image"))]
            Image::Raster { .. } => {
                log::warn!(
                    "Unsupported primitive in `iced_tiny_skia`: {image:?}",
                );
            }
            #[cfg(not(feature = "svg"))]
            Image::Vector { .. } => {
                log::warn!(
                    "Unsupported primitive in `iced_tiny_skia`: {image:?}",
                );
            }
        }
    }

    pub fn trim(&mut self) {
        self.text_pipeline.trim_cache();

        #[cfg(feature = "image")]
        self.raster_pipeline.trim_cache();

        #[cfg(feature = "svg")]
        self.vector_pipeline.trim_cache();
    }
}

pub(crate) fn quad_shadow_bounds(quad: &Quad) -> Option<Rectangle> {
    if quad.shadow.color.a <= 0.0 {
        return None;
    }
    let blur = quad.shadow.blur_radius.max(0.0);
    Some(Rectangle {
        x: quad.bounds.x + quad.shadow.offset.x - blur,
        y: quad.bounds.y + quad.shadow.offset.y - blur,
        width: quad.bounds.width + blur * 2.0,
        height: quad.bounds.height + blur * 2.0,
    })
}

pub(crate) fn quad_visual_bounds(quad: &Quad) -> Rectangle {
    quad_shadow_bounds(quad)
        .map_or(quad.bounds, |shadow| quad.bounds.union(&shadow))
        .expand(1.0)
}

pub fn into_color(color: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(color.b, color.g, color.r, color.a)
        .expect("Convert color from iced to tiny_skia")
}

fn into_transform(transformation: Transformation) -> tiny_skia::Transform {
    let translation = transformation.translation();

    tiny_skia::Transform {
        sx: transformation.scale_factor(),
        kx: 0.0,
        ky: 0.0,
        sy: transformation.scale_factor(),
        tx: translation.x,
        ty: translation.y,
    }
}

fn rounded_rectangle(
    bounds: Rectangle,
    border_radius: [f32; 4],
) -> tiny_skia::Path {
    let [top_left, top_right, bottom_right, bottom_left] = border_radius;

    if top_left == 0.0
        && top_right == 0.0
        && bottom_right == 0.0
        && bottom_left == 0.0
    {
        return tiny_skia::PathBuilder::from_rect(
            tiny_skia::Rect::from_xywh(
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
            )
            .expect("Build quad rectangle"),
        );
    }

    if top_left == top_right
        && top_left == bottom_right
        && top_left == bottom_left
        && top_left == bounds.width / 2.0
        && top_left == bounds.height / 2.0
    {
        return tiny_skia::PathBuilder::from_circle(
            bounds.x + bounds.width / 2.0,
            bounds.y + bounds.height / 2.0,
            top_left,
        )
        .expect("Build circle path");
    }

    let mut builder = tiny_skia::PathBuilder::new();

    builder.move_to(bounds.x + top_left, bounds.y);
    builder.line_to(bounds.x + bounds.width - top_right, bounds.y);

    if top_right > 0.0 {
        arc_to(
            &mut builder,
            bounds.x + bounds.width - top_right,
            bounds.y,
            bounds.x + bounds.width,
            bounds.y + top_right,
            top_right,
        );
    }

    maybe_line_to(
        &mut builder,
        bounds.x + bounds.width,
        bounds.y + bounds.height - bottom_right,
    );

    if bottom_right > 0.0 {
        arc_to(
            &mut builder,
            bounds.x + bounds.width,
            bounds.y + bounds.height - bottom_right,
            bounds.x + bounds.width - bottom_right,
            bounds.y + bounds.height,
            bottom_right,
        );
    }

    maybe_line_to(
        &mut builder,
        bounds.x + bottom_left,
        bounds.y + bounds.height,
    );

    if bottom_left > 0.0 {
        arc_to(
            &mut builder,
            bounds.x + bottom_left,
            bounds.y + bounds.height,
            bounds.x,
            bounds.y + bounds.height - bottom_left,
            bottom_left,
        );
    }

    maybe_line_to(&mut builder, bounds.x, bounds.y + top_left);

    if top_left > 0.0 {
        arc_to(
            &mut builder,
            bounds.x,
            bounds.y + top_left,
            bounds.x + top_left,
            bounds.y,
            top_left,
        );
    }

    builder.finish().expect("Build rounded rectangle path")
}

fn maybe_line_to(path: &mut tiny_skia::PathBuilder, x: f32, y: f32) {
    if path.last_point() != Some(tiny_skia::Point { x, y }) {
        path.line_to(x, y);
    }
}

fn arc_to(
    path: &mut tiny_skia::PathBuilder,
    x_from: f32,
    y_from: f32,
    x_to: f32,
    y_to: f32,
    radius: f32,
) {
    let svg_arc = kurbo::SvgArc {
        from: kurbo::Point::new(f64::from(x_from), f64::from(y_from)),
        to: kurbo::Point::new(f64::from(x_to), f64::from(y_to)),
        radii: kurbo::Vec2::new(f64::from(radius), f64::from(radius)),
        x_rotation: 0.0,
        large_arc: false,
        sweep: true,
    };

    match kurbo::Arc::from_svg_arc(&svg_arc) {
        Some(arc) => {
            arc.to_cubic_beziers(0.1, |p1, p2, p| {
                path.cubic_to(
                    p1.x as f32,
                    p1.y as f32,
                    p2.x as f32,
                    p2.y as f32,
                    p.x as f32,
                    p.y as f32,
                );
            });
        }
        None => {
            path.line_to(x_to, y_to);
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let x = ((x - a) / (b - a)).clamp(0.0, 1.0);

    x * x * (3.0 - 2.0 * x)
}

fn rounded_box_sdf(
    to_center: Vector,
    size: tiny_skia::Size,
    radii: &[f32],
) -> f32 {
    let radius = match (to_center.x > 0.0, to_center.y > 0.0) {
        (true, true) => radii[2],
        (true, false) => radii[1],
        (false, true) => radii[3],
        (false, false) => radii[0],
    };

    let x = (to_center.x.abs() - size.width() + radius).max(0.0);
    let y = (to_center.y.abs() - size.height() + radius).max(0.0);

    (x.powf(2.0) + y.powf(2.0)).sqrt() - radius
}

#[derive(Debug, Default)]
struct ClipMaskCache {
    bounds: Option<Rectangle>,
}

impl ClipMaskCache {
    fn reset(&mut self) {
        self.bounds = None;
    }

    fn adjust(
        &mut self,
        mask: &mut tiny_skia::Mask,
        bounds: Rectangle,
    ) -> bool {
        if self.bounds == Some(bounds) {
            return false;
        }
        adjust_clip_mask(mask, bounds);
        self.bounds = Some(bounds);
        true
    }
}

fn adjust_clip_mask(clip_mask: &mut tiny_skia::Mask, bounds: Rectangle) {
    clip_mask.clear();

    let path = {
        let mut builder = tiny_skia::PathBuilder::new();
        builder.push_rect(
            tiny_skia::Rect::from_xywh(
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
            )
            .unwrap(),
        );

        builder.finish().unwrap()
    };

    clip_mask.fill_path(
        &path,
        tiny_skia::FillRule::EvenOdd,
        false,
        tiny_skia::Transform::default(),
    );
}

#[cfg(test)]
mod clip_mask_tests {
    use super::*;
    use crate::core::{Font, Pixels, Point, Size, alignment, text};

    fn bounds(x: f32, y: f32) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(2.0, 2.0))
    }

    #[test]
    fn same_bounds_reuse_the_mask_until_the_draw_is_reset() {
        let mut cache = ClipMaskCache::default();
        let mut mask = tiny_skia::Mask::new(8, 8).unwrap();
        assert!(cache.adjust(&mut mask, bounds(1.0, 1.0)));
        let pixels = mask.data().to_vec();
        assert!(!cache.adjust(&mut mask, bounds(1.0, 1.0)));
        assert_eq!(mask.data(), pixels);
        cache.reset();
        assert!(cache.adjust(&mut mask, bounds(1.0, 1.0)));
        assert_eq!(mask.data(), pixels);
    }

    #[test]
    fn changed_bounds_clear_old_pixels_before_filling_new_rectangle() {
        let mut cache = ClipMaskCache::default();
        let mut mask = tiny_skia::Mask::new(8, 8).unwrap();
        assert!(cache.adjust(&mut mask, bounds(1.0, 1.0)));
        assert!(cache.adjust(&mut mask, bounds(4.0, 4.0)));
        assert_eq!(mask.data()[1 + 8], 0);
        assert_eq!(mask.data()[4 + 4 * 8], 255);
    }

    #[test]
    fn reset_prevents_stale_memo_when_next_draw_uses_a_different_mask() {
        let mut cache = ClipMaskCache::default();
        let mut first = tiny_skia::Mask::new(8, 8).unwrap();
        assert!(cache.adjust(&mut first, bounds(1.0, 1.0)));
        let mut second = tiny_skia::Mask::new(8, 8).unwrap();
        second.data_mut().fill(255);
        cache.reset();
        assert!(cache.adjust(&mut second, bounds(1.0, 1.0)));
        assert_eq!(second.data(), first.data());
    }

    fn rasterize_cached_label(
        layer_clip: Rectangle,
        local_clip: Rectangle,
    ) -> tiny_skia::Pixmap {
        let mut pixels = tiny_skia::Pixmap::new(96, 64).unwrap();
        let mut mask = tiny_skia::Mask::new(96, 64).unwrap();
        // This font is embedded by iced_graphics on every platform, so the
        // test cannot pass vacuously because a machine has no system fonts.
        let label = Text::Cached {
            content: "\u{e800}\u{e800}\u{e800}".into(),
            bounds: Rectangle::new(
                Point::new(4.5, 24.5),
                Size::new(80.0, 24.0),
            ),
            color: Color::BLACK,
            size: Pixels(24.0),
            line_height: Pixels(24.0),
            font: Font::with_name("Iced-Icons"),
            align_x: text::Alignment::Default,
            align_y: alignment::Vertical::Center,
            shaping: text::Shaping::Basic,
            clip_bounds: local_clip,
        };
        Engine::new().draw_text(
            &label,
            Transformation::IDENTITY,
            &mut pixels.as_mut(),
            &mut mask,
            layer_clip,
        );
        pixels
    }

    #[test]
    fn scrolled_cached_label_preserves_header_and_left_of_viewport_pixels() {
        let full = Rectangle::with_size(Size::new(96.0, 64.0));
        let original = rasterize_cached_label(full, full);
        let ink: Vec<_> = original
            .pixels()
            .iter()
            .enumerate()
            .filter(|(_, pixel)| pixel.alpha() > 0)
            .map(|(index, _)| (index as u32 % 96, index as u32 / 96))
            .collect();
        assert!(
            !ink.is_empty(),
            "embedded label must contain visible glyphs"
        );
        let min_x = ink.iter().map(|(x, _)| *x).min().unwrap();
        let max_x = ink.iter().map(|(x, _)| *x).max().unwrap();
        let min_y = ink.iter().map(|(_, y)| *y).min().unwrap();
        let max_y = ink.iter().map(|(_, y)| *y).max().unwrap();
        let left = (min_x + max_x) / 2;
        let top = (min_y + max_y) / 2;
        assert!(ink.iter().any(|(x, y)| *x >= left && *y >= top));
        assert!(ink.iter().any(|(x, y)| *x < left && *y < top));
        let viewport = Rectangle::new(
            Point::new(left as f32, top as f32),
            Size::new((96 - left) as f32, (64 - top) as f32),
        );
        // Both equality and containment used to bypass the glyph clip mask.
        for layer_clip in [viewport, full] {
            let clipped = rasterize_cached_label(layer_clip, viewport);
            for (index, pixel) in clipped.pixels().iter().enumerate() {
                let x = index as u32 % 96;
                let y = index as u32 / 96;
                if x < left || y < top {
                    assert_eq!(pixel.alpha(), 0, "clipped glyph at {x},{y}");
                } else {
                    assert_eq!(pixel, &original.pixels()[index]);
                }
            }
        }
    }
}

#[cfg(test)]
mod shadow_tests {
    use super::*;
    use crate::core::{Border, Point, Shadow};

    fn shadow_quad(position: Point) -> Quad {
        Quad {
            bounds: Rectangle::new(position, Size::new(24.0, 20.0)),
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            shadow: Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
                offset: Vector::new(0.0, 3.0),
                blur_radius: 8.0,
            },
            ..Quad::default()
        }
    }

    fn render_quad(
        pixels: &mut tiny_skia::Pixmap,
        quad: &Quad,
        clip: Rectangle,
    ) {
        let mut mask =
            tiny_skia::Mask::new(pixels.width(), pixels.height()).unwrap();
        let mut engine = Engine::new();
        engine.draw_quad(
            quad,
            &Background::Color(Color::WHITE),
            Transformation::IDENTITY,
            &mut pixels.as_mut(),
            &mut mask,
            clip,
        );
    }

    fn white_pixels(width: u32, height: u32) -> tiny_skia::Pixmap {
        let mut pixels = tiny_skia::Pixmap::new(width, height).unwrap();
        pixels.fill(tiny_skia::Color::WHITE);
        pixels
    }

    #[test]
    fn repeated_partial_redraw_does_not_darken_shadow_outside_damage() {
        let quad = shadow_quad(Point::new(24.0, 24.0));
        let mut pixels = white_pixels(96, 80);
        render_quad(
            &mut pixels,
            &quad,
            Rectangle::with_size(Size::new(96.0, 80.0)),
        );
        let original = pixels.clone();
        assert!(original.pixels().iter().any(|pixel| pixel.red() < 255));
        let damage =
            Rectangle::new(Point::new(30.0, 28.0), Size::new(8.0, 8.0));
        for _ in 0..5 {
            // Match the compositor's Source clear of only the damaged area.
            for y in 28..36 {
                for x in 30..38 {
                    let index = (y * 96 + x) * 4;
                    pixels.data_mut()[index..index + 4].fill(255);
                }
            }
            render_quad(&mut pixels, &quad, damage);
            assert_eq!(pixels.data(), original.data());
        }
    }

    #[test]
    fn damage_that_only_intersects_shadow_is_drawn_and_clipped() {
        let quad = shadow_quad(Point::new(24.0, 24.0));
        let full = Rectangle::with_size(Size::new(96.0, 80.0));
        let damage =
            Rectangle::new(Point::new(17.0, 30.0), Size::new(5.0, 6.0));
        assert!(!damage.intersects(&quad.bounds));
        let mut original = white_pixels(96, 80);
        render_quad(&mut original, &quad, full);
        let mut partial = white_pixels(96, 80);
        render_quad(&mut partial, &quad, damage);
        let mut visible_shadow = false;
        for (index, pixel) in partial.pixels().iter().enumerate() {
            let position = Point::new((index % 96) as f32, (index / 96) as f32);
            if damage.contains(position) {
                assert_eq!(pixel, &original.pixels()[index]);
                visible_shadow |= pixel.red() < 255;
            } else {
                assert_eq!(pixel.red(), 255);
                assert_eq!(pixel.green(), 255);
                assert_eq!(pixel.blue(), 255);
                assert_eq!(pixel.alpha(), 255);
            }
        }
        assert!(visible_shadow, "shadow-only damage must not be culled");
    }

    #[test]
    fn negative_shadow_origin_matches_a_crop_of_translated_scene() {
        let mut quad = shadow_quad(Point::new(2.0, 2.0));
        quad.shadow.offset = Vector::new(-7.0, -9.0);
        let mut clipped = white_pixels(96, 80);
        render_quad(
            &mut clipped,
            &quad,
            Rectangle::with_size(Size::new(96.0, 80.0)),
        );
        quad.bounds.x += 24.0;
        quad.bounds.y += 24.0;
        let mut translated = white_pixels(128, 112);
        render_quad(
            &mut translated,
            &quad,
            Rectangle::new(Point::new(24.0, 24.0), Size::new(96.0, 80.0)),
        );
        // Translate the viewport too, keeping the mask path consistent.
        // tiny-skia's signed pixmap clipping can quantize the boundary by
        // one RGB level (1/255); alpha and geometric placement stay exact.
        for y in 0..80 {
            for x in 0..96 {
                let actual = clipped.pixels()[y * 96 + x];
                let expected = translated.pixels()[(y + 24) * 128 + x + 24];
                assert_eq!(
                    actual.alpha(),
                    expected.alpha(),
                    "negative shadow alpha at {x},{y}"
                );
                for (actual, expected) in [
                    (actual.red(), expected.red()),
                    (actual.green(), expected.green()),
                    (actual.blue(), expected.blue()),
                ] {
                    assert!(
                        actual.abs_diff(expected) <= 1,
                        "negative shadow RGB at {x},{y}: {actual} vs {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn zero_blur_shadow_remains_visible_without_nan_opacity() {
        let mut quad = shadow_quad(Point::new(24.0, 24.0));
        quad.shadow.blur_radius = 0.0;
        let mut pixels = white_pixels(96, 80);
        render_quad(
            &mut pixels,
            &quad,
            Rectangle::with_size(Size::new(96.0, 80.0)),
        );
        assert!(pixels.pixels()[46 * 96 + 36].red() < 255);
    }
}
