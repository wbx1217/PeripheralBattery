//! 主面板：页眉、设备行、刷新间隔与托盘联动。
//!
//! 三层状态各自独立：
//! - 数据：`Monitor` 的后台线程按设定间隔采一次，UI 只比对版本号；
//! - 主题：亮 / 暗两态，落盘在注册表；
//! - 显隐：托盘与失焦驱动，走 `chrome` 的 `ShowWindow`。
//!
//! 窗口尺寸也跟着数据走：设备行数变了就按 `theme::panel_height` 重算高度，
//! 右下角钉在原处（见 `Panel::fit_rows`）。

use std::time::{Duration, Instant};

use gpui_kit::*;
use peripheral_battery::prefs::{self, ThemeMode};
use peripheral_battery::service::INTERVAL_CHOICES;
use peripheral_battery::{Monitor, Snapshot, autostart, provider, tray::Tray, tray::TrayEvent};

use crate::chrome;
use crate::device::{placeholder, row};
use crate::icons;
use crate::theme::{
    APP_NAME, FONT_CN, FOOTER_GAP, FOOTER_H, HEADER_H, LIST_GAP, LIST_TOP, LOGO_FROM, LOGO_TO, MAX_ROWS,
    PAD_BOTTOM, PAD_X, PAD_Y, TAGLINE, WINDOW_W, palette, panel_height,
};

/// 刷新按钮转一圈的时长。
const SPIN: Duration = Duration::from_millis(650);

/// 界面自身的定时器周期：检查托盘事件、快照版本、系统主题。
const UI_TICK: Duration = Duration::from_millis(150);

/// 托盘提示最多列几台设备，多出来的折成一行「…另有 N 台」。
///
/// 上限 6：`NOTIFYICONDATAW::szTip` 只有 128 个 UTF-16 单元，六行「名字 + 百分比」
/// 正好塞得下，再多就会被硬截断。
const TOOLTIP_ROWS: usize = 6;

/// 主面板视图。
pub struct Panel {
    monitor: Monitor,
    tray: Option<Tray>,
    snapshot: Snapshot,
    /// 用户选的刷新间隔（托盘菜单与底部胶囊）。面板收起时实际用的是
    /// 活动档（面板开着时每 10 秒一次），但用户选的间隔要留着，收起时按它走。
    preferred: u64,
    enabled_at_startup: bool,
    window: AnyWindowHandle,

    /// 上一次同步到托盘的百分比，避免每次 tick 都重建图标。
    tray_percent: Option<u8>,
    /// 上一次同步到托盘的提示文字。
    ///
    /// 图标现在是固定的，但提示要跟着每台设备的电量变 —— 只比百分比不够，
    /// 第二台设备电量变了而「最该关注的那台」没变时，提示就会停在旧值上。
    tray_tooltip: String,

    /// 主题偏好。
    mode: ThemeMode,
    /// 刷新按钮的旋转动效：从哪一刻开始转。
    ///
    /// 没用 gpui 的 `with_animation`：它听系统的"动画效果"开关，这台机器上
    /// 那个开关是关的，动画会被静态渲染。这里自己按帧推进，用户点了就看得见。
    spin_start: Option<Instant>,
    /// 旋转动效的代号：每点一次自增。
    ///
    /// 连点两次时，前一次那个收尾循环不能把后一次的旋转提前掐掉 —— 它只允许
    /// 清理自己那一代。
    spin_token: u64,

    /// 这一帧用哪套配色。
    dark: bool,
    /// 上一次按几行设备摆的窗口。
    ///
    /// 行数一变就要重算窗口高度；有它就只在不一致时动窗口，不会每 tick 都调。
    rows: usize,
    /// 是否已经把主题推给过 DWM。
    ///
    /// 窗口创建时 `Panel::new` 可能还拿不到 HWND，第一次同步放到 tick 里做。
    theme_applied: bool,

}

impl Panel {
    /// 建立视图：起轮询线程、挂托盘、启动界面定时器。
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let monitor = Monitor::start(provider::defaults());
        monitor.refresh_now();

        let tray = match Tray::spawn() {
            Ok(tray) => Some(tray),
            Err(error) => {
                // 没有托盘就没有「收进托盘」这个入口：此时点 ✕ 必须真的能退出，
                // 否则窗口一藏，用户既看不见也关不掉（只剩任务管理器）。
                peripheral_battery::diag::log(&format!("托盘启动失败，按普通窗口运行：{error}"));
                None
            }
        };
        // 改过名的话，先把旧的自启项迁过来，再读状态。
        autostart::migrate_legacy();
        let enabled_at_startup = autostart::is_enabled();

        let mode = prefs::theme_mode();
        let dark = mode.is_dark();

        // 间隔从偏好读回来；存的值不在候选里（改过候选列表）就退回默认档。
        let preferred = prefs::interval_ms()
            .filter(|saved| {
                INTERVAL_CHOICES
                    .iter()
                    .any(|(value, _)| value == saved)
            })
            .unwrap_or(peripheral_battery::service::DEFAULT_INTERVAL_MS);
        monitor.set_interval(preferred);

        let panel = Self {
            monitor,
            tray,
            snapshot: Snapshot::default(),
            preferred,
            enabled_at_startup,
            window: window.window_handle(),
            tray_percent: None,
            tray_tooltip: String::new(),
            spin_start: None,
            spin_token: 0,
            mode,
            dark,
            // 启动窗口按「注册了几台设备」开的，第一次快照就是同一个数，
            // 写在这里免得开局多摆一次窗口。
            rows: provider::defaults().len(),
            theme_applied: false,
        };

        // Alt+F4 / 任务栏右键「关闭」都走这里：没有托盘就直接退，
        // 有托盘则收进去（返回 false 告诉系统「别关闭」，窗口留给托盘唤回）。
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let has_tray = weak
                .update(cx, |panel, cx| {
                    let has_tray = panel.tray.is_some();
                    if has_tray {
                        panel.hide();
                        cx.notify();
                    }
                    has_tray
                })
                .unwrap_or(false);
            if !has_tray {
                // 没有托盘可回：允许真正关闭窗口。
                cx.quit();
            }
            !has_tray
        });

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(UI_TICK).await;
                if this.update(cx, |panel, cx| panel.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        panel
    }

    /// 每个 tick：处理托盘事件、拉取新快照、同步托盘图标与主题。
    fn tick(&mut self, cx: &mut Context<Self>) {
        // 在标题栏上拖到窗口外松手时，窗口收不到 on_mouse_up —— 不看一眼左键状态，
        // 之后鼠标随便动一下窗口都会跟着跑。
        if crate::chrome::dragging() && !crate::chrome::left_button_down() {
            crate::chrome::end_drag();
        }
        let events = self.tray.as_ref().map(Tray::drain).unwrap_or_default();
        for event in events {
            match event {
                TrayEvent::ToggleWindow => {
                    // 问系统而不是问自己的字段：窗口可能被任务栏最小化/其它途径藏掉，
                    // 字段一旦漂移，用户就会遇到「点一次没反应、点两次才出来」。
                    if chrome::is_visible() {
                        self.hide();
                    } else {
                        self.show_window(cx);
                    }
                }
                TrayEvent::ShowWindow => self.show_window(cx),
                TrayEvent::SetInterval(interval) => {
                    self.preferred = interval;
                    // 记住选择：下次启动直接按这个节奏跑。
                    prefs::set_interval_ms(interval);
                    self.monitor.set_interval(interval);
                    self.sync_menu();
                    cx.notify();
                }
                TrayEvent::ToggleAutostart => {
                    let want = !self.enabled_at_startup;
                    if let Err(code) = autostart::set_enabled(want) {
                        // 写不进去就别装作成功了：勾选状态按回读值走，并告诉用户原因。
                        peripheral_battery::diag::log(&format!(
                            "开机自启写入失败：目标={want}，错误码={code}"
                        ));
                        // 弹框自带消息循环，跑在 UI 线程会把面板冻住；挪到后台线程去弹。
                        let title = "无法修改开机自启";
                        let body = format!(
                            "写入开机自启失败（错误码 {code}）。\n\n错误码 5 = 拒绝访问，常见原因：\n① 安全软件拦了「修改启动项」；\n② 程序运行在受限环境（从沙箱、调试器或某些工程目录启动时会被降权）—— 把 exe 复制到普通目录再运行。"
                        );
                        cx.background_executor()
                            .spawn(async move { crate::chrome::alert(title, &body) })
                            .detach();
                    }
                    // 以注册表实际值为准：写失败时勾选状态不能骗人。
                    self.enabled_at_startup = autostart::is_enabled();
                    peripheral_battery::diag::log(&format!(
                        "开机自启：目标={want}，回读={}",
                        self.enabled_at_startup
                    ));
                    self.sync_menu();
                }
                TrayEvent::Quit => cx.quit(),
            }
        }

        if !self.theme_applied {
            // HWND 现在肯定有了，把启动时那次没能落地的 DWM 深色边框补上。
            self.theme_applied = true;
            chrome::set_dark(self.dark);
        }

        let fresh = self.monitor.snapshot();
        if fresh.is_newer_than(&self.snapshot) {
            self.snapshot = fresh;
            self.sync_tray();
            self.fit_rows();
            cx.notify();
        }
    }

    /// 把当前电量推给托盘图标与提示。
    fn sync_tray(&mut self) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        // 取「最该关注」的那台：正在充电的不算，全离线时画通用图标。
        let level = self.snapshot.tray_percent();
        let tooltip = self.tooltip();
        // 图标只按「是否低电」换色，所以百分比不变就不必重画；但提示文字得逐字比对。
        if level != self.tray_percent || tooltip != self.tray_tooltip {
            self.tray_percent = level;
            self.tray_tooltip = tooltip.clone();
            tray.set_status(&tooltip, level);
        }
        self.sync_menu();
    }

    fn sync_menu(&self) {
        if let Some(tray) = self.tray.as_ref() {
            tray.set_menu_state(self.enabled_at_startup, self.preferred);
        }
    }

    /// 托盘悬浮提示：一台设备一行。
    ///
    /// 图标固定成品牌标之后，电量就只能从提示里读 —— 一行一台，比用「·」串成
    /// 一长条好扫。超过 [`TOOLTIP_ROWS`] 台的折成最后一行。
    fn tooltip(&self) -> String {
        let rows: Vec<(String, Option<u8>)> = self
            .snapshot
            .devices
            .iter()
            .map(|device| (device.name.clone(), device.status.percent()))
            .collect();
        tooltip_text(&rows)
    }

    /// 从托盘唤出面板。
    fn show_window(&mut self, cx: &mut Context<Self>) {
        // 开合只影响立即刷新，不改轮询节奏：节奏永远听用户的选择。
        self.monitor.refresh_now();
        // 每次唤出都回到工作区正中：面板是临时看的，不指望用户记得上次拖到哪。
        // 摆好位置再显示，免得先露一下再跳。
        chrome::center();
        chrome::show();
        // gpui 不一定知道窗口刚被显示出来，主动重绘一次免得露出空窗。
        let _ = self
            .window
            .update(cx, |_, window, _| window.activate_window());
        cx.notify();
    }

    /// 收起面板。刷新节奏不受影响 —— 那是用户在菜单/胶囊里选的。
    fn hide(&mut self) {
        chrome::hide();
    }

    /// 设备行数变了就重排窗口高度，右下角钉住。
    ///
    /// 只认行数：电量在变、时间戳在变，但布局一个像素都不用动。
    fn fit_rows(&mut self) {
        let rows = self.snapshot.devices.len();
        if rows == self.rows {
            return;
        }
        self.rows = rows;
        chrome::resize_anchored(WINDOW_W, panel_height(rows));
    }

    /// 按当前偏好重算配色，并同步 DWM 的深色边框。
    fn apply_theme(&mut self) {
        self.dark = self.mode.is_dark();
        chrome::set_dark(self.dark);
    }
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(self.dark);
        // 不 clone 整份设备列表：每帧都复制一遍只是把分配器当暖手宝。
        let rows: Vec<_> = self
            .snapshot
            .devices
            .iter()
            .map(|device| row(device, palette, self.dark))
            .collect();
        let mut list = div()
            .id("device-list")
            .flex_1()
            // 不写 min_h 的话 flex 子项会被内容撑开，滚动条永远不会出现。
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(LIST_GAP))
            .pt(px(LIST_TOP));
        // 行数封顶后窗口不再长高，多出来的行走滚动 —— 页脚不会被挤出画面。
        if self.snapshot.devices.len() > MAX_ROWS {
            list = list.overflow_y_scroll();
        }
        // 空列表时给一行占位，免得窗口开出来是一片空白。
        if rows.is_empty() {
            list = list.child(placeholder(palette));
        }

        div()
            .relative()
            .size_full()
            .font_family(FONT_CN)
            .bg(linear_gradient(
                160.0,
                linear_color_stop(rgb(palette.bg_top), 0.0),
                linear_color_stop(rgb(palette.bg_bottom), 1.0),
            ))
            .flex()
            .flex_col()
            .px(px(PAD_X))
            .pt(px(PAD_Y))
            .pb(px(PAD_BOTTOM))
            .child(self.header(cx))
            .child(list.children(rows))
            .child(self.footer(cx))
    }
}

impl Panel {
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(self.dark);
        div()
            .flex()
            .items_center()
            .h(px(HEADER_H))
            .gap(px(10.0))
            .child(logo())
            .child(
                // 只有这块区域拖动窗口，按钮区不参与，否则点按钮会误拖。
                div()
                    .id("drag-region")
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(1.0))
                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                        crate::chrome::begin_drag();
                    })
                    .on_mouse_move(|event, _, _| {
                        if event.dragging() {
                            crate::chrome::drag_to();
                        }
                    })
                    .on_mouse_up(MouseButton::Left, |_, _, _| crate::chrome::end_drag())
                    .child(
                        div()
                            .font_family(FONT_CN)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(14.0))
                            .line_height(px(18.0))
                            .text_color(rgb(palette.text))
                            .child(APP_NAME),
                    )
                    .child(
                        div()
                            .font_family(FONT_CN)
                            .text_size(px(10.0))
                            .line_height(px(12.0))
                            .text_color(rgb(palette.text_faint))
                            .child(TAGLINE),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(self.vector_button(
                        "refresh",
                        refresh_icon(self.spin_start, palette),
                        palette,
                        cx.listener(|this, _, _, cx| {
                            this.monitor.refresh_now();
                            // 转一圈表示"收到了"。自己按帧推进，不依赖系统动画开关。
                            this.spin_start = Some(Instant::now());
                            this.spin_token = this.spin_token.wrapping_add(1);
                            let token = this.spin_token;
                            cx.notify();
                            cx.spawn(async move |this, cx| {
                                while this
                                    .update(cx, |this, _| {
                                        this.spin_token == token
                                            && this
                                                .spin_start
                                                .is_some_and(|start| start.elapsed() < SPIN)
                                    })
                                    .unwrap_or(false)
                                {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(16))
                                        .await;
                                    this.update(cx, |_, cx| cx.notify()).ok();
                                }
                                this.update(cx, |this, cx| {
                                    // 只有自己那一代还在转，才轮到它收尾。
                                    if this.spin_token == token {
                                        this.spin_start = None;
                                        cx.notify();
                                    }
                                })
                                .ok();
                            })
                            .detach();
                        }),
                    ))
                    .child(self.vector_button(
                        "theme",
                        // 画的是「点下去会变成什么」：暗色时显示太阳（切回亮色）。
                        if self.mode.is_dark() {
                            icons::sun(palette.text_muted).into_any_element()
                        } else {
                            icons::moon(palette.text_muted).into_any_element()
                        },
                        palette,
                        cx.listener(|this, _, _, cx| {
                            this.mode = this.mode.next();
                            prefs::set_theme_mode(this.mode);
                            this.apply_theme();
                            cx.notify();
                        }),
                    ))
                    .child(self.vector_button(
                        "close",
                        icons::close(palette.text_muted),
                        palette,
                        cx.listener(|this, _, _, cx| {
                            this.hide();
                            cx.notify();
                        }),
                    )),
            )
    }

    /// 页眉右侧的图标按钮：28px 热区 + 20px 矢量图标。
    ///
    /// 以前用字体字形，字形自带的边距让图标看着偏上偏左；换成 `PathBuilder`
    /// 之后热区和图标都以几何中心对齐，问题从根上没了。
    fn vector_button(
        &self,
        id: &'static str,
        icon: impl IntoElement + 'static,
        palette: &'static crate::theme::Palette,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .size(px(28.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(rgba(palette.hover)))
            .on_click(on_click)
            .child(icon)
    }

    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(self.dark);
        let chips = INTERVAL_CHOICES
            .iter()
            .map(|(value, label)| {
                let value = *value;
                let label = *label;
                let selected = value == self.preferred;
                div()
                    .id(("chip", value as usize))
                    .px(px(9.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .bg(rgb(if selected {
                        palette.chip_active
                    } else {
                        palette.chip_bg
                    }))
                    .hover(|style| style.bg(rgba(palette.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.preferred = value;
                        this.monitor.set_interval(value);
                        this.monitor.refresh_now();
                        this.sync_menu();
                        cx.notify();
                    }))
                    .child(
                        div()
                            .font_family(FONT_CN)
                            .text_size(px(10.0))
                            .text_color(rgb(if selected {
                                palette.text
                            } else {
                                palette.text_faint
                            }))
                            .child(label),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .items_center()
            .h(px(FOOTER_H))
            .mt(px(FOOTER_GAP))
            .gap(px(2.0))
            .children(chips)
            .child(div().flex_1())
            .child(
                div()
                    .font_family(FONT_CN)
                    .text_size(px(9.5))
                    .text_color(rgb(palette.text_faint))
                    .child(self.summary()),
            )
    }

    /// 底部一行状态摘要。
    fn summary(&self) -> String {
        if self.snapshot.devices.is_empty() {
            return "正在读取…".to_owned();
        }
        let online = self.snapshot.online_count();
        let total = self.snapshot.devices.len();
        let latest = self
            .snapshot
            .devices
            .iter()
            .filter_map(|d| d.updated_at.as_deref())
            .max()
            .map(str::to_owned);
        match latest {
            Some(time) => format!("{online}/{total} 在线 · 更新于 {time}"),
            None => format!("{online}/{total} 在线"),
        }
    }
}

/// 刷新图标：转的时候按已过时间旋转，静止时就是普通 SVG。
fn refresh_icon(spin_start: Option<Instant>, palette: &'static crate::theme::Palette) -> AnyElement {
    match spin_start {
        Some(start) => {
            let progress = (start.elapsed().as_secs_f32() / SPIN.as_secs_f32()).min(1.0);
            icons::refresh_at(progress * std::f32::consts::TAU, palette.text_muted)
                .into_any_element()
        }
        None => icons::refresh(palette.text_muted).into_any_element(),
    }
}

/// 页眉左侧的标志：渐变圆角方块 + 白色闪电。
///
/// 闪电和托盘/exe 图标共用同一份 `assets/icons/bolt.svg`，占方块边长的
/// `icons::BOLT_RATIO`（62%）—— 三处比例一致，缩放时不会"越小的图标闪电越胖"。
fn logo() -> impl IntoElement {
    div()
        .size(px(30.0))
        .flex_none()
        .rounded(px(9.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(
            135.0,
            linear_color_stop(rgb(LOGO_FROM), 0.0),
            linear_color_stop(rgb(LOGO_TO), 1.0),
        ))
        .child(icons::bolt(30.0, 0xFFFFFF))
}

/// 给定设备行数的窗口初始尺寸。
///
/// 只在开机那一刻用一次；之后行数变化由 [`Panel::fit_rows`] 接手。
#[must_use]
pub fn window_size(rows: usize) -> Size<Pixels> {
    size(px(WINDOW_W), px(panel_height(rows)))
}

/// 拼托盘提示：一台一行，最多 [`TOOLTIP_ROWS`] 行，多余折成「…另有 N 台」。
///
/// 单独拎出来是为了能直接测字符串 —— `szTip` 只有 128 个 UTF-16 单元，
/// 超过 6 台就得靠这行折叠兜底。
fn tooltip_text(rows: &[(String, Option<u8>)]) -> String {
    if rows.is_empty() {
        return APP_NAME.to_owned();
    }
    let mut lines: Vec<String> = rows
        .iter()
        .take(TOOLTIP_ROWS)
        .map(|(name, percent)| match percent {
            Some(percent) => format!("{name} {percent}%"),
            None => format!("{name} 未连接"),
        })
        .collect();
    let hidden = rows.len().saturating_sub(TOOLTIP_ROWS);
    if hidden > 0 {
        lines.push(format!("…另有 {hidden} 台"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{tooltip_text, TOOLTIP_ROWS};

    fn rows(count: usize) -> Vec<(String, Option<u8>)> {
        (0..count)
            .map(|index| (format!("设备{index}"), Some(50)))
            .collect()
    }

    #[test]
    fn one_device_per_line() {
        let text = tooltip_text(&[
            ("键盘".to_owned(), Some(99)),
            ("耳机".to_owned(), None),
        ]);
        assert_eq!(text, "键盘 99%\n耳机 未连接");
    }

    #[test]
    fn no_device_falls_back_to_app_name() {
        assert_eq!(tooltip_text(&[]), super::APP_NAME);
    }

    #[test]
    fn extra_devices_fold_into_last_line() {
        let text = tooltip_text(&rows(TOOLTIP_ROWS + 2));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), TOOLTIP_ROWS + 1);
        assert_eq!(lines.last().copied(), Some("…另有 2 台"));
    }

    #[test]
    fn exactly_the_limit_does_not_fold() {
        let text = tooltip_text(&rows(TOOLTIP_ROWS));
        assert_eq!(text.lines().count(), TOOLTIP_ROWS);
        assert!(!text.contains("另有"));
    }
}
