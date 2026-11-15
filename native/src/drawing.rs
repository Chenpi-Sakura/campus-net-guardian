//! GDI+ 抗锯齿几何绘制；不可用时退回普通 GDI，窗口仍能正常使用。

use anyhow::{Result, bail};
use std::{
    marker::PhantomData,
    ptr::{self, NonNull},
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::{
        Gdi::{self as gdi, HDC},
        GdiPlus as gp,
    },
};

static SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// GDI+ 生命周期，必须保留到所有窗口绘制结束；只能在创建它的 UI 线程使用。
#[must_use = "必须保留绘图会话，直到所有窗口绘制结束"]
pub struct DrawingSession {
    token: usize,
    // GDI+ 图形对象和 HDC 都绑定 UI 绘制时序，禁止把本会话移交到后台线程。
    _ui_thread: PhantomData<Rc<()>>,
}

impl DrawingSession {
    /// 启动 Windows 内置 GDI+，不加载外部图片编码器。
    pub fn start() -> Result<Self> {
        let input = gp::GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 1,
        };
        let mut token = 0;
        // SAFETY: 参数均为有效的局部存储，成功返回的 token 由本类型唯一管理。
        let status = unsafe { gp::GdiplusStartup(&mut token, &input, ptr::null_mut()) };
        if status != gp::Ok {
            bail!("抗锯齿绘图初始化失败（状态 {status}），已使用基础绘图模式");
        }
        SESSIONS.fetch_add(1, Ordering::Release);
        Ok(Self {
            token,
            _ui_thread: PhantomData,
        })
    }
}

impl Drop for DrawingSession {
    fn drop(&mut self) {
        SESSIONS.fetch_sub(1, Ordering::AcqRel);
        // SAFETY: token 仅来自成功的 GdiplusStartup，且只会关闭一次。
        unsafe { gp::GdiplusShutdown(self.token) };
    }
}

struct Graphics(NonNull<gp::GpGraphics>);
impl Drop for Graphics {
    fn drop(&mut self) {
        // SAFETY: 持有且唯一管理 GdipCreateFromHDC 创建的图形对象。
        unsafe {
            gp::GdipDeleteGraphics(self.0.as_ptr());
        }
    }
}
struct Path(NonNull<gp::GpPath>);
impl Drop for Path {
    fn drop(&mut self) {
        // SAFETY: 持有且唯一管理 GdipCreatePath 创建的路径。
        unsafe {
            gp::GdipDeletePath(self.0.as_ptr());
        }
    }
}
struct Brush(NonNull<gp::GpSolidFill>);
impl Drop for Brush {
    fn drop(&mut self) {
        // SAFETY: SolidFill 是 GpBrush 的具体实现，释放时使用其基类型指针。
        unsafe {
            gp::GdipDeleteBrush(self.0.as_ptr().cast());
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Geometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: f32,
}

fn geometry(rect: &RECT, radius: f32) -> Option<Geometry> {
    let width = i64::from(rect.right) - i64::from(rect.left);
    let height = i64::from(rect.bottom) - i64::from(rect.top);
    if width <= 0 || height <= 0 || width > i64::from(i32::MAX) || height > i64::from(i32::MAX) {
        return None;
    }
    let radius = if radius.is_finite() {
        radius.max(0.0)
    } else {
        0.0
    };
    Some(Geometry {
        x: rect.left,
        y: rect.top,
        width: width as i32,
        height: height as i32,
        radius: radius.min(width.min(height) as f32 / 2.0),
    })
}

const fn colorref_to_argb(color: u32) -> u32 {
    0xff00_0000 | ((color & 0xff) << 16) | (color & 0xff00) | ((color >> 16) & 0xff)
}

fn check(status: gp::Status) -> std::result::Result<(), gp::Status> {
    if status == gp::Ok {
        Ok(())
    } else {
        Err(status)
    }
}

/// 在指定物理像素矩形内绘制不带边框的抗锯齿圆角，颜色使用 Win32 COLORREF。
///
/// `radius` 是真实圆角半径，自动限制为短边的一半；无效矩形不绘制。
/// 返回 `true` 表示抗锯齿绘制成功或矩形无需绘制，`false` 表示已退回 GDI。
/// 应先清理目标背景，避免多次绘制把半透明边缘反复叠加。
///
/// # Safety
/// `dc` 必须是调用线程持有的有效 HDC，且本次调用期间不能有其它 GDI+ 对象同时绘制它。
/// 启动的 DrawingSession 必须保留到本调用结束；调用方不得在其它线程同时关闭该会话。
pub unsafe fn rounded_rect(dc: HDC, rect: &RECT, radius: f32, color: u32) -> bool {
    let Some(shape) = geometry(rect, radius) else {
        return true;
    };
    if dc.is_null() {
        return false;
    }
    let mut clipping = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: HDC 有效，由调用者保证；GetClipBox 只写入局部 RECT。
    let region = unsafe { gdi::GetClipBox(dc, &mut clipping) };
    if region == gdi::NULLREGION {
        return true;
    }
    if region == gdi::ERROR {
        clipping = *rect;
    } else {
        clipping.left = clipping.left.max(rect.left);
        clipping.top = clipping.top.max(rect.top);
        clipping.right = clipping.right.min(rect.right);
        clipping.bottom = clipping.bottom.min(rect.bottom);
    }
    if geometry(&clipping, 0.0).is_none() {
        return true;
    }

    // SAFETY: 图形、路径和画刷全部局部持有，返回前都会释放，GDI 回退时不再持有图形对象。
    if SESSIONS.load(Ordering::Acquire) > 0
        && unsafe { draw_antialiased(dc, shape, &clipping, color) }.is_ok()
    {
        true
    } else {
        unsafe { draw_gdi(dc, rect, shape, color) };
        false
    }
}

unsafe fn draw_antialiased(
    dc: HDC,
    shape: Geometry,
    clip: &RECT,
    color: u32,
) -> std::result::Result<(), gp::Status> {
    // SAFETY: 外层已校验矩形；传给 GDI+ 的对象均在此作用域内有效且由 RAII 释放。
    unsafe {
        let mut graphics = ptr::null_mut();
        check(gp::GdipCreateFromHDC(dc, &mut graphics))?;
        let graphics = Graphics(NonNull::new(graphics).ok_or(gp::GenericError)?);
        check(gp::GdipSetPageUnit(graphics.0.as_ptr(), gp::UnitPixel))?;
        check(gp::GdipSetPageScale(graphics.0.as_ptr(), 1.0))?;
        check(gp::GdipSetSmoothingMode(
            graphics.0.as_ptr(),
            gp::SmoothingModeAntiAlias,
        ))?;
        check(gp::GdipSetPixelOffsetMode(
            graphics.0.as_ptr(),
            gp::PixelOffsetModeHighQuality,
        ))?;
        check(gp::GdipSetClipRectI(
            graphics.0.as_ptr(),
            clip.left,
            clip.top,
            clip.right - clip.left,
            clip.bottom - clip.top,
            gp::CombineModeIntersect,
        ))?;

        let mut path = ptr::null_mut();
        check(gp::GdipCreatePath(gp::FillModeWinding, &mut path))?;
        let path = Path(NonNull::new(path).ok_or(gp::GenericError)?);
        let (x, y, width, height) = (
            shape.x as f32,
            shape.y as f32,
            shape.width as f32,
            shape.height as f32,
        );
        if shape.radius == 0.0 {
            check(gp::GdipAddPathRectangle(
                path.0.as_ptr(),
                x,
                y,
                width,
                height,
            ))?;
        } else {
            let diameter = shape.radius * 2.0;
            for (arc_x, arc_y, angle) in [
                (x, y, 180.0),
                (x + width - diameter, y, 270.0),
                (x + width - diameter, y + height - diameter, 0.0),
                (x, y + height - diameter, 90.0),
            ] {
                check(gp::GdipAddPathArc(
                    path.0.as_ptr(),
                    arc_x,
                    arc_y,
                    diameter,
                    diameter,
                    angle,
                    90.0,
                ))?;
            }
            check(gp::GdipClosePathFigure(path.0.as_ptr()))?;
        }
        let mut brush = ptr::null_mut();
        check(gp::GdipCreateSolidFill(colorref_to_argb(color), &mut brush))?;
        let brush = Brush(NonNull::new(brush).ok_or(gp::GenericError)?);
        check(gp::GdipFillPath(
            graphics.0.as_ptr(),
            brush.0.as_ptr().cast(),
            path.0.as_ptr(),
        ))
    }
}

unsafe fn draw_gdi(dc: HDC, rect: &RECT, shape: Geometry, color: u32) {
    // SAFETY: 仅短暂选择本函数创建的画刷和系统空画笔，结束前恢复原对象后释放画刷。
    unsafe {
        let brush = gdi::CreateSolidBrush(color & 0x00ff_ffff);
        if brush.is_null() {
            return;
        }
        if shape.radius == 0.0 {
            gdi::FillRect(dc, rect, brush);
        } else {
            let old_brush = gdi::SelectObject(dc, brush);
            if !old_brush.is_null() && old_brush as isize != -1 {
                let old_pen = gdi::SelectObject(dc, gdi::GetStockObject(gdi::NULL_PEN));
                if !old_pen.is_null() && old_pen as isize != -1 {
                    let diameter = (shape.radius * 2.0).round() as i32;
                    gdi::RoundRect(
                        dc,
                        rect.left,
                        rect.top,
                        rect.right,
                        rect.bottom,
                        diameter,
                        diameter,
                    );
                    gdi::SelectObject(dc, old_pen);
                }
                gdi::SelectObject(dc, old_brush);
            }
        }
        gdi::DeleteObject(brush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_blue_becomes_opaque_argb_without_swapping_channels() {
        assert_eq!(colorref_to_argb(0x00fe_7e03), 0xff03_7efe);
        assert_eq!(colorref_to_argb(0x00ff_ffff), 0xffff_ffff);
        assert_eq!(colorref_to_argb(0xff00_0000), 0xff00_0000);
    }

    #[test]
    fn radius_is_finite_nonnegative_and_never_exceeds_half_short_side() {
        let rect = RECT {
            left: 10,
            top: 20,
            right: 110,
            bottom: 40,
        };
        assert_eq!(geometry(&rect, 100.0).unwrap().radius, 10.0);
        assert_eq!(geometry(&rect, -1.0).unwrap().radius, 0.0);
        assert_eq!(geometry(&rect, f32::NAN).unwrap().radius, 0.0);
        assert_eq!(geometry(&rect, f32::INFINITY).unwrap().radius, 0.0);
        assert!(
            geometry(
                &RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 10
                },
                2.0
            )
            .is_none()
        );
        assert!(
            geometry(
                &RECT {
                    left: i32::MIN,
                    top: 0,
                    right: i32::MAX,
                    bottom: 10
                },
                2.0
            )
            .is_none()
        );
    }

    struct BitmapDc {
        dc: HDC,
        bitmap: gdi::HBITMAP,
        old: gdi::HGDIOBJ,
        bits: *mut u32,
    }
    impl BitmapDc {
        unsafe fn new() -> Self {
            unsafe {
                let dc = gdi::CreateCompatibleDC(ptr::null_mut());
                assert!(!dc.is_null());
                let mut info: gdi::BITMAPINFO = std::mem::zeroed();
                info.bmiHeader.biSize = std::mem::size_of::<gdi::BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = 48;
                info.bmiHeader.biHeight = -48;
                info.bmiHeader.biPlanes = 1;
                info.bmiHeader.biBitCount = 32;
                info.bmiHeader.biCompression = gdi::BI_RGB;
                let mut bits = ptr::null_mut();
                let bitmap = gdi::CreateDIBSection(
                    dc,
                    &info,
                    gdi::DIB_RGB_COLORS,
                    &mut bits,
                    ptr::null_mut(),
                    0,
                );
                assert!(!bitmap.is_null() && !bits.is_null());
                let old = gdi::SelectObject(dc, bitmap);
                ptr::write_bytes(bits, 0xff, 48 * 48 * 4);
                Self {
                    dc,
                    bitmap,
                    old,
                    bits: bits.cast(),
                }
            }
        }
    }
    impl Drop for BitmapDc {
        fn drop(&mut self) {
            unsafe {
                gdi::SelectObject(self.dc, self.old);
                gdi::DeleteObject(self.bitmap);
                gdi::DeleteDC(self.dc);
            }
        }
    }

    #[test]
    fn native_rounding_has_blended_edge_pixels_and_respects_clip() {
        let _session = DrawingSession::start().unwrap();
        unsafe {
            let image = BitmapDc::new();
            gdi::IntersectClipRect(image.dc, 16, 0, 48, 48);
            let rect = RECT {
                left: 4,
                top: 4,
                right: 44,
                bottom: 44,
            };
            assert!(rounded_rect(image.dc, &rect, 12.0, 0x00fe_7e03));
            gdi::GdiFlush();
            let pixels = std::slice::from_raw_parts(image.bits, 48 * 48);
            assert!(
                pixels
                    .iter()
                    .enumerate()
                    .all(|(i, color)| i % 48 >= 16 || (color & 0x00ff_ffff) == 0x00ff_ffff)
            );
            assert_eq!(pixels[24 * 48 + 24] & 0x00ff_ffff, 0x0003_7efe);
            assert!(
                pixels.iter().any(|color| {
                    let red = (color >> 16) & 0xff;
                    let green = (color >> 8) & 0xff;
                    red > 3 && red < 255 && green > 126 && green < 255
                }),
                "圆角边缘应该含背景与主题色混合的过渡像素"
            );
        }
    }
}
