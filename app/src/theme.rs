//! 主题：颜色与尺寸的唯一出处。
//!
//! 颜色一律写成 `0xRRGGBB` 的 `u32`，交给 gpui 的 `rgb()` / `rgba()`；
//! 不用 `const fn` 拼 `Rgba`，因为 `const` 上下文里做不了类型转换。
//!
//! 尺寸同理：只在这里定义一次，布局代码不许出现魔法数字。

/// 窗口逻辑宽度（内容宽度 + 左右留白）。按 MCHOSE G87 这种 9 位设备名排出来。
pub const WINDOW_W: f32 = 372.0;

// 高度不写死：早期是常量，只够两行，加第三台设备时会被裁掉半行。
// 现在由 `panel_height(行数)` 算，`Panel::fit_rows` 在行数变化时改窗口尺寸。

/// 窗口上留白。
pub const PAD_Y: f32 = 16.0;
/// 窗口下留白：和卡片间距一致，页脚不会显得被"顶"在中间。
pub const PAD_BOTTOM: f32 = 8.0;
/// 页眉高度（内容高度，不含留白）。
pub const HEADER_H: f32 = 30.0;
/// 设备列表上方的间距。
pub const LIST_TOP: f32 = 12.0;
/// 页脚高度。
pub const FOOTER_H: f32 = 24.0;
/// 相邻设备行之间的间距。
pub const LIST_GAP: f32 = 8.0;
/// 列表和页脚之间的间距，和 `LIST_TOP`、`LIST_GAP` 一致。
pub const FOOTER_GAP: f32 = 8.0;
/// 尺寸余量：描边、行高取整之后会差一两个像素，留一点免得页脚被压。
const HEIGHT_SLACK: f32 = 4.0;

/// 最多显示几行设备；再多就滚动，免得面板高过屏幕。
pub const MAX_ROWS: usize = 5;

/// 给定设备行数算出面板高度（逻辑像素）。
///
/// 行数夹到 `1..=MAX_ROWS`：0 行时列表里是占位行，高度按一行算；超过上限
/// 不再长高，交给列表滚动。
#[must_use]
pub fn panel_height(rows: usize) -> f32 {
    let rows = rows.clamp(1, MAX_ROWS);
    let stacked = rows as f32 * ROW_H + rows.saturating_sub(1) as f32 * LIST_GAP;
    PAD_Y + PAD_BOTTOM + HEADER_H + LIST_TOP + stacked + FOOTER_GAP + FOOTER_H + HEIGHT_SLACK
}

/// 软件名：窗口标题、面板页眉、托盘缺省提示都用它，避免各处写散。
pub const APP_NAME: &str = "外设电量显示";
/// 页眉下面那行小字。
pub const TAGLINE: &str = "2.4G 无线外设";

/// 左右留白。
pub const PAD_X: f32 = 16.0;

/// 设备行高度。
pub const ROW_H: f32 = 76.0;
/// 设备行圆角。
pub const ROW_RADIUS: f32 = 14.0;
/// 设备行内左右内边距。
pub const ROW_PAD_X: f32 = 12.0;
/// 设备行内上下内边距。
pub const ROW_PAD_Y: f32 = 10.0;
/// 左侧设备类型方块的边长。
pub const BADGE: f32 = 38.0;

/// 电量条高度。
pub const BAR_H: f32 = 6.0;
/// 电量条圆角。
pub const BAR_RADIUS: f32 = 3.0;

/// 中文正文：Win10/11 都自带。
pub const FONT_CN: &str = "Microsoft YaHei UI";
/// 数字用 Segoe UI，等宽感更好。
pub const FONT_LATIN: &str = "Segoe UI";

/// 品牌色块渐变的起点（绿）。
pub const LOGO_FROM: u32 = 0x34D399;
/// 品牌色块渐变的终点（青）。
pub const LOGO_TO: u32 = 0x0EA5E9;

/// 一套配色。
///
/// 颜色都是 `u32` 而不是 `Rgba`：`const` 里没法做转换，用的时候再 `rgb()`。
pub struct Palette {
    /// 窗口渐变起点。
    pub bg_top: u32,
    /// 窗口渐变终点。
    pub bg_bottom: u32,
    /// 卡片底色。
    pub card: u32,
    /// 卡片描边。
    pub card_border: u32,
    /// 电量条空槽。
    pub track: u32,

    /// 主文字。
    pub text: u32,
    /// 次要文字（副标题、单位）。
    pub text_muted: u32,
    /// 最弱的文字（"更新 12:03""设备休眠中"）。
    pub text_faint: u32,

    /// 悬停底色。
    pub hover: u32,
    /// 未选中间隔胶囊。
    pub chip_bg: u32,
    /// 选中间隔胶囊。
    pub chip_active: u32,

    /// 高电量色。
    pub high: u32,
    /// 中电量色。
    pub mid: u32,
    /// 低电量色。
    pub low: u32,
}

/// 暗色：对着 Windows 11 深色模式调的。
pub const DARK: Palette = Palette {
    bg_top: 0x1A1D24,
    bg_bottom: 0x101217,
    card: 0x22262F,
    card_border: 0x2E3440,
    track: 0x333A47,

    text: 0xF2F5F9,
    text_muted: 0x9AA4B2,
    text_faint: 0x6B7482,

    hover: 0xFFFFFF14,
    chip_bg: 0x1E222A,
    chip_active: 0x323A47,

    high: 0x4ADE80,
    mid: 0xFBBF24,
    low: 0xF87171,
};

/// 亮色：白卡片 + 冷灰底。
pub const LIGHT: Palette = Palette {
    bg_top: 0xFFFFFF,
    bg_bottom: 0xF3F4F7,
    card: 0xFFFFFF,
    card_border: 0xE4E7EC,
    track: 0xE9ECF1,

    text: 0x1B1F26,
    text_muted: 0x6A7280,
    text_faint: 0x9AA1AC,

    hover: 0x0000000F,
    chip_bg: 0xF0F2F5,
    chip_active: 0xFFFFFF,

    high: 0x16A34A,
    mid: 0xD97706,
    low: 0xDC2626,
};

/// 按明暗取配色。
#[must_use]
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// 按电量取颜色：≥50% 绿 / ≥20% 橙 / 其他红。
///
/// 阈值和托盘图标的低电判定（`LOW_THRESHOLD = 20`）保持一致。
#[must_use]
pub fn level_hex(palette: &Palette, percent: u8) -> u32 {
    if percent >= 50 {
        palette.high
    } else if percent >= 20 {
        palette.mid
    } else {
        palette.low
    }
}

/// 把 alpha 拼到 `0xRRGGBB` 后面，得到 `0xRRGGBBAA`。
#[must_use]
pub const fn alpha(hex: u32, alpha: u8) -> u32 {
    (hex << 8) | alpha as u32
}

#[cfg(test)]
mod tests {
    use super::{DARK, LIGHT, alpha, level_hex};

    /// 50% / 20% 两个分界都落在高的一侧。
    #[test]
    fn level_thresholds() {
        assert_eq!(level_hex(&DARK, 100), DARK.high);
        assert_eq!(level_hex(&DARK, 50), DARK.high);
        assert_eq!(level_hex(&DARK, 49), DARK.mid);
        assert_eq!(level_hex(&DARK, 20), DARK.mid);
        assert_eq!(level_hex(&DARK, 19), DARK.low);
        assert_eq!(level_hex(&LIGHT, 0), LIGHT.low);
    }

    /// 拼 alpha 不动 RGB。
    #[test]
    fn alpha_keeps_rgb() {
        assert_eq!(alpha(0x112233, 0x80), 0x1122_3380);
        assert_eq!(alpha(0xFFFFFF, 0xFF), 0xFFFF_FFFF);
    }

    /// 两套主题确实不一样。
    #[test]
    fn themes_are_distinct() {
        assert_ne!(DARK.bg_top, LIGHT.bg_top);
        assert_ne!(DARK.text, LIGHT.text);
    }

    /// 每多一行，高度就多一个 "行高 + 行间距"。
    #[test]
    fn height_grows_with_rows() {
        let step = super::ROW_H + super::LIST_GAP;
        let two = super::panel_height(2);
        let three = super::panel_height(3);
        assert!((three - two - step).abs() < 0.01);
    }

    /// 0 行按一行算，超过上限不再长高。
    #[test]
    fn height_is_clamped() {
        assert_eq!(super::panel_height(0), super::panel_height(1));
        assert_eq!(
            super::panel_height(super::MAX_ROWS + 3),
            super::panel_height(super::MAX_ROWS)
        );
    }
}
