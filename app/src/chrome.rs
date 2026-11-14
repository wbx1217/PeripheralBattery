//! 窗口外观与显隐：给无边框窗口补上 Windows 11 的圆角、深色边框，
//! 并提供真正意义上的「隐藏 / 唤出」。
//!
//! GPUI 没有暴露底层 HWND，所以这里按「本进程 + 可见 + 无标题」三个条件把窗口
//! 找出来存下 —— 托盘窗口在同一个进程里，但它不可见且有标题，不会误伤。
//!
//! 为什么要自己收 HWND：gpui 的 `Window::hide()` 在 Windows 后端是个空实现
//! （`fn hide(&self) {}`），`minimize_window()` 才是唯一能干的事。而「收进托盘」
//! 要求窗口真的从任务栏上消失、还能被唤回来，所以显隐必须走 `ShowWindow`。

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::core::BOOL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorPos, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MB_ICONWARNING, MB_OK,
    MB_SETFOREGROUND, MessageBoxW, SW_HIDE, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_NOSIZE, SetForegroundWindow, SetWindowPos, ShowWindow,
};

/// 弹一个系统提示框。
///
/// 只在「用户点了开关但系统不让改」这种必须让他知道的场合用；不要拿它当调试输出。
pub fn alert(title: &str, body: &str) {
    let title = wide(title);
    let body = wide(body);
    let owner = ensure().unwrap_or(std::ptr::null_mut());
    // SAFETY: 两个字符串都以 NUL 结尾，owner 可能是空句柄（允许）。
    unsafe {
        MessageBoxW(
            owner,
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND,
        )
    };
}

/// UTF-16 NUL 结尾字符串。
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `DWMWCP_ROUND`
const DWMWCP_ROUND: u32 = 2;
/// `DWMWA_USE_IMMERSIVE_DARK_MODE`（Win10 1809 起为 20）
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
/// `DWMWA_WINDOW_CORNER_PREFERENCE`（Win11 起为 33）
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;

/// 主窗口句柄。0 表示还没找到。
///
/// 只存一次：窗口隐藏之后 `IsWindowVisible` 就是假了，再枚举也找不到。
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// 给主窗口套用 DWM 圆角与深色边框。窗口一创建就调用。
pub fn apply() {
    if let Some(hwnd) = ensure() {
        style(hwnd);
    }
}

/// 取主窗口句柄；还没找到就现场枚举一次。
///
/// 启动时 `apply` 有可能赶在窗口真正可见之前跑，那时枚举不到。所以每次用到
/// 都走这里，找到一次就记住。
pub fn ensure() -> Option<HWND> {
    let stored = MAIN_WINDOW.load(Ordering::Relaxed);
    if stored != 0 {
        return Some(stored as HWND);
    }
    let hwnd = find_own_window()?;
    MAIN_WINDOW.store(hwnd as isize, Ordering::Relaxed);
    Some(hwnd)
}

/// 隐藏窗口（收进托盘）。
///
/// 不用 `SW_MINIMIZE`：最小化会在任务栏留下一个缩略图，而「收进托盘」的语义是
/// 从任务栏上彻底消失，只剩托盘图标那一个入口。
pub fn hide() {
    if let Some(hwnd) = ensure() {
        // SAFETY: hwnd 是本进程的顶层窗口。
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

/// 唤出窗口并抢到前台。
pub fn show() {
    if let Some(hwnd) = ensure() {
        // SAFETY: hwnd 有效；SW_SHOWNORMAL 会把隐藏或最小化的窗口恢复到正常状态。
        unsafe {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);
        }
    }
}

/// 拖动窗口：按下时记住「光标 + 窗口位置」，移动时按差值挪，松手清空。
///
/// 为什么不用 `window.start_window_move()`：`gpui-pre` 的 `PlatformWindow` trait
/// 给它的默认实现就是空函数体，Windows 后端（`gpui-pre-windows`）压根没覆盖 —— 
/// 调它什么也不会发生。
///
/// 为什么不走 `WM_NCLBUTTONDOWN` + `HTCAPTION`：gpui 的窗口过程会**拦截**
/// `WM_NCLBUTTONDOWN`（`events.rs` 里把它转成自己的鼠标按下事件），
/// `DefWindowProc` 根本收不到，系统的模态拖动循环也就起不来。
///
/// 所以只能自己算：`SetWindowPos` 跟着光标走，物理像素，和 DPI 无关。
static DRAG: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);

pub fn begin_drag() {
    let Some(hwnd) = ensure() else {
        return;
    };

    let mut point = POINT::default();
    let mut rect = RECT::default();
    // SAFETY: 两个出参都是本栈上的有效地址；hwnd 是本进程的顶层窗口。
    unsafe {
        if GetCursorPos(&mut point) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
            return;
        }
    }
    if let Ok(mut drag) = DRAG.lock() {
        *drag = Some((point.x, point.y, rect.left, rect.top));
    }
}

pub fn drag_to() {
    let Some(hwnd) = ensure() else {
        return;
    };
    let Some((start_x, start_y, win_x, win_y)) = DRAG.lock().ok().and_then(|d| *d) else {
        return;
    };

    let mut point = POINT::default();
    // SAFETY: point 是栈上有效出参。
    unsafe {
        if GetCursorPos(&mut point) == 0 {
            return;
        }
    }

    let x = win_x + (point.x - start_x);
    let y = win_y + (point.y - start_y);
    // SAFETY: hwnd 是本进程顶层窗口；只挪位置，尺寸与 Z 序都不动。
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 当前是否处于「按住标题栏拖动」状态。
#[must_use]
pub fn dragging() -> bool {
    DRAG.lock().map(|slot| slot.is_some()).unwrap_or(false)
}

/// 鼠标左键是否还按着（`GetAsyncKeyState` 的最高位）。
#[must_use]
pub fn left_button_down() -> bool {
    // SAFETY: 纯查询，不改任何状态。
    (unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON)) } as u16 & 0x8000) != 0
}

pub fn end_drag() {
    if let Ok(mut drag) = DRAG.lock() {
        *drag = None;
    }
}

/// 把窗口摆到所在显示器工作区的正中（尺寸不变）。
///
/// 每次从托盘唤出都调一次：面板是临时看的，用户不会记得上次拖到哪，
/// 回到中间最省事。窗口还藏着时调用不会闪 —— 摆好再 `show()`。
pub fn center() {
    let Some(hwnd) = ensure() else {
        return;
    };

    let mut rect = RECT::default();
    // SAFETY: rect 是本栈上的有效出参，hwnd 是本进程的顶层窗口。
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: 纯查询，hwnd 有效；MONITORINFO 的 cbSize 已填好。
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return;
    }
    let work = info.rcWork;
    let x = work.left + (work.right - work.left - width) / 2;
    let y = work.top + (work.bottom - work.top - height) / 2;

    // SAFETY: hwnd 有效，宽度/高度取自 GetWindowRect，单位都是物理像素。
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 调整窗口内容尺寸（逻辑像素），并把中心钉在原处。
///
/// 只做两件事：算新尺寸、摆回去。窗口不可缩放，尺寸完全由设备行数决定，
/// 所以每次变化都得显式调过来。
///
/// 为什么不用 `Window::resize`：
/// 1. 它以左上角为锚点，面板一长高就往屏幕下沿外长；
/// 2. 它把 `SetWindowPos` 扔进 executor 异步执行，和随后的移位没有先后保证。
///
/// 直接 `SetWindowPos` 一次搞定位置和尺寸；gpui 会从 `WM_SIZE` 里读到新的
/// 逻辑尺寸并重排，不需要额外通知。
///
/// 钉中心而不是右下角：面板每次唤出都居中，长高时上下对称地长才不跳。
pub fn resize_anchored(width: f32, height: f32) {
    let Some(hwnd) = ensure() else {
        return;
    };

    let mut rect = RECT::default();
    // SAFETY: rect 是本栈上的有效出参，hwnd 是本进程的顶层窗口。
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return;
    }

    // SAFETY: 纯查询，hwnd 有效；失败返回 0，下面按 96 DPI 兜底。
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
    let width = (width * scale).round() as i32;
    let height = (height * scale).round() as i32;

    let center_x = (rect.left + rect.right) / 2;
    let center_y = (rect.top + rect.bottom) / 2;
    let mut x = center_x - width / 2;
    let mut y = center_y - height / 2;

    // 夹进工作区：行数多到面板高过屏幕时，至少保证整块面板还在视野里。
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: 纯查询，hwnd 有效；MONITORINFO 的 cbSize 已填好。
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        let work = info.rcWork;
        x = x.clamp(work.left, (work.right - width).max(work.left));
        y = y.clamp(work.top, (work.bottom - height).max(work.top));
    }

    // SAFETY: hwnd 有效；尺寸已按 DPI 换算成物理像素。
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 切换 DWM 的深色边框，并把窗口样式重新套一遍。
pub fn set_dark(dark: bool) {
    let Some(hwnd) = ensure() else {
        return;
    };
    let enabled: u32 = u32::from(dark);
    // SAFETY: hwnd 有效；属性值都是指向 u32 的有效指针，长度为 4。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&enabled).cast::<c_void>(),
            4,
        );
        // 圆角跟着一起设：窗口重建后不必再单跑一次 style()。
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&DWMWCP_ROUND).cast::<c_void>(),
            4,
        );
    }
}

/// 面板当前是否真的可见。
///
/// 不要用界面里的布尔字段：用户从任务栏最小化、或者别处把窗口藏了，那个字段
/// 就会和现实脱节，表现为「点托盘一次没反应、点两次才出来」。
#[must_use]
pub fn is_visible() -> bool {
    let Some(hwnd) = ensure() else {
        return false;
    };
    // SAFETY: hwnd 来自本进程的窗口枚举。
    unsafe { IsWindowVisible(hwnd) != 0 }
}

/// 圆角 + 深色边框。
///
/// 以前这里还加 `WS_EX_TOOLWINDOW`、清 `WS_EX_APPWINDOW`，把窗口藏出任务栏；
/// 现在它是普通窗口，任务栏必须有入口 —— 那两行删掉即可，`gpui-pre-windows`
/// 给 `WindowKind::Normal` 本来就会加 `WS_EX_APPWINDOW`。
fn style(hwnd: HWND) {
    // 只设圆角。深色边框交给 `set_dark`：这里强写 1 会让每次启动先闪一下
    // 暗色边框，亮色主题的用户能看到那一下跳变。
    // SAFETY: hwnd 是本进程的顶层窗口；属性值是指向 u32 的有效指针，长度为 4。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&DWMWCP_ROUND).cast::<c_void>(),
            4,
        );
    }
}

/// 本进程里不能当作主窗口的窗口类：托盘那个隐藏的收消息窗口。
const TRAY_CLASS: &str = "PeripheralBatteryTrayWnd";

/// 找本进程的主窗口。
///
/// 判据是「可见 + 属于本进程 + 不是托盘窗口 + 标题是软件名」；万一标题没取到，
/// 退回到「面积最大」。早先只按面积挑，本进程将来多开一个可见窗口就会认错。
fn find_own_window() -> Option<HWND> {
    let mut found: Vec<(HWND, i64)> = Vec::new();
    let ptr = std::ptr::from_mut(&mut found).cast::<c_void>();

    // SAFETY: 回调只在这个调用期间被系统调用，ptr 指向的 Vec 一直有效。
    unsafe { EnumWindows(Some(collect), ptr as LPARAM) };

    found
        .into_iter()
        .max_by_key(|(_, area)| *area)
        .map(|(hwnd, _)| hwnd)
}

/// 枚举回调：挑出属于本进程、可见、且不是托盘窗口的顶层窗口。
unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam 由 `find_own_window` 传入，指向一个有效的 Vec。
    let list = unsafe { &mut *(lparam as *mut Vec<(HWND, i64)>) };

    // SAFETY: 纯查询调用，句柄来自系统。
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }

    let mut pid = 0u32;
    // SAFETY: pid 是本栈上的有效出参。
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != std::process::id() {
        return 1;
    }

    // 托盘窗口是隐藏的，理论上上面那条可见性就把它挡掉了；但托盘线程万一
    // 在别的时机露过面，按类名再挡一次更保险。
    let mut name = [0u16; 64];
    // SAFETY: name 是本栈上的可写缓冲区，长度按元素个数给出。
    let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    if len > 0 {
        let class = String::from_utf16_lossy(&name[..len as usize]);
        if class == TRAY_CLASS {
            return 1;
        }
    }

    // 标题优先：面板的标题就是软件名，比「面积最大」稳。
    let mut title = [0u16; 96];
    // SAFETY: title 是本栈上的可写缓冲区，长度按元素个数给出。
    let title_len = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    let matches_title = title_len > 0
        && String::from_utf16_lossy(&title[..title_len as usize]) == crate::theme::APP_NAME;

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: rect 是本栈上的有效出参。
    if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
        let area = i64::from(rect.right - rect.left) * i64::from(rect.bottom - rect.top);
        // 标题对上的窗口额外加权，确保它压过同进程里可能出现的其它可见窗口。
        let weight = if matches_title { area + i64::MAX / 2 } else { area };
        list.push((hwnd, weight));
    }
    1
}
