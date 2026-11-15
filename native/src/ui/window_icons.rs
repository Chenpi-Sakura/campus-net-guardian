//! 在 Iced 窗口线程应用当前 DPI 的原生图标及标题栏明暗模式。

use iced::{Task, window};
use image::{RgbaImage, imageops::FilterType};
use std::{cell::RefCell, mem::size_of, ptr, rc::Rc};
use window::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::{
    Win32::{
        Foundation::HWND,
        Graphics::{
            Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute},
            Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection,
                DIB_RGB_COLORS, DeleteObject, HBITMAP,
            },
        },
        UI::{
            HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
            WindowsAndMessaging::{
                CreateIconIndirect, DestroyIcon, HICON, ICON_BIG, ICON_SMALL, ICONINFO, SM_CXICON,
                SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
            },
        },
    },
    core::BOOL,
};

thread_local! {
    // 应用只有一个面板；同 DPI、同强调色复用图标，替换后释放旧图标。
    static ICONS: RefCell<Option<Rc<WindowIcons>>> = const { RefCell::new(None) };
}

struct OwnedIcon(HICON);

impl OwnedIcon {
    fn from_image(source: &RgbaImage, width: i32, height: i32, accent: [u8; 3]) -> Option<Self> {
        let width = u32::try_from(width).ok()?;
        let height = u32::try_from(height).ok()?;
        if !(1..=1024).contains(&width) || !(1..=1024).contains(&height) {
            return None;
        }
        let pixels = tinted_bgra(source, width, height, accent);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = ptr::null_mut();
        // Top-down 32 位 DIB 的每行天然按 DWORD 对齐；它从未被选入 DC。
        let color = OwnedBitmap::new(unsafe {
            CreateDIBSection(
                ptr::null_mut(),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                ptr::null_mut(),
                0,
            )
        })?;
        if bits.is_null() {
            return None;
        }
        // 系统按上述尺寸分配全部 DIB 像素；输入长度恰为 width * height * 4。
        unsafe { ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len()) };
        // ARGB 图标使用颜色位图的 alpha；零 AND mask 避免未初始化的单色像素。
        // CreateBitmap 要求每一行按 WORD 对齐，不能沿用 32 位颜色图的步长。
        let mask_pixels = vec![0u8; (width as usize).div_ceil(16) * 2 * height as usize];
        let mask = OwnedBitmap::new(unsafe {
            CreateBitmap(
                width as i32,
                height as i32,
                1,
                1,
                mask_pixels.as_ptr().cast(),
            )
        })?;
        let icon_info = ICONINFO {
            fIcon: 1,
            hbmMask: mask.0,
            hbmColor: color.0,
            ..Default::default()
        };
        // CreateIconIndirect 复制两张位图；局部 RAII 在成功和失败路径都立即释放位图。
        let icon = unsafe { CreateIconIndirect(&icon_info) };
        if icon.is_null() {
            None
        } else {
            Some(Self(icon))
        }
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // 仅释放本模块创建的图标，不释放 Iced 或系统拥有的图标。
        unsafe { DestroyIcon(self.0) };
    }
}

struct OwnedBitmap(HBITMAP);

impl OwnedBitmap {
    fn new(bitmap: HBITMAP) -> Option<Self> {
        if bitmap.is_null() {
            None
        } else {
            Some(Self(bitmap))
        }
    }
}

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        // 位图从未选入 DC；CreateIconIndirect 已复制其像素，不再借用原对象。
        unsafe { DeleteObject(self.0) };
    }
}

fn tinted_bgra(source: &RgbaImage, width: u32, height: u32, accent: [u8; 3]) -> Vec<u8> {
    let mut pixels =
        image::imageops::resize(source, width, height, FilterType::Lanczos3).into_raw();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        for (channel, color) in pixel[..3].iter_mut().zip(accent.into_iter().rev()) {
            *channel = ((u16::from(color) * alpha + 127) / 255) as u8;
        }
    }
    pixels
}

struct WindowIcons {
    hwnd: HWND,
    dpi: u32,
    accent: [u8; 3],
    small: OwnedIcon,
    big: OwnedIcon,
}

impl WindowIcons {
    fn load(hwnd: HWND, dpi: u32, accent: [u8; 3]) -> Option<Self> {
        let source = image::load_from_memory_with_format(
            include_bytes!("../../../logo.png"),
            image::ImageFormat::Png,
        )
        .ok()?
        .into_rgba8();
        let metric = |index, fallback: u64| {
            let value = unsafe { GetSystemMetricsForDpi(index, dpi) };
            if value > 0 {
                value
            } else {
                ((fallback * u64::from(dpi) + 48) / 96).min(i32::MAX as u64) as i32
            }
        };
        Some(Self {
            hwnd,
            dpi,
            accent,
            small: OwnedIcon::from_image(
                &source,
                metric(SM_CXSMICON, 16),
                metric(SM_CYSMICON, 16),
                accent,
            )?,
            big: OwnedIcon::from_image(
                &source,
                metric(SM_CXICON, 32),
                metric(SM_CYICON, 32),
                accent,
            )?,
        })
    }
}

/// 在窗口所属线程刷新大小图标和标题栏主题；不支持的外观属性保留系统默认行为。
pub(super) fn refresh(id: window::Id, dark: bool, accent: [u8; 3]) -> Task<()> {
    window::run(id, move |native_window| {
        let Ok(handle) = HasWindowHandle::window_handle(native_window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = handle.hwnd.get() as HWND;
        let dark_enabled = BOOL::from(dark);
        // window::run 保证本回调在原生窗口线程运行，借用期间 HWND 始终有效。
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&dark_enabled as *const BOOL).cast(),
                size_of::<BOOL>() as u32,
            );
        }
        let dpi = match unsafe { GetDpiForWindow(hwnd) } {
            0 => 96,
            dpi => dpi,
        };
        let Some((icons, previous)) = ICONS.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(icons) = slot
                .as_ref()
                .filter(|icons| icons.hwnd == hwnd && icons.dpi == dpi && icons.accent == accent)
            {
                return Some((Rc::clone(icons), None));
            }
            let icons = Rc::new(WindowIcons::load(hwnd, dpi, accent)?);
            let previous = slot.replace(Rc::clone(&icons));
            Some((icons, previous))
        }) else {
            // 资源加载失败时保留 Iced 原有图标，不将窗口图标清空。
            return;
        };
        // 不在 SendMessageW 期间持有 RefCell 借用；Rc 保证同步窗口消息期间资源有效。
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                ICON_SMALL as usize,
                icons.small.0 as isize,
            );
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icons.big.0 as isize);
        }
        // 新图标已同时安装后，才释放本模块以前持有的两枚图标。
        drop(previous);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_icon_pixels_are_bgra_premultiplied_and_keep_alpha() {
        let source =
            RgbaImage::from_raw(3, 1, vec![90, 80, 70, 0, 90, 80, 70, 128, 90, 80, 70, 255])
                .unwrap();
        assert_eq!(
            tinted_bgra(&source, 3, 1, [3, 126, 254]),
            [0, 0, 0, 0, 127, 63, 2, 128, 254, 126, 3, 255]
        );
    }
}
