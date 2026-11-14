//! 一行设备：图标、型号、电量条、状态。
//!
//! 用的是「列表 + 横条」这套主流写法（Windows 设置、系统电池浮窗都是这个路子），
//! 而不是之前那种两张竖卡 + 闪电液面 —— 加设备只是多一行，不用改布局。

use gpui_kit::*;
use peripheral_battery::{DeviceKind, DeviceState};

use crate::theme::{
    BADGE, BAR_H, BAR_RADIUS, FONT_CN, FONT_LATIN, Palette, ROW_H, ROW_PAD_X, ROW_PAD_Y,
    ROW_RADIUS, alpha, level_hex,
};

/// 渲染一行设备。
pub fn row(device: &DeviceState, palette: &Palette, dark: bool) -> impl IntoElement {
    let percent = device.status.percent();
    let level = percent.unwrap_or(0);
    // 离线时不要沿用「低电量红」：那不是缺电，而是读不到 —— 两者含义完全不同。
    let accent_hex = match percent {
        Some(_) => level_hex(palette, level),
        None => palette.text_faint,
    };
    let caption = device.status.caption();
    let updated = device.updated_at.clone().unwrap_or_else(|| "--:--".to_owned());
    let sub = format!("{} · {}", device.kind.label(), device.transport);

    div()
        .w_full()
        .h(px(ROW_H))
        .flex_none()
        .rounded(px(ROW_RADIUS))
        .bg(rgb(palette.card))
        .border_1()
        .border_color(rgb(palette.card_border))
        .px(px(ROW_PAD_X))
        .py(px(ROW_PAD_Y))
        .flex()
        .items_center()
        .gap(px(11.0))
        .child(badge(device.kind, accent_hex, dark))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(7.0))
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(1.0))
                                .child(
                                    div()
                                        .font_family(FONT_LATIN)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_size(px(13.0))
                                        .text_color(rgb(palette.text))
                                        .child(device.name.clone()),
                                )
                                .child(
                                    div()
                                        .font_family(FONT_CN)
                                        .text_size(px(10.0))
                                        .text_color(rgb(palette.text_faint))
                                        .child(sub),
                                ),
                        )
                        .child(reading(percent, accent_hex, palette)),
                )
                .child(bar(percent, accent_hex, palette))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .font_family(FONT_CN)
                                .text_size(px(10.0))
                                .text_color(rgb(palette.text_muted))
                                .child(caption),
                        )
                        .child(
                            div()
                                .font_family(FONT_CN)
                                .text_size(px(9.5))
                                .text_color(rgb(palette.text_faint))
                                .child(format!("更新 {updated}")),
                        ),
                ),
        )
}

/// 右侧读数：大号数字 + 小号百分号。
fn reading(percent: Option<u8>, accent_hex: u32, palette: &Palette) -> impl IntoElement {
    let mut block = div().flex().items_end().gap(px(1.0));
    block = match percent {
        Some(value) => block
            .child(
                div()
                    .font_family(FONT_LATIN)
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(20.0))
                    .text_color(rgb(accent_hex))
                    .child(value.to_string()),
            )
            .child(
                div()
                    .pb(px(3.0))
                    .font_family(FONT_LATIN)
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(10.0))
                    .text_color(rgb(palette.text_faint))
                    .child("%"),
            ),
        None => block.child(
            div()
                .font_family(FONT_LATIN)
                .font_weight(FontWeight::BOLD)
                .text_size(px(18.0))
                .text_color(rgb(palette.text_faint))
                .child("--"),
        ),
    };
    block
}

/// 横向电量条。
///
/// 填充宽度用相对值（占容器的百分比），不是算好的像素 —— 早期版本按
/// 「窗口宽 - 内边距」估算可用宽度，漏掉了左边图标和间距，90% 以上就会
/// 冲出卡片右边。
fn bar(percent: Option<u8>, accent_hex: u32, palette: &Palette) -> impl IntoElement {
    let mut track = div()
        .w_full()
        .h(px(BAR_H))
        .flex_none()
        .rounded(px(BAR_RADIUS))
        .bg(rgb(palette.track));
    if let Some(value) = percent.filter(|value| *value > 0) {
        track = track.child(
            div()
                .w(relative(fill_ratio(value)))
                .h_full()
                .rounded(px(BAR_RADIUS))
                .bg(rgb(accent_hex)),
        );
    }
    track
}

/// 电量 → 填充比例。0 表示一排空槽，1 表示铺满，中间线性变化。
fn fill_ratio(percent: u8) -> f32 {
    f32::from(percent.min(100)) / 100.0
}

/// 左侧图标底座：一个带色调的圆角方块，里面是设备类别的线描图标。
fn badge(kind: DeviceKind, accent_hex: u32, dark: bool) -> impl IntoElement {
    // 暗色底上色调要更实一点才看得出来，亮色底反过来。
    let tint = if dark { 0x33 } else { 0x1F };
    div()
        .size(px(BADGE))
        .flex_none()
        .rounded(px(11.0))
        .bg(rgba(alpha(accent_hex, tint)))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph(kind, accent_hex))
}

/// 设备图标。用路径画，不依赖字体里有没有对应字形。
fn glyph(kind: DeviceKind, accent_hex: u32) -> impl IntoElement {
    canvas(
        |bounds, _, _| bounds,
        move |_, bounds, window, _| {
            let color = rgb(accent_hex);
            let width = bounds.size.width;
            let height = bounds.size.height;
            // 归一化坐标 → 实际像素。
            let at = |x: f32, y: f32| {
                point(
                    bounds.origin.x + width * x,
                    bounds.origin.y + height * y,
                )
            };
            let radius = |r: f32| point(width * r, height * r);

            match kind {
                DeviceKind::Keyboard => {
                    let mut body = PathBuilder::stroke(px(1.6));
                    rounded_rect(&mut body, &at, &radius, (0.05, 0.28, 0.95, 0.72), 0.12);
                    if let Ok(path) = body.build() {
                        window.paint_path(path, color);
                    }
                    // 空格键。
                    let mut space = PathBuilder::stroke(px(1.6));
                    space.move_to(at(0.34, 0.58));
                    space.line_to(at(0.66, 0.58));
                    if let Ok(path) = space.build() {
                        window.paint_path(path, color);
                    }
                }
                DeviceKind::Headset => {
                    // 头梁：两段三次贝塞尔拼出半圆。
                    let mut band = PathBuilder::stroke(px(1.6));
                    band.move_to(at(0.16, 0.62));
                    band.cubic_bezier_to(at(0.50, 0.16), at(0.16, 0.34), at(0.31, 0.16));
                    band.cubic_bezier_to(at(0.84, 0.62), at(0.69, 0.16), at(0.84, 0.34));
                    if let Ok(path) = band.build() {
                        window.paint_path(path, color);
                    }
                    // 两只耳罩。
                    for (left, right) in [(0.10, 0.28), (0.72, 0.90)] {
                        let mut cup = PathBuilder::fill();
                        rounded_rect(
                            &mut cup,
                            &at,
                            &radius,
                            (left, 0.54, right, 0.86),
                            0.05,
                        );
                        if let Ok(path) = cup.build() {
                            window.paint_path(path, color);
                        }
                    }
                }
            }
        },
    )
    .w(px(22.0))
    .h(px(22.0))
}

/// 往构建器里加一个圆角矩形（归一化坐标，四个角用圆弧过渡）。
fn rounded_rect(
    builder: &mut PathBuilder,
    at: &impl Fn(f32, f32) -> Point<Pixels>,
    radius: &impl Fn(f32) -> Point<Pixels>,
    (x0, y0, x1, y1): (f32, f32, f32, f32),
    r: f32,
) {
    builder.move_to(at(x0 + r, y0));
    builder.line_to(at(x1 - r, y0));
    builder.arc_to(radius(r), px(0.0), false, true, at(x1, y0 + r));
    builder.line_to(at(x1, y1 - r));
    builder.arc_to(radius(r), px(0.0), false, true, at(x1 - r, y1));
    builder.line_to(at(x0 + r, y1));
    builder.arc_to(radius(r), px(0.0), false, true, at(x0, y1 - r));
    builder.line_to(at(x0, y0 + r));
    builder.arc_to(radius(r), px(0.0), false, true, at(x0 + r, y0));
    builder.close();
}

/// 一台设备都没读到时的占位行。
pub fn placeholder(palette: &Palette) -> impl IntoElement {
    div()
        .w_full()
        .h(px(ROW_H))
        .flex_none()
        .rounded(px(ROW_RADIUS))
        .bg(rgb(palette.card))
        .border_1()
        .border_color(rgb(palette.card_border))
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT_CN)
        .text_size(px(11.0))
        .text_color(rgb(palette.text_faint))
        .child("正在读取设备…")
}

#[cfg(test)]
mod tests {
    use super::fill_ratio;

    /// 填充比例必须严格随电量走，0% 不能留残影、100% 要铺满。
    #[test]
    fn fill_ratio_tracks_percent() {
        assert_eq!(fill_ratio(0), 0.0);
        assert_eq!(fill_ratio(100), 1.0);
        assert!((fill_ratio(50) - 0.5).abs() < 0.001);
        // 越界输入不能把条撑出容器。
        assert_eq!(fill_ratio(120), 1.0);
    }

    /// 状态到文案的映射是对外的承诺，改了要有人知道。
    #[test]
    fn captions_cover_every_state() {
        use peripheral_battery::Status;
        assert_eq!(
            Status::Online {
                percent: 80,
                charging: Some(true)
            }
            .caption(),
            "正在充电"
        );
        assert_eq!(
            Status::Online {
                percent: 80,
                charging: Some(false)
            }
            .caption(),
            "电池放电中"
        );
        // 协议里没有充电位时不许瞎猜。
        assert_eq!(
            Status::Online {
                percent: 80,
                charging: None
            }
            .caption(),
            "电池状态未知"
        );
        assert_eq!(Status::Asleep.caption(), "设备休眠中");
        assert_eq!(Status::Missing.caption(), "未检测到设备");
    }
}
