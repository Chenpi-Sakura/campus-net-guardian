//! 纯本地合成路线与运动轨迹预览，不代表真实校区或地理位置。

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_POINTS: usize = 50_000;
const MAX_STEPS: usize = 200_000;

/// 使用相对米坐标表示的平面位置，不是经纬度。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// 相对坐标的横轴，单位为米。
    pub x: f64,
    /// 相对坐标的纵轴，单位为米。
    pub y: f64,
}
/// 合成路线图中的节点及其邻接节点索引。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    /// 节点的相对米坐标。
    pub point: Point,
    /// 可行走的邻接节点索引。
    pub edges: Vec<usize>,
}
/// 可选的合成闭合路线。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutePreset {
    /// 路线唯一标识。
    pub id: String,
    /// 面向用户显示的路线名称。
    pub name: String,
    /// 按行走顺序排列的节点索引，末尾可重复首节点。
    pub nodes: Vec<usize>,
}
/// 合成校区图及路线选项。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Campus {
    /// 校区唯一标识。
    pub id: String,
    /// 校区名称。
    pub name: String,
    /// 合成图节点。
    pub nodes: Vec<Node>,
    /// 可选预设路线。
    pub routes: Vec<RoutePreset>,
    /// 可选的本地地图绘制数据。
    #[serde(default)]
    pub map: Option<MapData>,
}
/// 校区地图的本地平面几何数据。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapData {
    /// 地图范围，依次为最小横坐标、最小纵坐标、最大横坐标、最大纵坐标。
    pub bounds: [f64; 4],
    /// 校区边界多边形。
    pub boundary: Vec<Point>,
    /// 建筑物轮廓集合。
    pub buildings: Vec<Vec<Point>>,
    /// 绿地轮廓集合。
    pub greens: Vec<Vec<Point>>,
    /// 水域轮廓集合。
    pub water: Vec<Vec<Point>>,
    /// 道路线条集合。
    pub roads: Vec<Vec<Point>>,
    /// 地图数据来源说明。
    pub source: String,
}
/// 供界面选择的合成校区目录。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    /// 可供选择的校区。
    pub campuses: Vec<Campus>,
}
/// 合成扰动等级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    /// 不扰动。
    Off,
    /// 轻微扰动。
    Light,
    /// 中等扰动。
    Medium,
}
impl Noise {
    /// 全部支持的扰动等级。
    pub const ALL: &'static [Self] = &[Self::Off, Self::Light, Self::Medium];
}
impl fmt::Display for Noise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "关闭",
            Self::Light => "轻微（本地模拟）",
            Self::Medium => "中等（本地模拟）",
        })
    }
}
/// 合成路线生成参数。
#[derive(Clone, Debug)]
pub struct GenerateRequest {
    /// 可选预设路线 ID；空值表示随机图行走。
    pub preset_id: Option<String>,
    /// 目标距离，范围 100 至 10000 米。
    pub distance_m: u32,
    /// 目标时长，范围 60 至 10800 秒。
    pub duration_seconds: u32,
    /// 本地合成扰动强度。
    pub noise: Noise,
    /// 可复现的伪随机种子。
    pub seed: u64,
}
/// 轨迹中的单个相对位置及其经过时间。
#[derive(Clone, Debug)]
pub struct TrackPoint {
    /// 经过有限微扰后的相对位置。
    pub position: Point,
    /// 从开始起算的毫秒数。
    pub elapsed_millis: u64,
}
/// 生成的合成轨迹和参考路线示意。
#[derive(Clone, Debug)]
pub struct Preview {
    /// 含起点和终点的合成轨迹。
    pub points: Vec<TrackPoint>,
    /// 用于示意的原始相对路线。
    pub reference: Vec<Point>,
    /// 生成轨迹重算所得距离，单位为米。
    pub distance_m: f64,
    /// 请求时长，单位为秒。
    pub duration_seconds: u32,
    /// 使用的随机种子。
    pub seed: u64,
    /// 可选的本地校区地图数据。
    pub map: Option<MapData>,
}

/// 加载并校验随程序内置的校区地图目录。
pub fn builtin_catalog() -> Result<Catalog> {
    let catalog: Catalog = serde_json::from_str(include_str!("../../assets/campus/maps.json"))?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

/// 返回明确标注为合成数据的两个示例校区。
pub fn demo_catalog() -> Catalog {
    fn campus(id: &str, origin: (f64, f64), scale: f64) -> Campus {
        let xy = [
            (0., 0.),
            (120., 0.),
            (120., 90.),
            (0., 90.),
            (60., -55.),
            (180., 45.),
        ];
        let links = [
            vec![1, 3, 4],
            vec![0, 2, 4, 5],
            vec![1, 3, 5],
            vec![0, 2],
            vec![0, 1],
            vec![1, 2],
        ];
        Campus {
            id: id.into(),
            name: format!(
                "示例校区 {}（合成）",
                if id == "demo-a" { "A" } else { "B" }
            ),
            nodes: xy
                .iter()
                .zip(links)
                .map(|(&(x, y), edges)| Node {
                    point: Point {
                        x: origin.0 + x * scale,
                        y: origin.1 + y * scale,
                    },
                    edges,
                })
                .collect(),
            routes: vec![
                RoutePreset {
                    id: "loop-a".into(),
                    name: "环线甲".into(),
                    nodes: vec![0, 1, 2, 3, 0],
                },
                RoutePreset {
                    id: "loop-b".into(),
                    name: "环线乙".into(),
                    nodes: vec![0, 4, 1, 5, 2, 3, 0],
                },
            ],
            map: None,
        }
    }
    Catalog {
        campuses: vec![
            campus("demo-a", (0., 0.), 1.),
            campus("demo-b", (400., 250.), 1.4),
        ],
    }
}

/// 校验合成图结构、唯一标识、边及路线连通性。
pub fn validate_catalog(catalog: &Catalog) -> Result<()> {
    if catalog.campuses.is_empty() || catalog.campuses.len() > 32 {
        bail!("路线目录需包含 1–32 个校区");
    }
    let valid_id = |id: &str| {
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    };
    let mut ids = HashSet::new();
    for c in &catalog.campuses {
        if !valid_id(&c.id)
            || c.name.trim().is_empty()
            || c.name.chars().count() > 80
            || !ids.insert(&c.id)
        {
            bail!("校区 ID 或名称无效，或 ID 重复");
        }
        if c.nodes.is_empty() || c.nodes.len() > 2048 || c.routes.len() > 64 {
            bail!("校区节点或路线数量超出范围");
        }
        for (i, n) in c.nodes.iter().enumerate() {
            if !n.point.x.is_finite()
                || !n.point.y.is_finite()
                || n.point.x.abs() > 10_000_000.
                || n.point.y.abs() > 10_000_000.
            {
                bail!("节点相对米坐标必须有限且在允许范围内");
            }
            if n.edges.is_empty() || n.edges.len() > 16 {
                bail!("节点需有 1–16 条相邻边");
            }
            let mut seen = HashSet::new();
            for &j in &n.edges {
                if j >= c.nodes.len() || j == i || !seen.insert(j) {
                    bail!("边索引无效、自环或重复");
                }
                let p = c.nodes[j].point;
                if distance(n.point, p) < 1.0 {
                    bail!("相邻节点间距不能小于一米");
                }
                if !c.nodes[j].edges.contains(&i) {
                    bail!("图边必须双向");
                }
            }
        }
        if let Some(map) = &c.map {
            if map.source.trim().is_empty()
                || map.source.chars().count() > 512
                || map
                    .bounds
                    .iter()
                    .any(|v| !v.is_finite() || v.abs() >= 10_000_000.)
                || map.bounds[0] >= map.bounds[2]
                || map.bounds[1] >= map.bounds[3]
            {
                bail!("地图范围或来源无效");
            }
            let mut count = 0usize;
            let mut check_points = |shape: &[Point]| -> Result<()> {
                count = count
                    .checked_add(shape.len())
                    .ok_or_else(|| anyhow::anyhow!("地图点数溢出"))?;
                if count > MAX_POINTS
                    || shape.iter().any(|p| {
                        !p.x.is_finite()
                            || !p.y.is_finite()
                            || p.x.abs() >= 10_000_000.
                            || p.y.abs() >= 10_000_000.
                    })
                {
                    bail!("地图几何点无效或超过上限");
                }
                Ok(())
            };
            check_points(&map.boundary)?;
            for group in [&map.buildings, &map.greens, &map.water, &map.roads] {
                if group.len() > MAX_POINTS {
                    bail!("地图几何数量超过上限");
                }
                for shape in group {
                    check_points(shape)?;
                }
            }
        }
        let mut route_ids = HashSet::new();
        for route in &c.routes {
            if !valid_id(&route.id)
                || route.name.trim().is_empty()
                || route.name.chars().count() > 80
                || !route_ids.insert(&route.id)
                || route.nodes.len() < 2
                || route.nodes.len() > 8192
            {
                bail!("预设路线 ID、名称或节点无效");
            }
            for pair in route.nodes.windows(2) {
                if pair[0] >= c.nodes.len()
                    || pair[1] >= c.nodes.len()
                    || !c.nodes[pair[0]].edges.contains(&pair[1])
                {
                    bail!("预设路线不连通");
                }
            }
        }
    }
    Ok(())
}

/// 按图和请求生成可复现的本地合成步行轨迹。
pub fn generate(
    campus: &Campus,
    request: &GenerateRequest,
    cancel: &AtomicBool,
) -> Result<Preview> {
    if request.distance_m < 100
        || request.distance_m > 10_000
        || request.duration_seconds < 60
        || request.duration_seconds > 10_800
    {
        bail!("距离或时长超出允许范围");
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("路线生成已取消");
    }
    validate_catalog(&Catalog {
        campuses: vec![campus.clone()],
    })?;
    let target = request.distance_m as f64;
    let mut rng = Rng(request.seed);
    let preset = request
        .preset_id
        .as_ref()
        .map(|id| {
            campus
                .routes
                .iter()
                .find(|r| &r.id == id)
                .ok_or_else(|| anyhow::anyhow!("找不到指定预设路线"))
        })
        .transpose()?;
    let sequence = preset.map(|route| {
        let mut nodes = route.nodes.clone();
        if nodes.first() == nodes.last() {
            nodes.pop();
        } else if !campus.nodes[*nodes.last().expect("已验证路线")]
            .edges
            .contains(&nodes[0])
        {
            let back: Vec<_> = nodes[1..nodes.len() - 1].iter().rev().copied().collect();
            nodes.extend(back);
        }
        nodes
    });
    let start = sequence
        .as_ref()
        .map_or_else(|| rng.usize(campus.nodes.len()), |route| route[0]);
    let mut raw = vec![campus.nodes[start].point];
    let mut travelled = 0.0;
    let mut current = start;
    let mut previous = None;
    let mut route_cursor = 1usize;
    for step in 0..MAX_STEPS {
        if step % 64 == 0 && cancel.load(Ordering::Relaxed) {
            bail!("路线生成已取消");
        }
        if travelled >= target {
            break;
        }
        let next = if let Some(route) = &sequence {
            let n = route[route_cursor % route.len()];
            route_cursor += 1;
            n
        } else {
            let edges = &campus.nodes[current].edges;
            let options: Vec<_> = edges
                .iter()
                .copied()
                .filter(|&j| Some(j) != previous)
                .collect();
            let options = if options.is_empty() { edges } else { &options };
            options[rng.usize(options.len())]
        };
        let a = campus.nodes[current].point;
        let b = campus.nodes[next].point;
        let length = distance(a, b);
        if travelled + length > target {
            let t = (target - travelled) / length;
            raw.push(Point {
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t,
            });
            travelled = target;
        } else {
            raw.push(b);
            travelled += length;
            previous = Some(current);
            current = next;
        }
        if raw.len() >= MAX_POINTS {
            bail!("轨迹点数超过上限");
        }
    }
    if travelled < target {
        bail!("达到行走步数上限仍未生成目标距离");
    }
    let reference = raw.clone();
    let amplitude = match request.noise {
        Noise::Off => 0.0,
        Noise::Light => 0.35,
        Noise::Medium => 0.8,
    };
    let phase = rng.unit() * std::f64::consts::TAU;
    let mut positions = vec![raw[0]];
    let mut along = 0.;
    for pair in raw.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let length = distance(a, b);
        if length < 0.000_001 {
            continue;
        }
        let samples = (length / 4.).ceil() as usize;
        if positions.len() + samples > MAX_POINTS {
            bail!("轨迹点数超过上限");
        }
        for i in 1..=samples {
            if i % 64 == 0 && cancel.load(Ordering::Relaxed) {
                bail!("路线生成已取消");
            }
            let t = i as f64 / samples as f64;
            let d = along + t * length;
            let shift = amplitude
                * (std::f64::consts::PI * t).sin()
                * (0.65 * (d / 37. + phase).sin() + 0.35 * (d / 13. - phase).sin());
            positions.push(Point {
                x: a.x + (b.x - a.x) * t - (b.y - a.y) / length * shift,
                y: a.y + (b.y - a.y) * t + (b.x - a.x) / length * shift,
            });
        }
        along += length;
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("路线生成已取消");
    }
    let last = positions.len() - 1;
    let points: Vec<_> = positions
        .into_iter()
        .enumerate()
        .map(|(i, position)| TrackPoint {
            position,
            elapsed_millis: i as u64 * request.duration_seconds as u64 * 1000 / last as u64,
        })
        .collect();
    let actual = points
        .windows(2)
        .map(|w| distance(w[0].position, w[1].position))
        .sum();
    Ok(Preview {
        points,
        reference,
        distance_m: actual,
        duration_seconds: request.duration_seconds,
        seed: request.seed,
        map: campus.map.clone(),
    })
}
fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}
struct Rng(u64);
/// 生成成组的模拟默认值：距离 1500–3000 米，取整后的配速保持 6–9 分钟每公里。
pub fn random_parameters(seed: u64) -> (u32, u32) {
    let mut rng = Rng(seed);
    let distance = 1500 + 100 * rng.usize(16) as u32;
    let pace = 360 + rng.usize(181) as u32;
    let minimum = (distance * 360).div_ceil(60_000);
    let maximum = distance * 540 / 60_000;
    let minutes = ((distance * pace + 30_000) / 60_000).clamp(minimum, maximum);
    (distance, minutes)
}
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn usize(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn random_defaults_vary_and_integer_duration_keeps_the_pace_range() {
        let mut values = std::collections::HashSet::new();
        for seed in 0..1000 {
            let (distance, minutes) = random_parameters(seed);
            assert!((1500..=3000).contains(&distance));
            let pace = minutes as f64 * 60_000. / distance as f64;
            assert!((360.0..=540.0).contains(&pace));
            assert_eq!(random_parameters(seed), (distance, minutes));
            values.insert((distance, minutes));
        }
        assert!(values.len() > 50);
    }
    fn req(seed: u64) -> GenerateRequest {
        GenerateRequest {
            preset_id: None,
            distance_m: 500,
            duration_seconds: 300,
            noise: Noise::Light,
            seed,
        }
    }
    #[test]
    fn seeded_walk_is_deterministic_and_varies() {
        let c = demo_catalog().campuses.remove(0);
        let cancel = AtomicBool::new(false);
        let a = generate(&c, &req(9), &cancel).unwrap();
        let b = generate(&c, &req(9), &cancel).unwrap();
        let d = generate(&c, &req(10), &cancel).unwrap();
        assert_eq!(
            a.points.iter().map(|p| p.position).collect::<Vec<_>>(),
            b.points.iter().map(|p| p.position).collect::<Vec<_>>()
        );
        assert_ne!(
            a.points.iter().map(|p| p.position).collect::<Vec<_>>(),
            d.points.iter().map(|p| p.position).collect::<Vec<_>>()
        );
    }
    #[test]
    fn preset_stays_on_route_and_distance_time_are_bounded() {
        let c = demo_catalog().campuses.remove(0);
        let mut r = req(4);
        r.preset_id = Some("loop-a".into());
        let p = generate(&c, &r, &AtomicBool::new(false)).unwrap();
        assert!((p.distance_m - 500.).abs() < 8.);
        assert_eq!(p.points.first().unwrap().elapsed_millis, 0);
        assert_eq!(p.points.last().unwrap().elapsed_millis, 300_000);
        assert!(
            p.points
                .windows(2)
                .all(|w| w[0].elapsed_millis < w[1].elapsed_millis)
        );
        assert!(p.points.len() <= MAX_POINTS);
        assert!(
            p.points
                .iter()
                .all(|p| p.position.x.is_finite() && p.position.y.is_finite())
        );
    }
    #[test]
    fn all_presets_repeat_without_zero_edges_and_keep_requested_duration() {
        let catalog = demo_catalog();
        validate_catalog(&catalog).unwrap();
        for campus in &catalog.campuses {
            for preset in &campus.routes {
                for distance in [100, 2000, 10_000] {
                    let r = GenerateRequest {
                        preset_id: Some(preset.id.clone()),
                        distance_m: distance,
                        duration_seconds: 60,
                        noise: Noise::Off,
                        seed: 7,
                    };
                    let result = generate(campus, &r, &AtomicBool::new(false)).unwrap();
                    assert!((result.distance_m - f64::from(distance)).abs() < 0.001);
                    assert!(result.points.len() <= MAX_POINTS);
                    assert_eq!(result.points.last().unwrap().elapsed_millis, 60_000);
                    assert!(
                        result
                            .points
                            .windows(2)
                            .all(|p| p[1].elapsed_millis > p[0].elapsed_millis)
                    );
                    assert!(
                        result
                            .points
                            .windows(2)
                            .all(|p| super::distance(p[0].position, p[1].position) < 4.001)
                    );
                }
            }
        }
    }
    #[test]
    fn builtin_maps_validate_and_generate_presets_and_random_routes() {
        let catalog = builtin_catalog().unwrap();
        assert_eq!(catalog.campuses.len(), 2);
        for campus in &catalog.campuses {
            assert!(campus.map.is_some());
            let cancel = AtomicBool::new(false);
            let mut random = req(71);
            random.distance_m = 1000;
            random.noise = Noise::Off;
            let preview = generate(campus, &random, &cancel).unwrap();
            assert_eq!(preview.points.last().unwrap().elapsed_millis, 300_000);
            for route in &campus.routes {
                random.preset_id = Some(route.id.clone());
                let preview = generate(campus, &random, &cancel).unwrap();
                assert_eq!(preview.points.last().unwrap().elapsed_millis, 300_000);
                assert!((preview.distance_m - 1000.).abs() < 0.001);
            }
        }
    }
    #[test]
    fn map_bounds_and_point_limits_are_enforced_and_legacy_config_is_compatible() {
        let mut catalog = builtin_catalog().unwrap();
        catalog.campuses[0].map.as_mut().unwrap().bounds = [1., 0., 0., 1.];
        assert!(validate_catalog(&catalog).is_err());
        let mut catalog = builtin_catalog().unwrap();
        catalog.campuses[0].map.as_mut().unwrap().roads =
            vec![vec![Point { x: 0., y: 0. }; MAX_POINTS + 1]];
        assert!(validate_catalog(&catalog).is_err());
        let legacy: Catalog =
            serde_json::from_str(include_str!("../../config.routes.example.json")).unwrap();
        validate_catalog(&legacy).unwrap();
        assert!(legacy.campuses[0].map.is_none());
    }
    #[test]
    fn open_preset_reverses_along_existing_edges() {
        let mut campus = demo_catalog().campuses.remove(0);
        campus.routes = vec![RoutePreset {
            id: "open".into(),
            name: "Open synthetic route".into(),
            nodes: vec![0, 1, 2],
        }];
        let r = GenerateRequest {
            preset_id: Some("open".into()),
            distance_m: 2000,
            duration_seconds: 1200,
            noise: Noise::Off,
            seed: 1,
        };
        let result = generate(&campus, &r, &AtomicBool::new(false)).unwrap();
        assert!((result.distance_m - 2000.).abs() < 0.001);
        assert!(
            result
                .points
                .iter()
                .all(|p| p.position.y.abs() < 1e-6 || (p.position.x - 120.).abs() < 1e-6)
        );
    }
    #[test]
    fn malformed_graphs_and_requests_fail_before_generation() {
        for bad in [0, 99, 10001, u32::MAX] {
            let mut r = req(1);
            r.distance_m = bad;
            assert!(generate(&demo_catalog().campuses[0], &r, &AtomicBool::new(false)).is_err());
        }
        let catalog = demo_catalog();
        for point in [Point { x: f64::NAN, y: 0. }, Point { x: f64::MAX, y: 0. }] {
            let mut bad = catalog.clone();
            bad.campuses[0].nodes[0].point = point;
            assert!(validate_catalog(&bad).is_err());
        }
        let mut bad = catalog.clone();
        bad.campuses[0].nodes[0].edges.push(999);
        assert!(validate_catalog(&bad).is_err());
        let mut bad = catalog.clone();
        bad.campuses[0].routes[0].nodes = vec![0, 2];
        assert!(validate_catalog(&bad).is_err());
        let mut bad = catalog;
        bad.campuses[0].nodes[1].point = bad.campuses[0].nodes[0].point;
        assert!(validate_catalog(&bad).is_err());
    }
    #[test]
    fn cancellation_and_invalid_graph_fail() {
        let c = demo_catalog().campuses.remove(0);
        assert!(generate(&c, &req(1), &AtomicBool::new(true)).is_err());
        let mut bad = c;
        bad.nodes[0].edges.clear();
        assert!(
            validate_catalog(&Catalog {
                campuses: vec![bad]
            })
            .is_err()
        );
    }
}
