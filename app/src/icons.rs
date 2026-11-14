//! 页眉图标：全部走 SVG。
//!
//! GPUI 自带 `svg()` 元素，底层是 `resvg`：它按像素覆盖率光栅化，再把结果当
//! alpha 蒙版用 `text_color` 上色 —— 同一份文件既能跟主题换色，又能在任意 DPI
//! 下保持平滑。之前手写 `PathBuilder` 的版本在 20px 上细斜线全是台阶，就是因
//! 为 GPU 三角化 + 多重采样在这种尺寸下不够看。
//!
//! 图形取自 Lucide（MIT），24 网格、2px 描边。`x` 改成了 4.5→19.5 的版本：
//! 官方那条只有 12 个单位宽，和 17 个单位的月牙摆在一起明显偏小。

use gpui_kit::*;

/// 关闭按钮的图标画布边长（`x` 的笔画只占 13/24，画布得给足）。
pub const SIZE: f32 = 18.0;

/// 其余三枚的视觉尺寸：它们本身几乎铺满 24 网格，同样 18 的画布会显得比 `x`
/// 大一圈；收到 15.0 后四枚的墨迹宽度都是 ~11px（实测 16–17 物理像素 @150%）。
pub const OPTICAL: f32 = 15.0;

const MOON: &[u8] = include_bytes!("../assets/icons/moon.svg");
const SUN: &[u8] = include_bytes!("../assets/icons/sun.svg");
const REFRESH: &[u8] = include_bytes!("../assets/icons/refresh.svg");
const CLOSE: &[u8] = include_bytes!("../assets/icons/close.svg");
const BOLT: &[u8] = include_bytes!("../assets/icons/bolt.svg");

/// 品牌闪电：占色块的比例，和托盘/exe 图标保持一致。
pub const BOLT_RATIO: f32 = 0.62;

fn icon(bytes: &'static [u8], size: f32, color: u32) -> Svg {
    svg()
        .data(bytes)
        .flex_none()
        .w(px(size))
        .h(px(size))
        .text_color(rgb(color))
}

/// 品牌闪电，`side` 是外层色块的边长。
pub fn bolt(side: f32, color: u32) -> Svg {
    icon(BOLT, side * BOLT_RATIO, color)
}

/// 月牙。
pub fn moon(color: u32) -> Svg {
    icon(MOON, OPTICAL, color)
}

/// 太阳。
pub fn sun(color: u32) -> Svg {
    icon(SUN, OPTICAL, color)
}

/// 刷新（顺时针圆弧 + 箭头）。
pub fn refresh(color: u32) -> Svg {
    icon(REFRESH, OPTICAL, color)
}

/// 关闭。
pub fn close(color: u32) -> Svg {
    icon(CLOSE, SIZE, color)
}

/// 转到 `angle` 弧度的刷新图标。
pub fn refresh_at(angle: f32, color: u32) -> impl IntoElement {
    refresh(color).with_transformation(Transformation::rotate(radians(angle)))
}
