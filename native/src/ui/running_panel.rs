//! 校园跑草稿和可取消的本地路线任务；不执行校园网操作或上传模拟记录。
use super::route_plot::Plot;
use crate::{
    config::RunningSettings,
    running::{self, Catalog, GenerateRequest, Noise, Preview},
};
use anyhow::{Result, bail};
use std::{
    fmt,
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Choice {
    pub(super) id: String,
    pub(super) name: String,
}
impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}
#[derive(Clone)]
pub(super) enum Message {
    Distance(String),
    Minutes(String),
    Campus(Choice),
    Route(Choice),
    Noise(Noise),
    RandomParameters(bool),
    Progress(f32),
    Generate,
    Cancel,
    Phone(String),
    Password(String),
    TogglePassword,
    DefaultCampus(Choice),
    CatalogFile(String),
    LoadCatalog,
}
impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Distance(_) => "RunDistance",
            Self::Minutes(_) => "RunMinutes",
            Self::Campus(_) => "RunCampus",
            Self::Route(_) => "RunRoute",
            Self::Noise(_) => "RunNoise",
            Self::RandomParameters(_) => "RunRandomParameters",
            Self::Progress(_) => "RunProgress",
            Self::Generate => "RunGenerate",
            Self::Cancel => "RunCancel",
            Self::Phone(_) => "RunPhone([REDACTED])",
            Self::Password(_) => "RunPassword([REDACTED])",
            Self::TogglePassword => "RunTogglePassword",
            Self::DefaultCampus(_) => "RunDefaultCampus",
            Self::CatalogFile(_) => "RunCatalogFile([REDACTED])",
            Self::LoadCatalog => "RunLoadCatalog",
        })
    }
}
enum Outcome {
    Preview(Box<Preview>),
    Catalog(Catalog),
}
pub(super) struct Job {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<Outcome>>,
    handle: JoinHandle<()>,
    revision: u64,
    file: String,
}
impl Job {
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub(super) fn join(self) {
        self.cancel();
        let _ = self.handle.join();
    }
}
pub(super) struct State {
    pub(super) prefs: RunningSettings,
    pub(super) password_visible: bool,
    pub(super) distance: String,
    pub(super) minutes: String,
    pub(super) noise: Noise,
    pub(super) random_parameters: bool,
    pub(super) preview: Option<Preview>,
    map_preview: Option<Preview>,
    pub(super) preview_stale: bool,
    pub(super) progress: f32,
    pub(super) revision: u64,
    pub(super) plot: Plot,
    pub(super) error: Option<String>,
    pub(super) status: Option<String>,
    catalog: Catalog,
    campus_override: Option<String>,
    preset: Option<String>,
    loaded_file: String,
    base_dir: PathBuf,
    job: Option<Job>,
    work_revision: u64,
    seed: u64,
}
impl State {
    pub(super) fn new(prefs: RunningSettings, base_dir: PathBuf) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_nanos() as u64)
            .unwrap_or(1);
        let (distance, minutes) = running::random_parameters(seed);
        let (catalog, error) = match running::builtin_catalog() {
            Ok(catalog) => (catalog, None),
            Err(_) => (
                running::demo_catalog(),
                Some("内置地图资源不可用，请检查安装文件".into()),
            ),
        };
        let mut state = Self {
            prefs,
            password_visible: false,
            distance: distance.to_string(),
            minutes: minutes.to_string(),
            noise: Noise::Strong,
            random_parameters: true,
            preview: None,
            map_preview: None,
            preview_stale: false,
            progress: 0.,
            revision: 0,
            plot: Plot::default(),
            error,
            status: None,
            catalog,
            campus_override: None,
            preset: Some("@track".into()),
            loaded_file: String::new(),
            base_dir,
            job: None,
            work_revision: 0,
            seed,
        };
        state.refresh_map();
        state
    }
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }
    fn refresh_map(&mut self) {
        self.map_preview = if self.loaded_file == self.prefs.catalog_file {
            self.catalog
                .campuses
                .iter()
                .find(|c| c.id == self.active_id())
                .and_then(|c| c.map.clone())
                .map(|map| Preview {
                    points: Vec::new(),
                    reference: Vec::new(),
                    distance_m: 0.,
                    duration_seconds: 0,
                    seed: 0,
                    map: Some(map),
                })
        } else {
            None
        };
    }
    pub(super) fn preview_for_plot(&self) -> Option<&Preview> {
        self.preview.as_ref().or(self.map_preview.as_ref())
    }
    fn active_id(&self) -> &str {
        self.campus_override
            .as_deref()
            .unwrap_or(&self.prefs.default_campus)
    }
    fn choice(&self, id: &str) -> Choice {
        Choice {
            id: id.into(),
            name: self
                .catalog
                .campuses
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "未找到校区，请在设置中选择".into()),
        }
    }
    pub(super) fn campus_choices(&self) -> Vec<Choice> {
        std::iter::once(Choice {
            id: String::new(),
            name: format!("默认 · {}", self.choice(&self.prefs.default_campus).name),
        })
        .chain(self.default_campus_choices())
        .collect()
    }
    pub(super) fn default_campus_choices(&self) -> Vec<Choice> {
        self.catalog
            .campuses
            .iter()
            .map(|c| Choice {
                id: c.id.clone(),
                name: c.name.clone(),
            })
            .collect()
    }
    pub(super) fn selected_default_campus(&self) -> Choice {
        self.choice(&self.prefs.default_campus)
    }
    pub(super) fn selected_campus(&self) -> Choice {
        self.campus_override
            .as_ref()
            .map(|id| self.choice(id))
            .unwrap_or_else(|| self.campus_choices().remove(0))
    }
    pub(super) fn route_choices(&self) -> Vec<Choice> {
        let mut choices = vec![Choice {
            id: String::new(),
            name: "校园内道路（随机）".into(),
        }];
        if let Some(campus) = self
            .catalog
            .campuses
            .iter()
            .find(|c| c.id == self.active_id())
        {
            choices.push(Choice {
                id: "@track".into(),
                name: "运动场（随机环线）".into(),
            });
            choices.extend(campus.routes.iter().map(|r| Choice {
                id: r.id.clone(),
                name: format!(
                    "{} · {}",
                    if r.id.starts_with("track-") {
                        "运动场"
                    } else {
                        "校园内道路"
                    },
                    r.name
                ),
            }));
        }
        choices
    }
    pub(super) fn selected_route(&self) -> Choice {
        self.route_choices()
            .into_iter()
            .find(|c| c.id == self.preset.as_deref().unwrap_or(""))
            .unwrap_or(Choice {
                id: String::new(),
                name: "校园内道路（随机）".into(),
            })
    }
    fn invalidate(&mut self, clear: bool) {
        self.work_revision = self.work_revision.wrapping_add(1);
        if let Some(job) = &self.job {
            job.cancel();
        }
        self.error = None;
        self.preview_stale = self.preview.is_some();
        self.status = Some("参数已修改，请重新生成预览".into());
        if clear {
            self.preview = None;
            self.preview_stale = false;
            self.revision = self.revision.wrapping_add(1);
            self.plot.clear();
            self.refresh_map();
        }
    }
    pub(super) fn take_job(&mut self) -> Option<Job> {
        if let Some(job) = &self.job {
            job.cancel();
        }
        self.job.take()
    }
    fn start(
        &mut self,
        file: String,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<Outcome> + Send + 'static,
    ) {
        if self.busy() {
            self.error = Some("请等待当前任务结束或取消后再试".into());
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        match thread::Builder::new()
            .name("running-preview".into())
            .spawn(move || {
                let _ = sender.send(work(flag));
            }) {
            Ok(handle) => {
                self.job = Some(Job {
                    cancel,
                    receiver,
                    handle,
                    revision: self.work_revision,
                    file,
                });
                self.error = None;
            }
            Err(_) => self.error = Some("无法启动路线任务，请重试".into()),
        }
    }
    pub(super) fn load_catalog(&mut self, preview: bool) {
        if self.busy() {
            return;
        }
        if preview {
            self.error = Some("离线预览只使用合成路线，不读取外部路线文件".into());
            return;
        }
        if self.prefs.catalog_file.trim().is_empty() {
            self.catalog = match running::builtin_catalog() {
                Ok(catalog) => catalog,
                Err(_) => {
                    self.error = Some("内置地图资源不可用".into());
                    return;
                }
            };
            self.loaded_file = String::new();
            self.invalidate(true);
            self.status = Some("已使用内置校区道路底图".into());
            return;
        }
        if self.prefs.validate().is_err() {
            self.error = Some("请先修正校园跑设置".into());
            return;
        }
        let file = self.prefs.catalog_file.clone();
        let path = PathBuf::from(&file);
        let path = if path.is_absolute() {
            path
        } else {
            self.base_dir.join(path)
        };
        self.status = Some("正在加载本地路线…".into());
        self.start(file, move |cancel| {
            if cancel.load(Ordering::Relaxed) {
                bail!("路线加载已取消");
            }
            let input = File::open(path).map_err(|_| anyhow::anyhow!("无法打开路线数据文件"))?;
            if !input
                .metadata()
                .map_err(|_| anyhow::anyhow!("无法检查路线数据文件"))?
                .is_file()
            {
                bail!("路线数据必须是普通本地文件");
            }
            let mut bytes = Vec::new();
            input
                .take(1_048_577)
                .read_to_end(&mut bytes)
                .map_err(|_| anyhow::anyhow!("无法读取路线数据文件"))?;
            if bytes.len() > 1_048_576 {
                bail!("路线数据文件不能超过 1 MiB");
            }
            let catalog: Catalog = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("路线 JSON 格式无效"))?;
            running::validate_catalog(&catalog)?;
            if cancel.load(Ordering::Relaxed) {
                bail!("路线加载已取消");
            }
            Ok(Outcome::Catalog(catalog))
        });
    }
    fn generate(&mut self) {
        if self.busy() {
            return;
        }
        if self.random_parameters {
            self.seed = self.seed.wrapping_add(1);
            let (distance, minutes) = running::random_parameters(self.seed);
            self.distance = distance.to_string();
            self.minutes = minutes.to_string();
            self.preview_stale = self.preview.is_some();
        }
        let input = (|| -> Result<_> {
            if self.loaded_file != self.prefs.catalog_file {
                bail!("路线文件已修改，请先在设置中加载路线");
            }
            let distance = self
                .distance
                .trim()
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("距离需要填写整数米"))?;
            let minutes = self
                .minutes
                .trim()
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("时长需要填写整数分钟"))?;
            if !(100..=10000).contains(&distance) || !(1..=180).contains(&minutes) {
                bail!("距离需为 100–10000 米，时长需为 1–180 分钟");
            }
            let campus = self
                .catalog
                .campuses
                .iter()
                .find(|c| c.id == self.active_id())
                .ok_or_else(|| anyhow::anyhow!("请选择有效校区"))?
                .clone();
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|v| v.as_nanos() as u64)
                .unwrap_or(1)
                ^ self.seed.wrapping_add(1);
            let preset_id = if self.preset.as_deref() == Some("@track") {
                let tracks: Vec<_> = campus
                    .routes
                    .iter()
                    .filter(|r| r.id.starts_with("track-"))
                    .collect();
                if tracks.is_empty() {
                    bail!("此校区暂缺已核实的跑道数据，请选择校园内道路");
                }
                Some(tracks[seed as usize % tracks.len()].id.clone())
            } else {
                self.preset.clone()
            };
            Ok((
                campus,
                GenerateRequest {
                    preset_id,
                    distance_m: distance,
                    duration_seconds: minutes * 60,
                    noise: self.noise,
                    seed,
                },
            ))
        })();
        match input {
            Ok((campus, request)) => {
                self.seed = request.seed;
                self.status = Some("正在生成本地模拟路线…".into());
                self.start(String::new(), move |cancel| {
                    running::generate(&campus, &request, &cancel)
                        .map(|preview| Outcome::Preview(Box::new(preview)))
                });
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    pub(super) fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(value) => value,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("路线任务意外结束，请重试"))
            }
        };
        let job = self.job.take().expect("刚才检查过任务");
        let stale = job.revision != self.work_revision || job.cancel.load(Ordering::Relaxed);
        let file = job.file.clone();
        // 只回收已经发送完成结果的线程；不在 UI 线程等待正在执行的任务。
        if job.handle.is_finished() {
            let _ = job.handle.join();
        }
        if stale {
            return;
        }
        match result {
            Ok(Outcome::Preview(mut preview)) => {
                if self.random_parameters {
                    let minutes = align_random_duration(&mut preview);
                    self.minutes = minutes.to_string();
                }
                self.preview = Some(*preview);
                self.preview_stale = false;
                self.progress = 0.;
                self.revision = self.revision.wrapping_add(1);
                self.plot.clear();
                self.status = Some("本地模拟预览已生成".into());
                self.error = None;
            }
            Ok(Outcome::Catalog(catalog)) => {
                self.catalog = catalog;
                self.loaded_file = file;
                self.campus_override = None;
                self.preset = None;
                self.preview = None;
                self.revision = self.revision.wrapping_add(1);
                self.plot.clear();
                self.status = Some("路线已加载，请确认默认校区".into());
                self.error = None;
                self.refresh_map();
            }
            Err(error) => {
                self.status = None;
                self.error = Some(error.to_string());
            }
        }
    }
    pub(super) fn handle(&mut self, message: Message, preview_mode: bool) -> bool {
        match message {
            Message::Phone(value) => {
                self.prefs.phone = value.trim().into();
                return true;
            }
            Message::Password(value) => {
                self.prefs.password = value;
                return true;
            }
            Message::DefaultCampus(choice) => {
                if !self.catalog.campuses.iter().any(|c| c.id == choice.id) {
                    return false;
                }
                self.prefs.default_campus = choice.id;
                if self.campus_override.is_none() {
                    self.preset = None;
                    self.invalidate(true);
                }
                return true;
            }
            Message::CatalogFile(value) => {
                self.prefs.catalog_file = value;
                self.invalidate(true);
                return true;
            }
            Message::TogglePassword => self.password_visible = !self.password_visible,
            Message::LoadCatalog => self.load_catalog(preview_mode),
            Message::Campus(choice) => {
                if !choice.id.is_empty() && !self.catalog.campuses.iter().any(|c| c.id == choice.id)
                {
                    return false;
                }
                self.campus_override = (!choice.id.is_empty()).then_some(choice.id);
                self.preset = None;
                self.invalidate(true);
            }
            Message::Route(choice) => {
                if !self.route_choices().iter().any(|r| r.id == choice.id) {
                    return false;
                }
                self.preset = (!choice.id.is_empty()).then_some(choice.id);
                self.invalidate(false);
            }
            Message::Distance(value) => {
                self.random_parameters = false;
                self.distance = value;
                self.invalidate(false);
            }
            Message::Minutes(value) => {
                self.random_parameters = false;
                self.minutes = value;
                self.invalidate(false);
            }
            Message::Noise(noise) => {
                self.noise = noise;
                self.invalidate(false);
            }
            Message::RandomParameters(enabled) => {
                self.random_parameters = enabled;
                if enabled {
                    self.seed = self.seed.wrapping_add(1);
                    let (distance, minutes) = running::random_parameters(self.seed);
                    self.distance = distance.to_string();
                    self.minutes = minutes.to_string();
                }
                self.invalidate(false);
            }
            Message::Progress(value) => {
                if value.is_finite() {
                    self.progress = value.clamp(0., 100.);
                }
            }
            Message::Generate => self.generate(),
            Message::Cancel => {
                self.invalidate(false);
                self.status = Some("已取消生成".into());
            }
        }
        false
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel();
        }
    }
}

// 扰动可能略微增加里程，按最终轨迹重配整数分钟，避免显示配速越出默认范围。
fn align_random_duration(preview: &mut Preview) -> u32 {
    let minimum = (preview.distance_m * 360. / 60_000.).ceil() as u32;
    let maximum = (preview.distance_m * 540. / 60_000.).floor() as u32;
    let minutes =
        (preview.duration_seconds / 60).clamp(minimum.max(1), maximum.max(minimum).max(1));
    preview.duration_seconds = minutes * 60;
    let count = preview.points.len().saturating_sub(1);
    if count > 0 {
        for (index, point) in preview.points.iter_mut().enumerate() {
            point.elapsed_millis =
                index as u64 * preview.duration_seconds as u64 * 1000 / count as u64;
        }
    }
    minutes
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        State::new(RunningSettings::default(), PathBuf::from("."))
    }
    #[test]
    fn noisy_random_preview_is_retimed_using_actual_distance() {
        let catalog = running::demo_catalog();
        let mut preview = running::generate(
            &catalog.campuses[0],
            &GenerateRequest {
                preset_id: None,
                distance_m: 1500,
                duration_seconds: 9 * 60,
                noise: Noise::Medium,
                seed: 1,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let minutes = align_random_duration(&mut preview);
        let pace = minutes as f64 * 60_000. / preview.distance_m;
        assert!((360.0..=540.0).contains(&pace));
        assert_eq!(
            preview.points.last().unwrap().elapsed_millis,
            u64::from(minutes) * 60_000
        );
    }
    #[test]
    fn temporary_campus_never_changes_preferences_and_default_changes_preserve_override() {
        let mut s = state();
        assert!(!s.handle(
            Message::Campus(Choice {
                id: "demo-b".into(),
                name: String::new()
            }),
            true
        ));
        assert_eq!(s.prefs.default_campus, "demo-a");
        assert_eq!(s.active_id(), "demo-b");
        assert!(s.handle(
            Message::DefaultCampus(Choice {
                id: "demo-a".into(),
                name: String::new()
            }),
            true
        ));
        assert_eq!(s.active_id(), "demo-b");
        s.handle(
            Message::Campus(Choice {
                id: String::new(),
                name: String::new(),
            }),
            true,
        );
        assert_eq!(s.active_id(), "demo-a");
    }
    #[test]
    fn cancelled_or_superseded_completion_cannot_replace_newer_draft() {
        let mut s = state();
        let catalog = running::demo_catalog();
        let result = running::generate(
            &catalog.campuses[0],
            &GenerateRequest {
                preset_id: None,
                distance_m: 1000,
                duration_seconds: 600,
                noise: Noise::Off,
                seed: 1,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let (sender, receiver) = mpsc::channel();
        assert!(sender.send(Ok(Outcome::Preview(Box::new(result)))).is_ok());
        s.job = Some(Job {
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
            handle: thread::spawn(|| {}),
            revision: s.work_revision,
            file: String::new(),
        });
        s.handle(Message::Distance("2500".into()), true);
        s.poll();
        assert!(s.preview.is_none());
        assert!(!s.busy());
        assert_eq!(s.distance, "2500");
    }
    #[test]
    fn input_drafts_survive_navigation_and_debug_redacts_credentials() {
        let mut s = state();
        s.handle(Message::Distance("bad-draft".into()), true);
        s.handle(Message::Noise(Noise::Off), true);
        assert_eq!(s.distance, "bad-draft");
        s.handle(Message::Generate, true);
        assert!(s.error.is_some());
        assert!(!s.busy());
        assert!(
            !format!("{:?}", Message::Password("synthetic-secret".into()))
                .contains("synthetic-secret")
        );
        s.prefs.catalog_file = "synthetic-private.json".into();
        s.load_catalog(true);
        assert!(!s.busy());
    }
}
