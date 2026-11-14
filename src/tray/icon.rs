//! 托盘图标：渐变圆角方块 + 白色闪电，和页眉左上角的标志同一套几何。
//!
//! 不再是「电量数字」：托盘位置固定成品牌标，电量改由悬浮提示显示（一设备一行）。
//! 唯一的例外是低电（< 20%）时闪电变红 —— 数字没了之后，总得留个能一眼看见的
//! 告警。
//!
//! 图形全部自己光栅化进 32bpp DIB：GDI 不写 alpha，圆角与斜边会留下黑边，所以
//! 每个像素做 3×3 超采样求覆盖率，再按覆盖率合成 alpha。

use std::ffi::c_void;
use std::mem::size_of;

use windows_sys::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, CreateBitmap, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICONINFO,
};

/// 品牌渐变的起点（绿），和 `theme::LOGO_FROM` 一致。
const TILE_FROM: (u8, u8, u8) = (0x34, 0xD3, 0x99);
/// 品牌渐变的终点（青），和 `theme::LOGO_TO` 一致。
const TILE_TO: (u8, u8, u8) = (0x0E, 0xA5, 0xE9);
/// 常态下的闪电。
const BOLT: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);
/// 低电时的闪电。
const BOLT_LOW: (u8, u8, u8) = (0xFF, 0x52, 0x52);

/// 闪电画布在方块里的占比（小尺寸时放大，16px 下才认得出是闪电）。
///
/// 0.70 是反推出来的：闪电本体占画布高度 88%，乘出来约等于方块边长的
/// 62%，和页眉里 `icons::BOLT_RATIO`（0.62）一致。
const ART_REGULAR: f32 = 0.70;
/// 小图标（≤20px）用的占比。
const ART_SMALL: f32 = 0.82;
/// 圆角半径相对边长的比例：设计稿里 28px 方块用 8px 圆角。
const RADIUS: f32 = 8.0 / 28.0;
/// 小图标降低渐变对比，免得 16px 里糊成一团色块。
const SMALL_CONTRAST: f32 = 0.35;

/// 闪电多边形：Bootstrap `lightning-fill` 的轮廓，按包围盒等比缩放到
/// 0.06–0.94 的画布里（宽 0.55、高 0.88），和 `assets/icons/bolt.svg` 同形。
const BOLT_SHAPE: [(f32, f32); 12] = [
    (0.3643, 0.0800),
    (0.3910, 0.0600),
    (0.6137, 0.0600),
    (0.6401, 0.0966),
    (0.5410, 0.3940),
    (0.7529, 0.3940),
    (0.7749, 0.4390),
    (0.3852, 0.9400),
    (0.3366, 0.9147),
    (0.4368, 0.5889),
    (0.2518, 0.5889),
    (0.2251, 0.5532),
];

/// 托盘图标该用多大的像素尺寸。
///
/// 按系统 DPI 换算：100% → 16、125% → 20、150% → 24、200% → 32。让 Windows
/// 拿到原生尺寸的位图，比自己画 32px 再缩小清晰得多。
#[must_use]
pub fn icon_size() -> i32 {
    // SAFETY: 无参数的系统查询，失败时返回 0。
    let dpi = unsafe { GetDpiForSystem() };
    let dpi = if dpi == 0 { 96 } else { dpi };
    ((dpi as i32 * 16 + 48) / 96).clamp(16, 64)
}

/// 渲染一张图标，返回自上而下的 RGBA 像素。
///
/// 抽成纯函数是为了让 exe 的 `.ico` 生成器（`app/examples/gen_icon.rs`）能复用
/// 同一份几何 —— 托盘和 exe 图标永远是同一个图案。
#[must_use]
pub fn render_rgba(size: i32, low: bool) -> Vec<u8> {
    let size = size.max(8);
    let side = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let art = if size <= 20 { ART_SMALL } else { ART_REGULAR };
    let inset = (1.0 - art) / 2.0;
    let bolt = if low { BOLT_LOW } else { BOLT };

    for y in 0..size {
        for x in 0..size {
            let mut tile_hits = 0u32;
            let mut bolt_hits = 0u32;
            for sy in 0..3 {
                for sx in 0..3 {
                    let fx = x as f32 + (sx as f32 + 0.5) / 3.0;
                    let fy = y as f32 + (sy as f32 + 0.5) / 3.0;
                    if inside_tile(fx, fy, side) {
                        tile_hits += 1;
                        let u = (fx / side - inset) / art;
                        let v = (fy / side - inset) / art;
                        if inside_polygon(u, v, &BOLT_SHAPE) {
                            bolt_hits += 1;
                        }
                    }
                }
            }
            if tile_hits == 0 {
                continue;
            }

            let tile_coverage = tile_hits as f32 / 9.0;
            let bolt_coverage = bolt_hits as f32 / 9.0;
            let alpha = bolt_coverage + tile_coverage * (1.0 - bolt_coverage);
            if alpha <= 0.0 {
                continue;
            }

            let base = gradient(x, y, size);
            let color = (
                (f32::from(bolt.0) * bolt_coverage
                    + base.0 * tile_coverage * (1.0 - bolt_coverage))
                    / alpha,
                (f32::from(bolt.1) * bolt_coverage
                    + base.1 * tile_coverage * (1.0 - bolt_coverage))
                    / alpha,
                (f32::from(bolt.2) * bolt_coverage
                    + base.2 * tile_coverage * (1.0 - bolt_coverage))
                    / alpha,
            );

            let offset = ((y * size + x) * 4) as usize;
            out[offset] = color.0.round().clamp(0.0, 255.0) as u8;
            out[offset + 1] = color.1.round().clamp(0.0, 255.0) as u8;
            out[offset + 2] = color.2.round().clamp(0.0, 255.0) as u8;
            out[offset + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// 渐变底色：135° 方向，绿→青。
fn gradient(x: i32, y: i32, size: i32) -> (f32, f32, f32) {
    let span = (2 * (size - 1)).max(1) as f32;
    let t = ((x + y) as f32 / span).clamp(0.0, 1.0);

    let (from, to) = if size <= 20 {
        // 小图里对比太强会把闪电吃掉：两边各往中间收一点。
        let mid = (
            (f32::from(TILE_FROM.0) + f32::from(TILE_TO.0)) / 2.0,
            (f32::from(TILE_FROM.1) + f32::from(TILE_TO.1)) / 2.0,
            (f32::from(TILE_FROM.2) + f32::from(TILE_TO.2)) / 2.0,
        );
        (blend(TILE_FROM, mid, SMALL_CONTRAST), blend(TILE_TO, mid, SMALL_CONTRAST))
    } else {
        (to_f32(TILE_FROM), to_f32(TILE_TO))
    };
    (
        from.0 + (to.0 - from.0) * t,
        from.1 + (to.1 - from.1) * t,
        from.2 + (to.2 - from.2) * t,
    )
}

fn to_f32(color: (u8, u8, u8)) -> (f32, f32, f32) {
    (
        f32::from(color.0),
        f32::from(color.1),
        f32::from(color.2),
    )
}

fn blend(from: (u8, u8, u8), to: (f32, f32, f32), amount: f32) -> (f32, f32, f32) {
    (
        f32::from(from.0) + (to.0 - f32::from(from.0)) * amount,
        f32::from(from.1) + (to.1 - f32::from(from.1)) * amount,
        f32::from(from.2) + (to.2 - f32::from(from.2)) * amount,
    )
}

/// 点是否落在圆角方块内。
fn inside_tile(x: f32, y: f32, size: f32) -> bool {
    if x < 0.0 || y < 0.0 || x >= size || y >= size {
        return false;
    }
    let radius = RADIUS * size;
    let cx = x.clamp(radius, size - radius);
    let cy = y.clamp(radius, size - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

/// 点在多边形内（奇偶规则，闪电是简单多边形）。
fn inside_polygon(x: f32, y: f32, points: &[(f32, f32)]) -> bool {
    if x < 0.0 || y < 0.0 || x > 1.0 || y > 1.0 {
        return false;
    }
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 画一张托盘图标。调用方负责在用完后 [`destroy_icon`]。
#[must_use]
pub fn make_icon(low: bool) -> HICON {
    let size = icon_size();
    let pixels = render_rgba(size, low);

    // SAFETY: 每个句柄都有配对的释放路径，失败时逐级返回空句柄。
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        if dc.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return std::ptr::null_mut();
        }

        let mut header: BITMAPV5HEADER = std::mem::zeroed();
        header.bV5Size = size_of::<BITMAPV5HEADER>() as u32;
        header.bV5Width = size;
        // 负高度 = 自上而下，第 0 行就是屏幕最上面一行。
        header.bV5Height = -size;
        header.bV5Planes = 1;
        header.bV5BitCount = 32;
        header.bV5Compression = BI_BITFIELDS;
        header.bV5RedMask = 0x00FF_0000;
        header.bV5GreenMask = 0x0000_FF00;
        header.bV5BlueMask = 0x0000_00FF;
        header.bV5AlphaMask = 0xFF00_0000;

        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(
            dc,
            std::ptr::from_ref(&header).cast::<BITMAPINFO>(),
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            DeleteDC(dc);
            ReleaseDC(std::ptr::null_mut(), screen);
            return std::ptr::null_mut();
        }

        // DIB 要 BGRA，渲染器给的是 RGBA —— 这里换一下通道顺序。
        let len = (size * size * 4) as usize;
        // SAFETY: CreateDIBSection 分配的正是 size*size*4 字节。
        let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), len);
        for index in 0..(size * size) as usize {
            let src = index * 4;
            dst[src] = pixels[src + 2];
            dst[src + 1] = pixels[src + 1];
            dst[src + 2] = pixels[src];
            dst[src + 3] = pixels[src + 3];
        }

        let old = SelectObject(dc, dib);
        SelectObject(dc, old);
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), screen);

        // 1bpp 掩码：全 0 表示不遮挡颜色位图，透明度交给 alpha 通道。
        let stride = (size as usize).div_ceil(32) * 4;
        let mask_bits = vec![0u8; stride * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr().cast());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: dib,
        };
        let icon = CreateIconIndirect(&info);

        if !mask.is_null() {
            DeleteObject(mask);
        }
        DeleteObject(dib);
        icon
    }
}

/// 销毁一张由 [`make_icon`] 创建的图标。
///
/// 句柄是 `*mut c_void` 别名，但这里只是把它交回系统，不解引用任何内存。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn destroy_icon(icon: HICON) {
    if icon.is_null() {
        return;
    }
    // SAFETY: 调用方保证该图标不再被任何窗口使用，且只销毁一次。
    unsafe {
        DestroyIcon(icon);
    }
}

#[cfg(test)]
mod tests {
    use super::{BOLT_SHAPE, inside_polygon, inside_tile, render_rgba};

    /// 闪电中心（0.5, 0.5）落在多边形里：坐标抄错时这条会先炸。
    #[test]
    fn bolt_center_is_inside() {
        assert!(inside_polygon(0.5, 0.5, &BOLT_SHAPE));
    }

    /// 四个角在方块外（被圆角切掉）。
    #[test]
    fn corners_are_outside_tile() {
        for &(x, y) in &[(0.1, 0.1), (31.9, 0.1), (0.1, 31.9), (31.9, 31.9)] {
            assert!(!inside_tile(x, y, 32.0));
        }
        assert!(inside_tile(16.0, 16.0, 32.0));
    }

    /// 渲染出来的中心像素必须不透明；低电时闪电由白变红，绿通道明显下降。
    #[test]
    fn renders_opaque_center() {
        let normal = render_rgba(32, false);
        let low = render_rgba(32, true);
        let center = (16 * 32 + 16) * 4;
        assert_eq!(normal[center + 3], 255);
        assert_eq!(low[center + 3], 255);
        // 红通道两边都是 255，靠绿/蓝通道区分白与红。
        assert_eq!(low[center], 255);
        assert_eq!(normal[center + 1], 255);
        assert!(low[center + 1] < 128);
        assert!(low[center + 2] < 128);
    }
}
