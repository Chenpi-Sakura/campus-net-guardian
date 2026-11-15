//! Windows 原生面板：GDI 绘制、系统控件与托盘，不嵌入浏览器。
#![allow(clippy::undocumented_unsafe_blocks)]

use crate::{
    config::{self, AppConfig, GuiOverrides},
    daemon::{Command, Event, Snapshot, Status, Worker},
    platform,
};
use anyhow::{Result, bail};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::{null, null_mut},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, Registry::*, Threading::CreateMutexW},
    UI::{
        Controls::*, HiDpi::*, Input::KeyboardAndMouse::EnableWindow, Shell::*,
        WindowsAndMessaging::*,
    },
};

const BLUE: u32 = rgb(3, 126, 254);
const CLASS: &str = "CampusGuardian.Native.2";
const TRAY: u32 = WM_APP + 1;
const CREATE_SETTINGS: u32 = WM_APP + 2;
const LOGIN: usize = 10;
const PAUSE: usize = 11;
const OPTIONS: usize = 12;
const EXIT: usize = 13;
const SHOW: usize = 14;
const SAVE: usize = 20;
const SCAN: usize = 21;
const CONFIG: usize = 22;
const LOG: usize = 23;
const BACK: usize = 24;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}
// Win32 MAKEINTRESOURCEW 的整数资源 ID，系统不会把它作为地址解引用。
#[allow(clippy::manual_dangling_ptr)]
fn icon_resource() -> *const u16 {
    1usize as *const u16
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

struct App {
    hwnd: HWND,
    settings: HWND,
    worker: Option<Worker>,
    config_path: PathBuf,
    overrides: GuiOverrides,
    effective: AppConfig,
    base: AppConfig,
    pending: Option<GuiOverrides>,
    snapshot: Option<Snapshot>,
    logs: VecDeque<String>,
    startup_error: String,
    dark: bool,
    background: HBRUSH,
    surface: HBRUSH,
    small_icon: HICON,
    big_icon: HICON,
    font: HFONT,
    fonts: HashMap<usize, HFONT>,
    icons: HashMap<usize, (HICON, HICON)>,
    tray: bool,
    benchmark: bool,
    autostart: bool,
    taskbar_created: u32,
    form_checks: [bool; 3],
    form_choices: [usize; 3],
}

impl App {
    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            let _ = worker.commands.send(command);
        }
    }
    fn log(&mut self, message: String) {
        if self.logs.len() == 200 {
            self.logs.pop_front();
        }
        self.logs.push_back(message);
        unsafe {
            let text = self.logs.iter().cloned().collect::<Vec<_>>().join("\r\n");
            let edit = GetDlgItem(self.hwnd, 100);
            SetWindowTextW(edit, wide(&text).as_ptr());
            ShowScrollBar(edit, SB_VERT, (self.logs.len() > 6) as i32);
            SendMessageW(edit, EM_SETSEL, usize::MAX, -1);
            SendMessageW(edit, EM_SCROLLCARET, 0, 0);
        }
    }
    fn bg(&self) -> u32 {
        if self.dark {
            rgb(18, 22, 29)
        } else {
            rgb(245, 247, 251)
        }
    }
    fn card(&self) -> u32 {
        if self.dark {
            rgb(29, 35, 44)
        } else {
            rgb(255, 255, 255)
        }
    }
    fn fg(&self) -> u32 {
        if self.dark {
            rgb(237, 242, 249)
        } else {
            rgb(25, 37, 54)
        }
    }
    fn muted(&self) -> u32 {
        if self.dark {
            rgb(157, 172, 192)
        } else {
            rgb(100, 115, 136)
        }
    }
}

/// 显示不会包含配置内容的启动错误。
pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(message).as_ptr(),
            wide("校园网守护").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 启动原生界面；性能测量模式不读取账密、不联网、不更改自启。
pub fn run(config_path: PathBuf, benchmark: bool, hidden: bool) -> Result<()> {
    // 与旧版测量基线一致：保留一个等待中的后台线程，但不构造真实网络后端。
    let (benchmark_stop, benchmark_wait) = std::sync::mpsc::channel::<()>();
    let benchmark_thread = benchmark.then(|| {
        std::thread::spawn(move || {
            let _ = benchmark_wait.recv();
        })
    });
    let drawing = crate::drawing::DrawingSession::start();
    let antialias = drawing.is_ok();
    let _drawing = drawing.ok();
    unsafe {
        let mutex = if benchmark {
            null_mut()
        } else {
            CreateMutexW(null(), 0, wide("Local\\CampusGuardian.Native.2").as_ptr())
        };
        if !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            let existing = FindWindowW(wide(CLASS).as_ptr(), null());
            if !existing.is_null() {
                ShowWindow(existing, SW_SHOW);
                SetForegroundWindow(existing);
            }
            CloseHandle(mutex);
            return Ok(());
        }
        let instance = GetModuleHandleW(null());
        {
            let class_name = wide(CLASS);
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: class_name.as_ptr(),
                ..zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                bail!("无法注册窗口类");
            }
        }
        let dark = is_dark();
        let overrides = if benchmark {
            GuiOverrides::default()
        } else {
            GuiOverrides::load()
        };
        let mut startup_error = String::new();
        let mut effective = AppConfig::default();
        let mut base = AppConfig::default();
        let worker = if benchmark {
            None
        } else {
            if !config_path.exists() {
                if let Some(parent) = config_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&config_path, include_str!("../../config.example.yaml"))?;
            }
            match AppConfig::load(&config_path) {
                Ok(cfg) => {
                    effective = cfg.effective(&overrides);
                    base = cfg.clone();
                    Some(Worker::start(cfg, overrides.clone()))
                }
                Err(error) => {
                    startup_error = format!("请检查配置：{error}。打开配置文件修正后重启。");
                    None
                }
            }
        };
        let state = Box::new(RefCell::new(App {
            hwnd: null_mut(),
            settings: null_mut(),
            worker,
            config_path,
            overrides,
            effective,
            base,
            pending: None,
            snapshot: None,
            logs: VecDeque::new(),
            startup_error,
            dark,
            background: CreateSolidBrush(if dark {
                rgb(18, 22, 29)
            } else {
                rgb(245, 247, 251)
            }),
            surface: CreateSolidBrush(if dark {
                rgb(29, 35, 44)
            } else {
                rgb(255, 255, 255)
            }),
            small_icon: null_mut(),
            big_icon: null_mut(),
            font: null_mut(),
            fonts: HashMap::new(),
            icons: HashMap::new(),
            tray: false,
            benchmark,
            autostart: !benchmark && platform::autostart_enabled(),
            form_checks: [false; 3],
            form_choices: [0, 1, 1],
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        }));
        let dpi = GetDpiForSystem().max(96);
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: scale(420, dpi),
            bottom: scale(580, dpi),
        };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        AdjustWindowRectExForDpi(&mut bounds, style, 0, 0, dpi);
        let hwnd = CreateWindowExW(
            0,
            wide(CLASS).as_ptr(),
            wide("校园网守护").as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            null_mut(),
            null_mut(),
            instance,
            (&*state as *const RefCell<App>).cast(),
        );
        if hwnd.is_null() {
            bail!("无法创建窗口");
        }
        let mut app = state.borrow_mut();
        app.hwnd = hwnd;
        let mut work: RECT = zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
        SetWindowPos(
            hwnd,
            null_mut(),
            (work.right - (bounds.right - bounds.left) - scale(20, dpi)).max(0),
            (work.bottom - (bounds.bottom - bounds.top) - scale(20, dpi)).max(0),
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER,
        );
        apply_icons(hwnd, &mut app);
        let mut notify = tray_data(&app);
        app.tray = Shell_NotifyIconW(NIM_ADD, &notify) != 0;
        if app.tray {
            notify.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &notify);
        }
        if !hidden || !app.tray {
            ShowWindow(hwnd, SW_SHOW);
        }
        SetTimer(hwnd, 1, 500, None);
        if benchmark {
            app.log("性能测量模式 · 已禁用联网与配置写入".into());
        }
        if !antialias {
            app.log("抗锯齿绘图不可用，已使用基础绘图模式".into());
        }
        drop(app);
        // 首次 ShowWindow 可能遵循父进程的 STARTF_USESHOWWINDOW（隐藏启动）。
        // 显式可见模式再显示一次，并在没有状态借用时完成首帧绘制。
        if !hidden || !state.borrow().tray {
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        }
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let mut app = state.borrow_mut();
        if app.tray {
            Shell_NotifyIconW(NIM_DELETE, &tray_data(&app));
        }
        app.send(Command::Stop);
        for font in app.fonts.values() {
            DeleteObject(*font);
        }
        for (small, big) in app.icons.values() {
            if !small.is_null() {
                DestroyIcon(*small);
            }
            if !big.is_null() {
                DestroyIcon(*big);
            }
        }
        DeleteObject(app.background);
        DeleteObject(app.surface);
        let worker = app.worker.take();
        drop(app);
        if let Some(worker) = worker {
            worker.join();
        }
        if !mutex.is_null() {
            CloseHandle(mutex);
        }
        drop(benchmark_stop);
        if let Some(thread) = benchmark_thread {
            let _ = thread.join();
        }
        Ok(())
    }
}

unsafe fn is_dark() -> bool {
    unsafe {
        let mut value: u32 = 1;
        let mut len = 4;
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            wide("AppsUseLightTheme").as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut len,
        );
        value == 0
    }
}
fn scale(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}
unsafe fn apply_icons(hwnd: HWND, app: &mut App) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let instance = GetModuleHandleW(null());
        let small: HICON = LoadImageW(
            instance,
            icon_resource(),
            IMAGE_ICON,
            GetSystemMetricsForDpi(SM_CXSMICON, dpi),
            GetSystemMetricsForDpi(SM_CYSMICON, dpi),
            0,
        )
        .cast();
        let big: HICON = LoadImageW(
            instance,
            icon_resource(),
            IMAGE_ICON,
            GetSystemMetricsForDpi(SM_CXICON, dpi),
            GetSystemMetricsForDpi(SM_CYICON, dpi),
            0,
        )
        .cast();
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
        if hwnd == app.hwnd {
            app.small_icon = small;
            app.big_icon = big;
            if app.tray {
                Shell_NotifyIconW(NIM_MODIFY, &tray_data(app));
            }
        }
        if let Some((old_small, old_big)) = app.icons.insert(hwnd as usize, (small, big)) {
            if !old_small.is_null() {
                DestroyIcon(old_small);
            }
            if !old_big.is_null() {
                DestroyIcon(old_big);
            }
        }
    }
}
unsafe fn tray_data(app: &App) -> NOTIFYICONDATAW {
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = app.hwnd;
        data.uID = 1;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY;
        data.hIcon = app.small_icon;
        let tip = wide("校园网守护");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        data
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, message, w, l);
        }
        let Ok(mut borrowed) = (*pointer).try_borrow_mut() else {
            return DefWindowProcW(hwnd, message, w, l);
        };
        let app = &mut *borrowed;
        if app.taskbar_created != 0 && message == app.taskbar_created && hwnd == app.hwnd {
            let mut notify = tray_data(app);
            app.tray = Shell_NotifyIconW(NIM_ADD, &notify) != 0;
            if app.tray {
                notify.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                Shell_NotifyIconW(NIM_SETVERSION, &notify);
            } else {
                ShowWindow(hwnd, SW_SHOW);
            }
            return 0;
        }
        match message {
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                let dark = is_dark();
                if dark != app.dark {
                    app.dark = dark;
                    DeleteObject(app.background);
                    DeleteObject(app.surface);
                    app.background = CreateSolidBrush(app.bg());
                    app.surface = CreateSolidBrush(app.card());
                    for win in [app.hwnd, app.settings] {
                        if !win.is_null() {
                            let enabled = dark as i32;
                            DwmSetWindowAttribute(win, 20, (&enabled as *const i32).cast(), 4);
                            RedrawWindow(win, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
                        }
                    }
                }
                return 0;
            }
            WM_CREATE => {
                app.hwnd = hwnd;
                build_controls(hwnd, app, false);
                let dark = app.dark as i32;
                DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
                return 0;
            }
            CREATE_SETTINGS => {
                clear_controls(hwnd);
                app.settings = hwnd;
                build_controls(hwnd, app, true);
                InvalidateRect(hwnd, null(), 1);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                paint(hwnd, app);
                return 0;
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let dc = w as HDC;
                SetTextColor(dc, app.fg());
                SetBkColor(dc, app.card());
                return app.surface as isize;
            }
            WM_DRAWITEM => {
                draw_button(&*(l as *const DRAWITEMSTRUCT), app);
                return 1;
            }
            WM_COMMAND => {
                command(hwnd, app, w & 0xffff);
                return 0;
            }
            WM_TIMER => {
                let events: Vec<_> = app
                    .worker
                    .as_ref()
                    .map(|worker| worker.events.try_iter().collect())
                    .unwrap_or_default();
                for event in events {
                    match event {
                        Event::Snapshot(snapshot) => {
                            app.snapshot = Some(snapshot);
                        }
                        Event::Log(message) => app.log(message),
                        Event::Networks(networks) => {
                            if !app.settings.is_null() {
                                pick_network(app.hwnd, &networks);
                            }
                        }
                        Event::Autostart(enabled) => {
                            app.autostart = enabled;
                        }
                        Event::SettingsSaved(success) => {
                            if success {
                                if let Some(settings) = app.pending.take() {
                                    app.effective = app.base.effective(&settings);
                                    app.overrides = settings;
                                }
                            } else {
                                app.pending = None;
                                show_error("守护设置保存失败，请查看最近动态。");
                            }
                        }
                    }
                }
                if let Some(snapshot) = &app.snapshot {
                    SetWindowTextW(
                        GetDlgItem(app.hwnd, PAUSE as i32),
                        wide(if snapshot.paused {
                            "恢复守护"
                        } else {
                            "暂停守护"
                        })
                        .as_ptr(),
                    );
                    EnableWindow(GetDlgItem(app.hwnd, LOGIN as i32), (!snapshot.busy) as i32);
                }
                InvalidateRect(app.hwnd, null(), 0);
                return 0;
            }
            TRAY => {
                match (l as u32) & 0xffff {
                    NIN_SELECT | 0x401 => {
                        if IsWindowVisible(app.hwnd) != 0 {
                            ShowWindow(app.hwnd, SW_HIDE);
                        } else {
                            ShowWindow(app.hwnd, SW_SHOW);
                            SetForegroundWindow(app.hwnd);
                        }
                    }
                    WM_CONTEXTMENU => {
                        let menu = CreatePopupMenu();
                        for (id, label) in [
                            (SHOW, "打开面板"),
                            (LOGIN, "立即登录"),
                            (PAUSE, "暂停 / 恢复守护"),
                            (EXIT, "退出"),
                        ] {
                            AppendMenuW(menu, MF_STRING, id, wide(label).as_ptr());
                        }
                        let mut p: POINT = zeroed();
                        GetCursorPos(&mut p);
                        SetForegroundWindow(app.hwnd);
                        let selected = TrackPopupMenu(
                            menu,
                            TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                            p.x,
                            p.y,
                            0,
                            app.hwnd,
                            null(),
                        );
                        DestroyMenu(menu);
                        if selected > 0 {
                            PostMessageW(app.hwnd, WM_COMMAND, selected as usize, 0);
                        }
                    }
                    _ => {}
                }
                return 0;
            }
            WM_DPICHANGED => {
                let r = &*(l as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER,
                );
                apply_icons(hwnd, app);
                update_font(hwnd, app);
                layout_controls(hwnd, app, hwnd == app.settings);
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_CLOSE => {
                if app.tray {
                    ShowWindow(hwnd, SW_HIDE);
                } else {
                    command(hwnd, app, EXIT);
                }
                return 0;
            }
            WM_DESTROY => {
                if hwnd == app.hwnd {
                    PostQuitMessage(0);
                } else {
                    app.settings = null_mut();
                }
                return 0;
            }
            _ => {}
        }
        drop(borrowed);
        DefWindowProcW(hwnd, message, w, l)
    }
}

unsafe fn control(
    hwnd: HWND,
    class: &str,
    label: &str,
    id: usize,
    style: u32,
    rect: (i32, i32, i32, i32),
    font: HFONT,
) -> HWND {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let (x, y, w, h) = rect;
        let child = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(label).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
            scale(x, dpi),
            scale(y, dpi),
            scale(w, dpi),
            scale(h, dpi),
            hwnd,
            id as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(child, WM_SETFONT, font as usize, 1);
        if is_dark() {
            SetWindowTheme(child, wide("DarkMode_Explorer").as_ptr(), null());
        }
        child
    }
}
unsafe fn build_controls(hwnd: HWND, app: &mut App, settings: bool) {
    unsafe {
        update_font(hwnd, app);
        let font = *app.fonts.get(&(hwnd as usize)).unwrap_or(&app.font);
        if !settings {
            control(
                hwnd,
                "BUTTON",
                "立即登录",
                LOGIN,
                BS_OWNERDRAW as u32,
                (28, 278, 176, 42),
                font,
            );
            control(
                hwnd,
                "BUTTON",
                "暂停守护",
                PAUSE,
                BS_OWNERDRAW as u32,
                (216, 278, 176, 42),
                font,
            );
            control(
                hwnd,
                "EDIT",
                "",
                100,
                ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL,
                (40, 376, 340, 130),
                font,
            );
            ShowScrollBar(GetDlgItem(hwnd, 100), SB_VERT, 0);
            control(
                hwnd,
                "BUTTON",
                "设置",
                OPTIONS,
                BS_OWNERDRAW as u32,
                (312, 536, 80, 30),
                font,
            );
        } else {
            control(
                hwnd,
                "BUTTON",
                "返回",
                BACK,
                BS_OWNERDRAW as u32,
                (312, 20, 80, 30),
                font,
            );
            control(
                hwnd,
                "BUTTON",
                "登录 Windows 时启动",
                200,
                BS_OWNERDRAW as u32,
                (28, 78, 364, 28),
                font,
            );
            control(
                hwnd,
                "BUTTON",
                "主网不可用时使用备用网络",
                201,
                BS_OWNERDRAW as u32,
                (28, 116, 364, 28),
                font,
            );
            control(
                hwnd,
                "EDIT",
                "",
                202,
                ES_AUTOHSCROLL as u32,
                (170, 169, 146, 24),
                font,
            );
            control(
                hwnd,
                "BUTTON",
                "扫描",
                SCAN,
                BS_OWNERDRAW as u32,
                (338, 164, 54, 30),
                font,
            );
            control(
                hwnd,
                "EDIT",
                "",
                203,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32,
                (170, 217, 210, 24),
                font,
            );
            for (id, y) in [(204, 260), (205, 308), (206, 356)] {
                control(
                    hwnd,
                    "BUTTON",
                    "",
                    id,
                    BS_OWNERDRAW as u32,
                    (158, y, 234, 34),
                    font,
                );
            }
            control(
                hwnd,
                "BUTTON",
                "启用调试日志",
                207,
                BS_OWNERDRAW as u32,
                (28, 406, 364, 28),
                font,
            );
            control(
                hwnd,
                "BUTTON",
                "打开配置文件",
                CONFIG,
                BS_OWNERDRAW as u32,
                (28, 466, 176, 34),
                font,
            );
            control(
                hwnd,
                "BUTTON",
                "打开日志",
                LOG,
                BS_OWNERDRAW as u32,
                (216, 466, 176, 34),
                font,
            );
            control(
                hwnd,
                "BUTTON",
                "保存设置",
                SAVE,
                BS_OWNERDRAW as u32,
                (28, 526, 364, 42),
                font,
            );
            app.form_checks = [
                app.autostart,
                app.effective.wifi.fallback.enabled,
                app.effective.logging.level == "DEBUG",
            ];
            for (slot, id) in [200, 201, 207].iter().enumerate() {
                update_toggle(hwnd, *id, slot, app);
            }
            EnableWindow(GetDlgItem(hwnd, SAVE as i32), app.pending.is_none() as i32);
            SetWindowTextW(
                GetDlgItem(hwnd, 202),
                wide(&app.effective.wifi.fallback.ssid).as_ptr(),
            );
            SetWindowTextW(
                GetDlgItem(hwnd, 203),
                wide(&app.effective.wifi.fallback.password).as_ptr(),
            );
            for (slot, (id, value, values)) in [
                (
                    204,
                    app.effective.wifi.fallback.switch_back_interval,
                    vec![0, 300, 900],
                ),
                (
                    205,
                    app.effective.daemon.probe_interval,
                    vec![15, 30, 60, 120],
                ),
                (
                    206,
                    app.effective.daemon.failure_threshold as u64,
                    vec![2, 3, 5],
                ),
            ]
            .into_iter()
            .enumerate()
            {
                app.form_choices[slot] = values
                    .iter()
                    .position(|v| *v == value)
                    .unwrap_or(values.len());
                update_choice(hwnd, id, slot, app);
            }
        }
    }
}
unsafe fn layout_controls(hwnd: HWND, _app: &App, settings: bool) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let rects = if settings {
            vec![
                (BACK, 312, 20, 80, 30),
                (200, 28, 78, 364, 28),
                (201, 28, 116, 364, 28),
                (202, 170, 169, 146, 24),
                (SCAN, 338, 164, 54, 30),
                (203, 170, 217, 210, 24),
                (204, 158, 260, 234, 34),
                (205, 158, 308, 234, 34),
                (206, 158, 356, 234, 34),
                (207, 28, 406, 364, 28),
                (CONFIG, 28, 466, 176, 34),
                (LOG, 216, 466, 176, 34),
                (SAVE, 28, 526, 364, 42),
            ]
        } else {
            vec![
                (LOGIN, 28, 278, 176, 42),
                (PAUSE, 216, 278, 176, 42),
                (100, 40, 376, 340, 130),
                (OPTIONS, 312, 536, 80, 30),
            ]
        };
        for (id, x, y, w, h) in rects {
            MoveWindow(
                GetDlgItem(hwnd, id as i32),
                scale(x, dpi),
                scale(y, dpi),
                scale(w, dpi),
                scale(h, dpi),
                1,
            );
        }
    }
}
unsafe extern "system" fn set_child_font(hwnd: HWND, font: LPARAM) -> i32 {
    unsafe {
        SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
        1
    }
}
unsafe fn update_font(hwnd: HWND, app: &mut App) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let font = CreateFontW(
            -scale(14, dpi),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Microsoft YaHei UI").as_ptr(),
        );
        EnumChildWindows(hwnd, Some(set_child_font), font as isize);
        if let Some(old) = app.fonts.insert(hwnd as usize, font) {
            DeleteObject(old);
        }
        if hwnd == app.hwnd {
            app.font = font;
        }
    }
}
unsafe fn clear_controls(hwnd: HWND) {
    unsafe {
        let mut child = GetWindow(hwnd, GW_CHILD);
        while !child.is_null() {
            let next = GetWindow(child, GW_HWNDNEXT);
            DestroyWindow(child);
            child = next;
        }
    }
}
unsafe fn close_settings(app: &mut App) {
    unsafe {
        clear_controls(app.hwnd);
        app.settings = null_mut();
        build_controls(app.hwnd, app, false);
        let text = app.logs.iter().cloned().collect::<Vec<_>>().join("\r\n");
        let edit = GetDlgItem(app.hwnd, 100);
        SetWindowTextW(edit, wide(&text).as_ptr());
        ShowScrollBar(edit, SB_VERT, (app.logs.len() > 6) as i32);
        InvalidateRect(app.hwnd, null(), 1);
    }
}
unsafe fn text_of(hwnd: HWND, id: i32) -> String {
    unsafe {
        let child = GetDlgItem(hwnd, id);
        let len = GetWindowTextLengthW(child).max(0) as usize;
        let mut text = vec![0u16; len + 1];
        GetWindowTextW(child, text.as_mut_ptr(), text.len() as i32);
        String::from_utf16_lossy(&text[..len])
    }
}
unsafe fn command(hwnd: HWND, app: &mut App, id: usize) {
    unsafe {
        match id {
            SHOW => {
                ShowWindow(app.hwnd, SW_SHOW);
                SetForegroundWindow(app.hwnd);
            }
            LOGIN => app.send(Command::Login),
            PAUSE => app.send(Command::Pause(
                !app.snapshot.as_ref().is_some_and(|s| s.paused),
            )),
            OPTIONS => {
                PostMessageW(app.hwnd, CREATE_SETTINGS, 0, 0);
            }
            BACK => close_settings(app),
            CONFIG => {
                if platform::open_file(&app.config_path).is_err() {
                    show_error("无法打开配置文件，请检查文件关联。");
                }
            }
            LOG => {
                let path = if app.effective.logging.file.is_empty() {
                    config::data_dir().join("guardian.log")
                } else {
                    PathBuf::from(&app.effective.logging.file)
                };
                if platform::open_file(&path).is_err() {
                    show_error("日志文件尚未生成。");
                }
            }
            SCAN => {
                if !app.benchmark {
                    app.send(Command::ScanNetworks);
                }
            }
            200 | 201 | 207 => {
                let slot = if id == 200 {
                    0
                } else if id == 201 {
                    1
                } else {
                    2
                };
                app.form_checks[slot] = !app.form_checks[slot];
                update_toggle(hwnd, id as i32, slot, app);
            }
            204..=206 => choose_option(hwnd, id as i32, app),
            SAVE => {
                if app.pending.is_some() {
                    app.log("正在保存设置，请稍候".into());
                    return;
                }
                if app.benchmark {
                    MessageBoxW(
                        hwnd,
                        wide("性能测量模式不保存配置。").as_ptr(),
                        wide("守护设置").as_ptr(),
                        MB_OK,
                    );
                    return;
                }
                let selection = |id: usize| app.form_choices[id - 204];
                let ov = GuiOverrides {
                    fallback_enabled: Some(app.form_checks[1]),
                    fallback_ssid: Some(text_of(hwnd, 202).trim().into()),
                    fallback_password: Some(text_of(hwnd, 203)),
                    switch_back_interval: [0, 300, 900].get(selection(204)).copied(),
                    probe_interval: [15, 30, 60, 120].get(selection(205)).copied(),
                    failure_threshold: [2, 3, 5].get(selection(206)).copied(),
                    log_level: if app.form_checks[2] {
                        Some("DEBUG".into())
                    } else if app.effective.logging.level == "DEBUG" {
                        Some("INFO".into())
                    } else {
                        app.overrides.log_level.clone()
                    },
                };
                if ov.fallback_enabled == Some(true) && ov.fallback_ssid.as_deref() == Some("") {
                    show_error("请先填写备用 WiFi 名称。");
                    return;
                }
                let auto = app.form_checks[0];
                if auto != app.autostart {
                    app.send(Command::Autostart(auto, app.config_path.clone()));
                }
                if app.worker.is_none() {
                    show_error("请先填写有效的校园网配置并重启，再保存守护设置。");
                    return;
                }
                app.pending = Some(ov.clone());
                app.send(Command::Settings(ov));
                close_settings(app);
            }
            EXIT if app.benchmark
                || MessageBoxW(
                    app.hwnd,
                    wide("退出后将停止自动重连，确定退出吗？").as_ptr(),
                    wide("退出校园网守护").as_ptr(),
                    MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
                ) == IDYES =>
            {
                app.send(Command::Stop);
                DestroyWindow(app.hwnd);
                PostQuitMessage(0);
            }
            _ => {}
        }
    }
}

struct Canvas {
    dc: HDC,
    dpi: u32,
}
impl Canvas {
    unsafe fn rect(&self, x: i32, y: i32, w: i32, h: i32, color: u32, radius: i32) {
        unsafe {
            let rect = RECT {
                left: scale(x, self.dpi),
                top: scale(y, self.dpi),
                right: scale(x + w, self.dpi),
                bottom: scale(y + h, self.dpi),
            };
            crate::drawing::rounded_rect(
                self.dc,
                &rect,
                scale(radius, self.dpi) as f32 / 2.0,
                color,
            );
        }
    }
    // 绘图调用直接表达坐标和字体，避免为一次绘制构造多层样式对象。
    #[allow(clippy::too_many_arguments)]
    unsafe fn text(
        &self,
        text: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        size: i32,
        weight: i32,
        color: u32,
    ) {
        unsafe {
            let font = CreateFontW(
                -scale(size, self.dpi),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Microsoft YaHei UI").as_ptr(),
            );
            let old = SelectObject(self.dc, font);
            SetTextColor(self.dc, color);
            SetBkMode(self.dc, TRANSPARENT as i32);
            let mut rect = RECT {
                left: scale(x, self.dpi),
                top: scale(y, self.dpi),
                right: scale(x + w, self.dpi),
                bottom: scale(y + h, self.dpi),
            };
            DrawTextW(
                self.dc,
                wide(text).as_ptr(),
                -1,
                &mut rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            SelectObject(self.dc, old);
            DeleteObject(font);
        }
    }
}
unsafe fn paint(hwnd: HWND, app: &App) {
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut bounds: RECT = zeroed();
        GetClientRect(hwnd, &mut bounds);
        let memory = CreateCompatibleDC(dc);
        let bitmap = CreateCompatibleBitmap(dc, bounds.right.max(1), bounds.bottom.max(1));
        let old = SelectObject(memory, bitmap);
        FillRect(memory, &bounds, app.background);
        let canvas = Canvas {
            dc: memory,
            dpi: GetDpiForWindow(hwnd).max(96),
        };
        if hwnd == app.settings {
            canvas.text("守护设置", 28, 18, 364, 32, 22, 600, app.fg());
            canvas.text(
                "修改后下一轮生效 · 账号与门户地址保存在配置文件",
                28,
                51,
                364,
                22,
                11,
                400,
                app.muted(),
            );
            canvas.rect(158, 164, 170, 34, app.card(), 10);
            canvas.rect(158, 212, 234, 34, app.card(), 10);
            for (y, label) in [
                (164, "备用 WiFi"),
                (212, "热点密码"),
                (260, "切回校园网"),
                (308, "探测间隔"),
                (356, "掉线判定"),
            ] {
                canvas.text(label, 28, y, 120, 30, 14, 400, app.fg());
            }
        } else {
            DrawIconEx(
                memory,
                scale(28, canvas.dpi),
                scale(26, canvas.dpi),
                app.big_icon,
                scale(32, canvas.dpi),
                scale(32, canvas.dpi),
                0,
                null_mut(),
                DI_NORMAL,
            );
            canvas.text("校园网守护", 72, 21, 270, 28, 20, 600, app.fg());
            canvas.text("CAMPUS NET GUARDIAN", 73, 48, 270, 18, 10, 400, app.muted());
            canvas.rect(28, 88, 364, 168, app.card(), 20);
            let (label, color) = if app.benchmark {
                ("网络已连接", rgb(38, 153, 103))
            } else {
                match app.snapshot.as_ref().map(|s| &s.status) {
                    Some(Status::Online) => ("网络已连接", rgb(38, 153, 103)),
                    Some(Status::Authenticating) => ("正在认证", BLUE),
                    Some(Status::Fallback) => ("使用备用网络", BLUE),
                    Some(Status::Paused) => ("守护已暂停", app.muted()),
                    Some(Status::Starting) => ("正在启动", BLUE),
                    Some(Status::Stopped) => ("守护已停止", app.muted()),
                    _ if !app.startup_error.is_empty() => ("需要配置", rgb(205, 132, 28)),
                    _ => ("网络未连接", rgb(215, 86, 86)),
                }
            };
            canvas.rect(48, 113, 8, 8, color, 8);
            canvas.text("当前状态", 64, 103, 280, 26, 12, 400, app.muted());
            canvas.text(label, 48, 136, 324, 42, 28, 600, app.fg());
            let detail = if app.benchmark {
                "连接正常，正在为你保持在线。"
            } else if !app.startup_error.is_empty() {
                &app.startup_error
            } else {
                app.snapshot
                    .as_ref()
                    .map(|s| s.detail.as_str())
                    .unwrap_or("正在准备网络守护…")
            };
            canvas.text(detail, 48, 183, 324, 24, 12, 400, app.muted());
            let last = app
                .snapshot
                .as_ref()
                .and_then(|s| s.last_login)
                .and_then(|t| t.elapsed().ok())
                .map(|age| {
                    if age.as_secs() < 60 {
                        "最近登录：刚刚".into()
                    } else {
                        format!("最近登录：{} 分钟前", age.as_secs() / 60)
                    }
                })
                .unwrap_or_else(|| "最近登录：—".into());
            canvas.text(&last, 48, 206, 324, 18, 10, 400, app.muted());
            let next = app
                .snapshot
                .as_ref()
                .and_then(|s| s.next_probe)
                .map(|t| {
                    format!(
                        "下次检查 {} 秒",
                        t.saturating_duration_since(Instant::now()).as_secs()
                    )
                })
                .unwrap_or_else(|| "自动检查连接状态".into());
            canvas.text(&next, 48, 230, 165, 22, 11, 400, app.muted());
            let ssid = if app.benchmark {
                "Campus_WiFi"
            } else {
                app.snapshot
                    .as_ref()
                    .map(|s| s.ssid.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("未连接 WiFi")
            };
            canvas.text(ssid, 210, 230, 162, 22, 11, 400, app.muted());
            canvas.rect(28, 344, 364, 176, app.card(), 18);
            canvas.text("最近动态", 42, 350, 200, 22, 12, 600, app.muted());
            let fallback = if app.effective.wifi.fallback.ssid.is_empty() {
                "未设置"
            } else {
                &app.effective.wifi.fallback.ssid
            };
            canvas.text(
                &format!("备用：{fallback}"),
                28,
                538,
                274,
                26,
                11,
                400,
                app.muted(),
            );
        }
        BitBlt(dc, 0, 0, bounds.right, bounds.bottom, memory, 0, 0, SRCCOPY);
        SelectObject(memory, old);
        DeleteObject(bitmap);
        DeleteDC(memory);
        EndPaint(hwnd, &ps);
    }
}
unsafe fn draw_button(item: &DRAWITEMSTRUCT, app: &App) {
    unsafe {
        FillRect(item.hDC, &item.rcItem, app.background);
        if [200, 201, 207].contains(&item.CtlID) {
            draw_toggle(item, app);
            return;
        }
        let selected = item.itemState & ODS_SELECTED != 0;
        let primary = item.CtlID as usize == LOGIN || item.CtlID as usize == SAVE;
        let color = if primary {
            if selected { rgb(0, 101, 218) } else { BLUE }
        } else {
            if app.dark {
                rgb(43, 52, 64)
            } else {
                rgb(230, 236, 245)
            }
        };
        let dpi = GetDpiForWindow(item.hwndItem).max(96);
        crate::drawing::rounded_rect(item.hDC, &item.rcItem, scale(8, dpi) as f32, color);
        let mut text = [0u16; 128];
        GetWindowTextW(item.hwndItem, text.as_mut_ptr(), 128);
        let parent = GetParent(item.hwndItem);
        let font = *app.fonts.get(&(parent as usize)).unwrap_or(&app.font);
        let old = SelectObject(item.hDC, font);
        SetBkMode(item.hDC, TRANSPARENT as i32);
        SetTextColor(
            item.hDC,
            if item.itemState & ODS_DISABLED != 0 {
                app.muted()
            } else if primary {
                rgb(255, 255, 255)
            } else {
                app.fg()
            },
        );
        let mut rect = item.rcItem;
        DrawTextW(
            item.hDC,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        if item.itemState & ODS_FOCUS != 0 {
            InflateRect(&mut rect, -scale(4, dpi), -scale(4, dpi));
            DrawFocusRect(item.hDC, &rect);
        }
        SelectObject(item.hDC, old);
    }
}

fn choice_values(slot: usize) -> Vec<u64> {
    match slot {
        0 => vec![0, 300, 900],
        1 => vec![15, 30, 60, 120],
        _ => vec![2, 3, 5],
    }
}
fn choice_label(slot: usize, value: u64) -> String {
    match slot {
        0 => {
            if value == 0 {
                "不自动切回".into()
            } else {
                format!("每 {} 分钟", value / 60)
            }
        }
        1 => format!("{value} 秒"),
        _ => format!("连续失败 {value} 次"),
    }
}
unsafe fn update_choice(hwnd: HWND, id: i32, slot: usize, app: &App) {
    unsafe {
        let values = choice_values(slot);
        let custom = match slot {
            0 => app.effective.wifi.fallback.switch_back_interval,
            1 => app.effective.daemon.probe_interval,
            _ => app.effective.daemon.failure_threshold as u64,
        };
        let label = values
            .get(app.form_choices[slot])
            .map(|v| choice_label(slot, *v))
            .unwrap_or_else(|| format!("沿用配置（{custom}）"));
        SetWindowTextW(
            GetDlgItem(hwnd, id),
            wide(&format!("{label}     ⌄")).as_ptr(),
        );
        InvalidateRect(GetDlgItem(hwnd, id), null(), 1);
    }
}
unsafe fn update_toggle(hwnd: HWND, id: i32, slot: usize, app: &App) {
    unsafe {
        let label = [
            "登录 Windows 时启动",
            "主网不可用时使用备用网络",
            "启用调试日志",
        ][slot];
        SetWindowTextW(
            GetDlgItem(hwnd, id),
            wide(&format!(
                "{label}：{}",
                if app.form_checks[slot] {
                    "开启"
                } else {
                    "关闭"
                }
            ))
            .as_ptr(),
        );
        InvalidateRect(GetDlgItem(hwnd, id), null(), 1);
    }
}
unsafe fn choose_option(hwnd: HWND, id: i32, app: &mut App) {
    unsafe {
        let slot = (id - 204) as usize;
        let values = choice_values(slot);
        let menu = CreatePopupMenu();
        for (index, value) in values.iter().enumerate() {
            AppendMenuW(
                menu,
                MF_STRING
                    | if app.form_choices[slot] == index {
                        MF_CHECKED
                    } else {
                        0
                    },
                index + 1,
                wide(&choice_label(slot, *value)).as_ptr(),
            );
        }
        let mut rect: RECT = zeroed();
        GetWindowRect(GetDlgItem(hwnd, id), &mut rect);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY,
            rect.left,
            rect.bottom,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        if selected > 0 {
            app.form_choices[slot] = selected as usize - 1;
            update_choice(hwnd, id, slot, app);
        }
    }
}
unsafe fn draw_toggle(item: &DRAWITEMSTRUCT, app: &App) {
    unsafe {
        let slot = if item.CtlID == 200 {
            0
        } else if item.CtlID == 201 {
            1
        } else {
            2
        };
        let on = app.form_checks[slot];
        let dpi = GetDpiForWindow(item.hwndItem).max(96);
        let mut text = [0u16; 128];
        GetWindowTextW(item.hwndItem, text.as_mut_ptr(), 128);
        let label = String::from_utf16_lossy(&text)
            .split('：')
            .next()
            .unwrap_or("")
            .to_string();
        let canvas = Canvas { dc: item.hDC, dpi };
        canvas.text(&label, 0, 0, 290, 28, 13, 400, app.fg());
        let x = item.rcItem.right * 96 / dpi as i32 - 40;
        canvas.rect(
            x,
            4,
            40,
            20,
            if on {
                BLUE
            } else {
                if app.dark {
                    rgb(67, 79, 94)
                } else {
                    rgb(191, 201, 214)
                }
            },
            20,
        );
        canvas.rect(
            x + if on { 23 } else { 3 },
            7,
            14,
            14,
            rgb(255, 255, 255),
            14,
        );
    }
}
unsafe fn pick_network(hwnd: HWND, networks: &[String]) {
    unsafe {
        if networks.is_empty() {
            return;
        }
        let menu = CreatePopupMenu();
        for (index, ssid) in networks.iter().take(100).enumerate() {
            AppendMenuW(menu, MF_STRING, index + 1, wide(ssid).as_ptr());
        }
        let mut rect: RECT = zeroed();
        GetWindowRect(GetDlgItem(hwnd, 202), &mut rect);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY,
            rect.left,
            rect.bottom,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        if selected > 0
            && let Some(ssid) = networks.get(selected as usize - 1)
        {
            SetWindowTextW(GetDlgItem(hwnd, 202), wide(ssid).as_ptr());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icon_contains_native_dpi_resources() {
        let bytes = include_bytes!("../../assets/app.ico");
        assert_eq!(&bytes[..4], &[0, 0, 1, 0]);
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let sizes: Vec<_> = (0..count)
            .map(|i| {
                let s = bytes[6 + i * 16];
                if s == 0 { 256 } else { s as u32 }
            })
            .collect();
        for size in [16, 20, 24, 28, 32, 40, 48, 64, 128, 256] {
            assert!(sizes.contains(&size));
        }
    }
    #[test]
    fn windows_loads_exact_size_from_embedded_icon() {
        unsafe {
            for size in [16, 20, 24, 32, 40, 48] {
                let icon = LoadImageW(
                    GetModuleHandleW(null()),
                    icon_resource(),
                    IMAGE_ICON,
                    size,
                    size,
                    0,
                ) as HICON;
                assert!(!icon.is_null(), "missing embedded icon");
                let mut info: ICONINFO = zeroed();
                assert_ne!(GetIconInfo(icon, &mut info), 0);
                let mut bitmap: BITMAP = zeroed();
                GetObjectW(
                    info.hbmColor,
                    size_of::<BITMAP>() as i32,
                    (&mut bitmap as *mut BITMAP).cast(),
                );
                assert_eq!((bitmap.bmWidth, bitmap.bmHeight), (size, size));
                DeleteObject(info.hbmColor);
                DeleteObject(info.hbmMask);
                DestroyIcon(icon);
            }
        }
    }
}
