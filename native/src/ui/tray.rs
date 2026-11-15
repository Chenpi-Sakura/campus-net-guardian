use std::{io, ptr};

use anyhow::{Context, Result};
use image::imageops::FilterType;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId,
    menu::{Menu, MenuEvent, MenuId, MenuItem},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0},
    System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject},
    UI::{
        HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi},
        WindowsAndMessaging::{SM_CXSMICON, SM_CYSMICON},
    },
};

const MUTEX_NAME: &str = "Local\\CampusGuardian.Native.2";
const RESTORE_EVENT_NAME: &str = "Local\\CampusGuardian.Native.2.Restore";

/// 托盘输入转换为主界面处理的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Toggle,
    Show,
    Login,
    Pause,
    Exit,
}

/// 托盘及菜单必须在 Iced 的窗口线程创建、轮询和销毁。
pub(super) struct Tray {
    // 先移除托盘图标，再释放菜单项和恢复事件。
    icon: TrayIcon,
    actions: MenuActions,
    pause: MenuItem,
    restore: Option<OwnedHandle>,
    accent: [u8; 3],
    icon_size: (u32, u32),
}

impl Tray {
    /// 在窗口事件循环开始后创建托盘；预览实例不参与实际应用的单实例通信。
    pub(super) fn new(preview: bool, accent: [u8; 3]) -> Result<Self> {
        let show = MenuItem::new("显示主面板", true, None);
        let login = MenuItem::new("立即登录", true, None);
        let pause = MenuItem::new("暂停守护", true, None);
        let exit = MenuItem::new("退出", true, None);
        let actions = MenuActions {
            show: show.id().clone(),
            login: login.id().clone(),
            pause: pause.id().clone(),
            exit: exit.id().clone(),
        };
        let menu = Menu::with_items(&[&show, &login, &pause, &exit]).context("无法创建托盘菜单")?;
        let restore = if preview {
            None
        } else {
            Some(create_restore_event()?)
        };
        let icon_size = icon_size();
        let icon = TrayIconBuilder::new()
            .with_icon(load_icon(accent, icon_size)?)
            .with_tooltip(if preview {
                "CUIT Helper · 界面预览"
            } else {
                "CUIT Helper"
            })
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .context("无法创建系统托盘图标")?;
        Ok(Self {
            icon,
            actions,
            pause,
            restore,
            accent,
            icon_size,
        })
    }

    /// 主题颜色或系统 DPI 改变时更新图标；失败保留原图及缓存，以便稍后重试。
    pub(super) fn set_accent(&mut self, accent: [u8; 3]) -> Result<()> {
        let icon_size = icon_size();
        if self.accent == accent && self.icon_size == icon_size {
            return Ok(());
        }
        self.icon
            .set_icon(Some(load_icon(accent, icon_size)?))
            .context("无法更新系统托盘图标")?;
        self.accent = accent;
        self.icon_size = icon_size;
        Ok(())
    }

    /// 非阻塞地读取托盘输入；隐藏面板后也应由主事件循环定期调用。
    pub(super) fn poll(&self) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.restore.as_ref().is_some_and(|event| {
            // 句柄由本对象独占管理且始终有效；0 毫秒不会等待 UI 线程。
            unsafe { WaitForSingleObject(event.0, 0) == WAIT_OBJECT_0 }
        }) {
            actions.push(Action::Show);
        }
        for event in TrayIconEvent::receiver().try_iter() {
            if let Some(action) = tray_action(self.icon.id(), &event) {
                actions.push(action);
            }
        }
        for event in MenuEvent::receiver().try_iter() {
            if let Some(action) = self.actions.action(&event.id) {
                actions.push(action);
            }
        }
        actions
    }

    /// 使托盘菜单始终反映后台实际的暂停状态。
    pub(super) fn set_paused(&self, paused: bool) {
        self.pause.set_text(if paused {
            "恢复守护"
        } else {
            "暂停守护"
        });
    }
}

struct MenuActions {
    show: MenuId,
    login: MenuId,
    pause: MenuId,
    exit: MenuId,
}

impl MenuActions {
    fn action(&self, id: &MenuId) -> Option<Action> {
        if id == &self.show {
            Some(Action::Show)
        } else if id == &self.login {
            Some(Action::Login)
        } else if id == &self.pause {
            Some(Action::Pause)
        } else if id == &self.exit {
            Some(Action::Exit)
        } else {
            None
        }
    }
}

fn tray_action(id: &TrayIconId, event: &TrayIconEvent) -> Option<Action> {
    match event {
        TrayIconEvent::Click {
            id: event_id,
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } if event_id == id => Some(Action::Toggle),
        _ => None,
    }
}

fn icon_size() -> (u32, u32) {
    let (width, height) = unsafe {
        let dpi = GetDpiForSystem().max(96);
        (
            GetSystemMetricsForDpi(SM_CXSMICON, dpi),
            GetSystemMetricsForDpi(SM_CYSMICON, dpi),
        )
    };
    let width = if width > 0 { width as u32 } else { 16 };
    let height = if height > 0 { height as u32 } else { 16 };
    (width, height)
}

fn load_icon(accent: [u8; 3], (width, height): (u32, u32)) -> Result<Icon> {
    let mut image = image::load_from_memory_with_format(
        include_bytes!("../../../logo.png"),
        image::ImageFormat::Png,
    )
    .context("无法解码应用托盘图标")?
    .resize_exact(width, height, FilterType::Lanczos3)
    .into_rgba8();
    // 仅替换 RGB，保留缩放后的 alpha 和抗锯齿边缘；from_rgba 接收直 alpha。
    for pixel in image.pixels_mut() {
        pixel.0[..3].copy_from_slice(&accent);
    }
    Icon::from_rgba(image.into_raw(), width, height).context("无法载入应用托盘图标")
}

/// 持有单实例对象；预览互相独立，重复启动只请求已有实例显示面板。
pub(super) struct InstanceGuard {
    _mutex: Option<OwnedHandle>,
    _restore: Option<OwnedHandle>,
}

impl InstanceGuard {
    /// 返回 `None` 表示已有正常实例，调用方应在启动后台前结束此次启动。
    pub(super) fn acquire(preview: bool) -> Result<Option<Self>> {
        if preview {
            return Ok(Some(Self {
                _mutex: None,
                _restore: None,
            }));
        }
        let name = wide(MUTEX_NAME);
        // 不取得 mutex 所有权，只在进程生命周期内保留命名对象。
        let raw = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        // CreateMutexW 的成功结果也携带 ERROR_ALREADY_EXISTS，必须立即读取。
        let error = unsafe { GetLastError() };
        let mutex = OwnedHandle::new(raw).context("无法创建应用单实例锁")?;
        let restore = create_restore_event()?;
        if error == ERROR_ALREADY_EXISTS {
            if unsafe { SetEvent(restore.0) } == 0 {
                return Err(io::Error::last_os_error()).context("无法恢复已有应用面板");
            }
            return Ok(None);
        }
        Ok(Some(Self {
            _mutex: Some(mutex),
            _restore: Some(restore),
        }))
    }
}

fn create_restore_event() -> Result<OwnedHandle> {
    let name = wide(RESTORE_EVENT_NAME);
    // 自动重置，确保一次重复启动只产生一次显示请求。
    let raw = unsafe { CreateEventW(ptr::null(), 0, 0, name.as_ptr()) };
    OwnedHandle::new(raw).context("无法创建应用面板恢复事件")
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // 仅接收本模块创建的有效句柄，句柄不会向外暴露或被提前关闭。
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(id: &str, button: MouseButton, button_state: MouseButtonState) -> TrayIconEvent {
        TrayIconEvent::Click {
            id: TrayIconId::new(id),
            position: Default::default(),
            rect: Default::default(),
            button,
            button_state,
        }
    }

    #[test]
    fn only_own_left_button_release_toggles_panel() {
        let id = TrayIconId::new("normal");
        assert_eq!(
            tray_action(
                &id,
                &click("normal", MouseButton::Left, MouseButtonState::Up)
            ),
            Some(Action::Toggle)
        );
        for event in [
            click("preview", MouseButton::Left, MouseButtonState::Up),
            click("normal", MouseButton::Left, MouseButtonState::Down),
            click("normal", MouseButton::Right, MouseButtonState::Up),
            click("normal", MouseButton::Middle, MouseButtonState::Up),
        ] {
            assert_eq!(tray_action(&id, &event), None);
        }
    }

    #[test]
    fn menu_actions_are_isolated_from_unrelated_menu_events() {
        let actions = MenuActions {
            show: MenuId::new("show"),
            login: MenuId::new("login"),
            pause: MenuId::new("pause"),
            exit: MenuId::new("exit"),
        };
        for (id, expected) in [
            (&actions.show, Action::Show),
            (&actions.login, Action::Login),
            (&actions.pause, Action::Pause),
            (&actions.exit, Action::Exit),
        ] {
            assert_eq!(actions.action(id), Some(expected));
        }
        assert_eq!(actions.action(&MenuId::new("other-instance")), None);
    }
}
