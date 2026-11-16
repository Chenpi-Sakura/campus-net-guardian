//! 使用 Iced canvas 呈现本地相对坐标路线预览。

use crate::running::{MapData, Point, Preview};
use iced::{
    Color, Element, Length, Rectangle, Renderer, Size, Theme,
    widget::{canvas, canvas::Path, container},
};
use std::cell::RefCell;

const DEFAULT_HEIGHT: f32 = 224.0;
const PAD: f32 = 12.0;
const SCALE_WIDTH: f32 = 56.0;
const TOP_RESERVED: f32 = 34.0;
const BOTTOM_RESERVED: f32 = 50.0;

/// 路径预览画布状态。
#[derive(Default)]
pub(super) struct Plot {
    cache: canvas::Cache,
    key: RefCell<Option<CacheKey>>,
    transform: RefCell<Option<Transform>>,
}

impl Plot {
    /// 创建自适应宽度的本地地图与路线预览；无地图、无路线时显示占位说明。
    pub(super) fn view<'a>(
        &'a self,
        preview: Option<&'a Preview>,
        progress: f32,
        revision: u64,
        height: f32,
    ) -> Element<'a, super::Message> {
        let height = if height.is_finite() {
            height.clamp(120.0, 420.0)
        } else {
            DEFAULT_HEIGHT
        };
        let Some(preview) =
            preview.filter(|preview| !preview.points.is_empty() || preview.map.is_some())
        else {
            return container(iced::widget::text("生成路线后显示预览").size(13))
                .width(Length::Fill)
                .height(height)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
        };

        canvas(PlotProgram {
            cache: &self.cache,
            key: &self.key,
            transform: &self.transform,
            preview,
            progress: finite_progress(progress / 100.0),
            revision,
        })
        .width(Length::Fill)
        .height(height)
        .into()
    }

    /// 清除路线或主题变化后缓存的基础几何。
    pub(super) fn clear(&mut self) {
        self.cache.clear();
        *self.key.borrow_mut() = None;
    }
}

struct PlotProgram<'a> {
    cache: &'a canvas::Cache,
    key: &'a RefCell<Option<CacheKey>>,
    transform: &'a RefCell<Option<Transform>>,
    preview: &'a Preview,
    progress: f32,
    revision: u64,
}

impl<Message> canvas::Program<Message> for PlotProgram<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let palette = theme.palette();
        let route_color = palette.primary;
        let reference_color = Color::from_rgba(
            palette.text.r,
            palette.text.g,
            palette.text.b,
            palette.text.a * 0.36,
        );
        let muted = Color::from_rgba(
            palette.text.r,
            palette.text.g,
            palette.text.b,
            palette.text.a * 0.62,
        );
        let map_colors = map_palette(theme);
        let colors = [
            route_color,
            reference_color,
            muted,
            palette.background,
            map_colors.0,
            map_colors.1,
            map_colors.2,
            map_colors.3,
        ];
        let key = CacheKey {
            revision: self.revision,
            width: bounds.width.to_bits(),
            height: bounds.height.to_bits(),
            colors: colors.map(color_key),
        };
        let bounds = Size::new(bounds.width, bounds.height);
        if self.key.borrow().as_ref() != Some(&key) {
            self.cache.clear();
            *self.transform.borrow_mut() = transform(self.preview, bounds);
            *self.key.borrow_mut() = Some(key);
        }
        let base = self.cache.draw(renderer, bounds, |frame| {
            draw_base(frame, self.preview, bounds, colors);
        });

        let mut frame = canvas::Frame::new(renderer, bounds);
        if let Some(position) = progress_position(self.preview, self.progress) {
            let mapped = self
                .transform
                .borrow()
                .map(|transform| map_with(position, transform))
                .unwrap_or_else(|| iced::Point::new(bounds.width * 0.5, bounds.height * 0.5));
            frame.fill(&Path::circle(mapped, 4.4), route_color);
            frame.stroke(
                &Path::circle(mapped, 6.6),
                canvas::Stroke::default()
                    .with_color(palette.background)
                    .with_width(2.0),
            );
        }
        vec![base, frame.into_geometry()]
    }
}

fn draw_base(
    frame: &mut canvas::Frame,
    preview: &Preview,
    bounds: Size,
    [
        route_color,
        reference_color,
        muted,
        background,
        building,
        green,
        water,
        road,
    ]: [Color; 8],
) {
    let Some(transform) = transform(preview, bounds) else {
        return;
    };
    if let Some(map) = &preview.map {
        draw_map(frame, map, transform, [building, green, water, road]);
    }
    let route: Vec<_> = preview
        .points
        .iter()
        .map(|point| map_with(point.position, transform))
        .collect();
    let reference: Vec<_> = preview
        .reference
        .iter()
        .map(|&point| map_with(point, transform))
        .collect();

    stroke_polyline(frame, &reference, reference_color, 2.0);
    stroke_polyline(frame, &route, route_color, 3.0);
    if let Some(&start) = route.first() {
        frame.fill(&Path::circle(start, 4.2), background);
        frame.stroke(
            &Path::circle(start, 4.2),
            canvas::Stroke::default()
                .with_color(route_color)
                .with_width(2.0),
        );
    }
    if let Some(&end) = route.last() {
        let marker = Path::new(|builder| {
            builder.move_to(iced::Point::new(end.x, end.y - 5.0));
            builder.line_to(iced::Point::new(end.x + 4.5, end.y));
            builder.line_to(iced::Point::new(end.x, end.y + 5.0));
            builder.line_to(iced::Point::new(end.x - 4.5, end.y));
            builder.line_to(iced::Point::new(end.x, end.y - 5.0));
        });
        frame.fill(&marker, route_color);
    }
    draw_scale(frame, preview, bounds, muted);
    draw_legend(frame, bounds, route_color, reference_color, muted);
    if preview
        .map
        .as_ref()
        .is_some_and(|map| map.source.contains("OpenStreetMap"))
    {
        draw_attribution(frame, bounds, muted);
    }
}

fn map_palette(theme: &Theme) -> (Color, Color, Color, Color) {
    let palette = theme.palette();
    let dark = palette.background.r + palette.background.g + palette.background.b < 1.5;
    if dark {
        (
            Color::from_rgba(0.72, 0.68, 0.58, 0.62),
            Color::from_rgba(0.26, 0.48, 0.32, 0.48),
            Color::from_rgba(0.24, 0.48, 0.65, 0.52),
            Color::from_rgba(palette.text.r, palette.text.g, palette.text.b, 0.30),
        )
    } else {
        (
            Color::from_rgba(0.78, 0.73, 0.63, 0.58),
            Color::from_rgba(0.40, 0.62, 0.43, 0.44),
            Color::from_rgba(0.42, 0.67, 0.78, 0.48),
            Color::from_rgba(0.48, 0.50, 0.50, 0.34),
        )
    }
}

fn draw_map(frame: &mut canvas::Frame, map: &MapData, transform: Transform, colors: [Color; 4]) {
    let [building, green, water, road] = colors;
    for polygon in &map.greens {
        fill_polygon(frame, polygon, transform, green);
    }
    for polygon in &map.water {
        fill_polygon(frame, polygon, transform, water);
    }
    for polygon in &map.buildings {
        fill_polygon(frame, polygon, transform, building);
    }
    for line in &map.roads {
        let points: Vec<_> = line
            .iter()
            .map(|&point| map_with(point, transform))
            .collect();
        stroke_polyline(frame, &points, road, 1.0);
    }
    if map.boundary.len() > 1 {
        let mut boundary = map.boundary.clone();
        if boundary.first() != boundary.last()
            && let Some(first) = boundary.first().copied()
        {
            boundary.push(first);
        }
        let points: Vec<_> = boundary
            .iter()
            .map(|&point| map_with(point, transform))
            .collect();
        stroke_polyline(frame, &points, road, 1.0);
    }
}

fn fill_polygon(frame: &mut canvas::Frame, polygon: &[Point], transform: Transform, color: Color) {
    if polygon.len() < 3 {
        return;
    }
    let points: Vec<_> = polygon
        .iter()
        .map(|&point| map_with(point, transform))
        .collect();
    let path = Path::new(|builder| {
        builder.move_to(points[0]);
        for &point in &points[1..] {
            builder.line_to(point);
        }
        builder.close();
    });
    frame.fill(&path, color);
}

fn draw_legend(
    frame: &mut canvas::Frame,
    bounds: Size,
    route_color: Color,
    reference_color: Color,
    color: Color,
) {
    if bounds.width < 220.0 {
        draw_legend_line(frame, 8.0, 14.0, route_color, color, "路线", 10.0);
        marker_legend(frame, 58.0, 14.0, route_color, color, 24.0);
        return;
    }
    let compact = bounds.width < 300.0;
    let y = if compact { 12.0 } else { 17.0 };
    draw_legend_line(frame, PAD, y, route_color, color, "计划路线", 10.0);
    draw_legend_line(
        frame,
        PAD + 78.0,
        y,
        reference_color,
        color,
        "参考路线",
        10.0,
    );
    let second_y = if compact { 27.0 } else { 17.0 };
    let marker_x = if compact { PAD } else { PAD + 162.0 };
    marker_legend(frame, marker_x, second_y, route_color, color, 36.0);
}

fn draw_legend_line(
    frame: &mut canvas::Frame,
    x: f32,
    y: f32,
    line_color: Color,
    text_color: Color,
    label: &str,
    size: f32,
) {
    let line = Path::new(|builder| {
        builder.move_to(iced::Point::new(x, y));
        builder.line_to(iced::Point::new(x + 14.0, y));
    });
    frame.stroke(
        &line,
        canvas::Stroke::default()
            .with_color(line_color)
            .with_width(2.5),
    );
    frame.fill_text(canvas::Text {
        content: label.into(),
        position: iced::Point::new(x + 18.0, y - 6.0),
        color: text_color,
        size: size.into(),
        ..canvas::Text::default()
    });
}

fn marker_legend(
    frame: &mut canvas::Frame,
    x: f32,
    y: f32,
    color: Color,
    text_color: Color,
    step: f32,
) {
    for (offset, label, shape) in [(0.0, "起", 0), (step, "终", 1), (step * 2.0, "现", 2)] {
        let center = iced::Point::new(x + offset + 4.0, y);
        match shape {
            0 => frame.stroke(
                &Path::circle(center, 3.0),
                canvas::Stroke::default().with_color(color).with_width(1.2),
            ),
            1 => frame.fill(&diamond(center, 3.3), color),
            _ => frame.fill(&Path::circle(center, 2.5), color),
        }
        frame.fill_text(canvas::Text {
            content: label.into(),
            position: iced::Point::new(center.x + 5.0, y - 6.0),
            color: text_color,
            size: 9.0.into(),
            ..canvas::Text::default()
        });
    }
}

fn diamond(center: iced::Point, radius: f32) -> Path {
    Path::new(|builder| {
        builder.move_to(iced::Point::new(center.x, center.y - radius));
        builder.line_to(iced::Point::new(center.x + radius, center.y));
        builder.line_to(iced::Point::new(center.x, center.y + radius));
        builder.line_to(iced::Point::new(center.x - radius, center.y));
        builder.close();
    })
}

fn draw_attribution(frame: &mut canvas::Frame, bounds: Size, color: Color) {
    if bounds.width < 220.0 {
        for (text, y) in [
            ("© OpenStreetMap", bounds.height - 23.0),
            ("contributors · ODbL", bounds.height - 11.0),
        ] {
            frame.fill_text(canvas::Text {
                content: text.into(),
                position: iced::Point::new(8.0, y),
                color,
                size: 9.0.into(),
                ..canvas::Text::default()
            });
        }
        return;
    }
    frame.fill_text(canvas::Text {
        content: if bounds.width < 220.0 {
            "© OSM contributors · ODbL".into()
        } else {
            "© OpenStreetMap contributors (ODbL)".into()
        },
        position: iced::Point::new(PAD, bounds.height - 11.0),
        color,
        size: 10.0.into(),
        ..canvas::Text::default()
    });
}

fn stroke_polyline(frame: &mut canvas::Frame, points: &[iced::Point], color: Color, width: f32) {
    if points.len() < 2 {
        return;
    }
    let path = Path::new(|builder| {
        builder.move_to(points[0]);
        for &point in &points[1..] {
            builder.line_to(point);
        }
    });
    frame.stroke(
        &path,
        canvas::Stroke::default()
            .with_color(color)
            .with_width(width),
    );
}

fn draw_scale(frame: &mut canvas::Frame, preview: &Preview, bounds: Size, color: Color) {
    let Some(transform) = transform(preview, bounds) else {
        return;
    };
    let meters = nice_scale(SCALE_WIDTH / transform.scale);
    let length = meters * transform.scale;
    let x = PAD;
    let y = bounds.height - 25.0;
    let length = length.min((bounds.width - PAD * 2.0).max(1.0));
    let path = Path::new(|builder| {
        builder.move_to(iced::Point::new(x, y - 6.0));
        builder.line_to(iced::Point::new(x + length, y - 6.0));
        builder.move_to(iced::Point::new(x, y - 9.0));
        builder.line_to(iced::Point::new(x, y - 3.0));
        builder.move_to(iced::Point::new(x + length, y - 9.0));
        builder.line_to(iced::Point::new(x + length, y - 3.0));
    });
    frame.stroke(
        &path,
        canvas::Stroke::default().with_color(color).with_width(1.5),
    );
    frame.fill_text(canvas::Text {
        content: format!("{meters:.0} 米"),
        position: iced::Point::new(x, y - 24.0),
        color,
        size: 11.0.into(),
        ..canvas::Text::default()
    });
}

#[derive(Clone, Copy)]
struct Transform {
    min_x: f64,
    max_y: f64,
    scale: f32,
    left: f32,
    top: f32,
}

fn transform(preview: &Preview, bounds: Size) -> Option<Transform> {
    let map_bounds = preview.map.as_ref().map(|map| map.bounds);
    let (min_x, max_x, min_y, max_y) = if let Some([min_x, min_y, max_x, max_y]) = map_bounds {
        if ![min_x, min_y, max_x, max_y].into_iter().all(f64::is_finite)
            || max_x <= min_x
            || max_y <= min_y
        {
            return None;
        }
        (min_x, max_x, min_y, max_y)
    } else {
        let points = preview
            .points
            .iter()
            .map(|point| point.position)
            .chain(preview.reference.iter().copied())
            .filter(|point| point.x.is_finite() && point.y.is_finite());
        point_bounds(points)?
    };
    let width = (max_x - min_x).max(1.0);
    let height = (max_y - min_y).max(1.0);
    let available_width = (bounds.width - PAD * 2.0).max(1.0);
    let available_height = (bounds.height - TOP_RESERVED - BOTTOM_RESERVED).max(1.0);
    let scale = (available_width / width as f32)
        .min(available_height / height as f32)
        .max(f32::MIN_POSITIVE);
    let drawn_width = width as f32 * scale;
    let drawn_height = height as f32 * scale;
    Some(Transform {
        min_x,
        max_y,
        scale,
        left: (bounds.width - drawn_width) * 0.5,
        top: TOP_RESERVED + ((bounds.height - TOP_RESERVED - BOTTOM_RESERVED) - drawn_height) * 0.5,
    })
}

#[cfg(test)]
fn map_point(point: Point, preview: &Preview, bounds: Size) -> iced::Point {
    let Some(transform) = transform(preview, bounds) else {
        return iced::Point::new(bounds.width * 0.5, bounds.height * 0.5);
    };
    map_with(point, transform)
}

fn map_with(point: Point, transform: Transform) -> iced::Point {
    iced::Point::new(
        transform.left + ((point.x - transform.min_x) as f32) * transform.scale,
        transform.top + ((transform.max_y - point.y) as f32) * transform.scale,
    )
}

fn point_bounds(points: impl Iterator<Item = Point>) -> Option<(f64, f64, f64, f64)> {
    points
        .filter(|point| point.x.is_finite() && point.y.is_finite())
        .fold(None, |bounds, point| {
            Some(match bounds {
                Some((min_x, max_x, min_y, max_y)) => (
                    min_x.min(point.x),
                    max_x.max(point.x),
                    min_y.min(point.y),
                    max_y.max(point.y),
                ),
                None => (point.x, point.x, point.y, point.y),
            })
        })
}

fn progress_position(preview: &Preview, progress: f32) -> Option<Point> {
    let first = preview.points.first()?;
    let last = preview.points.last()?;
    let target = first.elapsed_millis as f64
        + (last.elapsed_millis.saturating_sub(first.elapsed_millis) as f64)
            * f64::from(finite_progress(progress));
    let upper = preview
        .points
        .partition_point(|point| (point.elapsed_millis as f64) < target);
    if upper == 0 {
        return Some(first.position);
    }
    if upper >= preview.points.len() {
        return Some(last.position);
    }
    let a = &preview.points[upper - 1];
    let b = &preview.points[upper];
    let span = b.elapsed_millis.saturating_sub(a.elapsed_millis);
    let t = if span == 0 {
        0.0
    } else {
        ((target - a.elapsed_millis as f64) / span as f64).clamp(0.0, 1.0)
    };
    Some(Point {
        x: a.position.x + (b.position.x - a.position.x) * t,
        y: a.position.y + (b.position.y - a.position.y) * t,
    })
}

fn finite_progress(progress: f32) -> f32 {
    if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn nice_scale(max_meters: f32) -> f32 {
    if !max_meters.is_finite() || max_meters <= 0.0 {
        return 1.0;
    }
    let power = 10_f32.powf(max_meters.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|factor| factor * power)
        .filter(|value| *value <= max_meters)
        .max_by(f32::total_cmp)
        .unwrap_or(power)
}

fn color_key(color: Color) -> [u32; 4] {
    [color.r, color.g, color.b, color.a].map(f32::to_bits)
}

#[derive(Clone, Copy)]
struct CacheKey {
    revision: u64,
    width: u32,
    height: u32,
    colors: [[u32; 4]; 8],
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.width == other.width
            && self.height == other.height
            && self.colors == other.colors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::running::TrackPoint;

    fn preview() -> Preview {
        Preview {
            points: vec![
                TrackPoint {
                    position: Point { x: 0.0, y: 0.0 },
                    elapsed_millis: 0,
                },
                TrackPoint {
                    position: Point { x: 10.0, y: 20.0 },
                    elapsed_millis: 1000,
                },
                TrackPoint {
                    position: Point { x: 20.0, y: 0.0 },
                    elapsed_millis: 2000,
                },
            ],
            reference: vec![Point { x: 0.0, y: 0.0 }, Point { x: 20.0, y: 0.0 }],
            distance_m: 40.0,
            duration_seconds: 2,
            seed: 1,
            map: None,
        }
    }

    #[test]
    fn mapping_keeps_finite_routes_inside_canvas_margins() {
        let preview = preview();
        let size = Size::new(120.0, DEFAULT_HEIGHT);
        for point in &preview.points {
            let mapped = map_point(point.position, &preview, size);
            assert!(mapped.x.is_finite() && mapped.y.is_finite());
            assert!((0.0..=size.width).contains(&mapped.x));
            assert!((0.0..=size.height).contains(&mapped.y));
        }
    }

    #[test]
    fn progress_is_clamped_and_interpolated() {
        let preview = preview();
        assert_eq!(
            progress_position(&preview, finite_progress(-1.0 / 100.0))
                .unwrap()
                .x,
            0.0
        );
        assert_eq!(
            progress_position(&preview, finite_progress(f32::NAN))
                .unwrap()
                .x,
            0.0
        );
        let halfway = progress_position(&preview, finite_progress(25.0 / 100.0)).unwrap();
        assert_eq!(halfway, Point { x: 5.0, y: 10.0 });
        assert_eq!(
            progress_position(&preview, finite_progress(200.0 / 100.0))
                .unwrap()
                .x,
            20.0
        );
    }

    fn map_data() -> MapData {
        MapData {
            bounds: [-100.0, -80.0, 100.0, 80.0],
            boundary: vec![
                Point { x: -90.0, y: -70.0 },
                Point { x: 90.0, y: -70.0 },
                Point { x: 90.0, y: 70.0 },
                Point { x: -90.0, y: 70.0 },
            ],
            buildings: vec![vec![
                Point { x: -20.0, y: -10.0 },
                Point { x: 20.0, y: -10.0 },
                Point { x: 20.0, y: 10.0 },
                Point { x: -20.0, y: 10.0 },
            ]],
            greens: Vec::new(),
            water: Vec::new(),
            roads: vec![vec![Point { x: -80.0, y: 0.0 }, Point { x: 80.0, y: 0.0 }]],
            source: "© OpenStreetMap contributors (ODbL)".into(),
        }
    }

    #[test]
    fn map_only_preview_has_a_valid_transform() {
        let mut preview = preview();
        preview.points.clear();
        preview.reference.clear();
        preview.map = Some(map_data());
        let size = Size::new(150.0, 120.0);
        let transform = transform(&preview, size).expect("map bounds provide transform");
        let corner = map_with(Point { x: -100.0, y: 80.0 }, transform);
        let opposite = map_with(Point { x: 100.0, y: -80.0 }, transform);
        assert!(corner.x >= 0.0 && corner.y >= TOP_RESERVED);
        assert!(opposite.x <= size.width && opposite.y <= size.height - BOTTOM_RESERVED);
    }

    #[test]
    fn route_mapping_is_independent_of_track_when_map_is_present() {
        let mut first = preview();
        first.map = Some(map_data());
        let mut second = first.clone();
        second.points[1].position = Point {
            x: 900.0,
            y: -700.0,
        };
        let size = Size::new(360.0, DEFAULT_HEIGHT);
        let a = transform(&first, size).unwrap();
        let b = transform(&second, size).unwrap();
        assert_eq!(
            map_with(Point { x: 25.0, y: 15.0 }, a),
            map_with(Point { x: 25.0, y: 15.0 }, b)
        );
    }

    #[test]
    fn narrow_layout_keeps_scale_and_attribution_inside_canvas() {
        let preview = preview();
        let size = Size::new(150.0, 120.0);
        let transform = transform(&preview, size).unwrap();
        let scale_meters = nice_scale(SCALE_WIDTH / transform.scale);
        let scale_length = (scale_meters * transform.scale).min(size.width - PAD * 2.0);
        assert!(PAD + scale_length <= size.width - PAD);
        assert!(PAD + 100.0 < size.width);
        assert!(PAD + 25.0 * 4.0 < size.width);
        assert!(transform.top >= TOP_RESERVED);
    }
}
