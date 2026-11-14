//! 程序入口：开一个无边框窗口，把面板挂上去。

// Windows 子系统下不弹控制台窗口；调试时改用 `cargo run`。
#![windows_subsystem = "windows"]

mod chrome;
mod device;
mod icons;
mod panel;
mod theme;

use gpui_kit::*;
use peripheral_battery::provider;

use crate::theme::APP_NAME;

fn main() {
    set_app_user_model_id();

    // 只允许一个实例：第二个启动的进程只负责把已有面板叫出来，然后退出。
    // 否则每启动一次就多一个托盘图标、多一份 HID 轮询。
    if !claim_single_instance() {
        let mut shown = false;
        for _ in 0..20 {
            if peripheral_battery::tray::activate_existing() {
                shown = true;
                break;
            }
            // 先启动的那个可能还没把托盘窗口建出来，等它一会儿。
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // 静默退出最让人困惑（双击了却没反应）：留一行日志说明是「已有一个在跑」。
        peripheral_battery::diag::log(if shown {
            "已有实例在运行：已唤起它的面板，本进程退出"
        } else {
            "已有实例在运行，但 2 秒内没能唤起它（托盘窗口未就绪），本进程退出"
        });
        return;
    }

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);

        // 启动时就按注册的设备数开窗口，免得第一帧出现后再长高一次。
        let size = panel::window_size(provider::defaults().len());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(initial_bounds(cx, size))),
            titlebar: None,
            is_resizable: false,
            is_minimizable: true,
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            ..Default::default()
        };

        gpui_kit::open_window(options, cx, |window, cx| {
            // 无边框窗口的标题看不见，但任务视图和辅助工具读的是它。
            window.set_window_title(APP_NAME);
            cx.new(|cx| panel::Panel::new(window, cx))
        })
        .expect("打开主窗口失败");

        // 窗口创建之后再补 DWM 圆角：此时 HWND 才存在。
        chrome::apply();
    });
}

/// 用命名互斥体保证只有一个实例。
///
/// 返回 `false` 表示别的实例已经占着这个名字。互斥体句柄故意不关：进程结束时
/// 系统会自动释放，提前关掉反而会让第二个实例误以为没人。
fn claim_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    // `Local\`：只在当前登录会话里唯一，多用户同时登录时互不影响。
    let name: Vec<u16> = "Local\\PeripheralBattery.Panel\0".encode_utf16().collect();
    // SAFETY: name 以 NUL 结尾；返回的句柄由系统在进程退出时回收。
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            // 连互斥体都建不出来（极罕见）：宁可允许启动，别让程序打不开。
            return true;
        }
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// 首次出现的位置：主屏工作区正中。
///
/// 单位：`PlatformDisplay::visible_bounds()` 已经是**逻辑像素**（`gpui-pre-windows`
/// 在 `display.rs` 里做过 `physical / scale_factor`），`WindowBounds` 要的也是逻辑
/// 像素 —— 两边一致，直接算就行。
fn initial_bounds(cx: &App, size: Size<Pixels>) -> Bounds<Pixels> {
    let fallback = Bounds::new(point(px(240.0), px(180.0)), size);
    // `displays()` 的顺序来自 EnumDisplayMonitors，不保证主屏在前。
    let Some(display) = cx
        .primary_display()
        .or_else(|| cx.displays().into_iter().next())
    else {
        return fallback;
    };

    // 居中按工作区算：任务栏占掉的那一条不该把视觉重心带偏。
    let screen = display.visible_bounds();
    Bounds::new(
        point(
            screen.origin.x + (screen.size.width - size.width) / 2.0,
            screen.origin.y + (screen.size.height - size.height) / 2.0,
        ),
        size,
    )
}

/// 给进程一个显式 AppUserModelID。
///
/// 没有它时，任务栏按钮会按 exe 路径归组，`embed-resource` 嵌进去的图标和
/// 任务栏按钮的图标偶尔会对不上；有它之后，任务栏、开始菜单、通知都认同一个
/// 应用身份。
fn set_app_user_model_id() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    let id: Vec<u16> = "PeripheralBattery.Panel\0".encode_utf16().collect();
    // SAFETY: 字符串以 NUL 结尾，调用期间一直有效；失败只是个 HRESULT，忽略即可。
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
}

