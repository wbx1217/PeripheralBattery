//! 托盘图标 + 右键菜单。
//!
//! GPUI 没有托盘 API，所以这里自己封 `Shell_NotifyIconW`：一条独立线程跑自己的
//! 消息循环（`GetMessage` 是线程阻塞的，不能和 GPUI 的事件循环共用），用
//! `PostMessage` 双向通信。
//!
//! 托盘线程不直接调 GPUI —— 它只往队列里塞 [`TrayEvent`]，界面线程每个 tick
//! 取一次。跨线程碰 GPUI 是自找麻烦。

pub mod icon;

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICON_VERSION_4, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, EndMenu, FindWindowW, GWLP_USERDATA, GetCursorPos, GetMessageW,
    GetWindowLongPtrW,
    HICON, HMENU, MENUITEMINFOW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MFT_RADIOCHECK, MFT_STRING,
    MIIM_FTYPE, MSG, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetMenuItemInfoW, SetWindowLongPtrW, TrackPopupMenu, TranslateMessage,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_WORKAREA, UnregisterClassW, WNDCLASSW,
    WS_OVERLAPPED,
};

use icon::{destroy_icon, icon_size, make_icon};

/// 托盘回调消息（`WM_APP + 1`）。
const WM_TRAY: u32 = 0x8000 + 1;
/// 有新的状态 / 菜单状态要应用（`WM_APP + 2`）。
const WM_REFRESH: u32 = 0x8000 + 2;
/// 退出消息循环（`WM_APP + 3`）。
const WM_SHUTDOWN: u32 = 0x8000 + 3;
/// 另一个实例让我们把面板显示出来（`WM_APP + 4`）。
const WM_SHOW_PANEL: u32 = 0x8000 + 4;
/// 托盘图标 id。
const ICON_ID: u32 = 1;

/// Explorer 广播的 `TaskbarCreated` 消息 id（`0` 表示还没注册）。
///
/// Explorer 重启（崩溃、更新、装完软件重启外壳）会把所有托盘图标清空，只重新
/// 注册一次是不够的：不监听这条广播，图标就永久消失了。
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// 低电阈值：低于它托盘图标里的闪电变红（和面板里的语义色同一档）。
const LOW_THRESHOLD: u8 = 20;

/// 菜单命令 id。
mod menu_id {
    pub const SHOW: usize = 1;
    pub const AUTOSTART: usize = 3;
    pub const INTERVAL_BASE: usize = 10;
    pub const QUIT: usize = 99;
}

/// 托盘想告诉界面的事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// 左键点了图标：显示 / 隐藏面板。
    ToggleWindow,
    /// 菜单里点了「显示面板」：只显示，不切换。
    ShowWindow,
    /// 菜单里点了「开机自启」。
    ToggleAutostart,
    /// 菜单里改了刷新间隔。
    SetInterval(u64),
    /// 菜单里点了「退出」。
    Quit,
}

/// 托盘线程共享的数据。
struct Shared {
    events: Mutex<VecDeque<TrayEvent>>,
    /// 当前 tooltip 与电量。
    status: Mutex<(String, Option<u8>)>,
    /// 菜单里要展示的勾选状态。
    menu: Mutex<MenuState>,
    /// 当前图标句柄，替换时要销毁旧的。
    current_icon: Mutex<isize>,
    /// 上一次画进图标的「像素尺寸 + 是否低电」。
    ///
    /// 尺寸跟着系统 DPI 走（150% 下是 24px），DPI 变了也得重画，否则系统会把
    /// 小图放大，边缘发虚。
    painted: Mutex<(i32, bool)>,
}

/// 菜单需要的状态。
#[derive(Clone, Copy, Default)]
struct MenuState {
    autostart: bool,
    interval_ms: u64,
}

/// 电量是否已经低到要报警。
fn is_low(percent: Option<u8>) -> bool {
    percent.is_some_and(|value| value < LOW_THRESHOLD)
}

/// 找到已经在跑的实例，让它把面板显示出来。
///
/// 返回 `false` 表示没找到（对方可能还在启动，或者托盘窗口没建起来）。
#[must_use]
pub fn activate_existing() -> bool {
    let class = wide("PeripheralBatteryTrayWnd");
    // SAFETY: class 以 NUL 结尾；只按类名找窗口，不碰对方进程的内存。
    let hwnd = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: 只投递一条自定义消息，参数都是整数。
    let posted = unsafe { PostMessageW(hwnd, WM_SHOW_PANEL, 0, 0) };
    posted != 0
}

/// 托盘句柄。`Drop` 时结束托盘线程并移除图标。
pub struct Tray {
    /// 托盘窗口句柄，按 `isize` 存 —— 裸指针会让 `Tray` 变成 `!Send`，
    /// 而界面层需要把它放进 GPUI 的实体里。
    hwnd: isize,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Tray {
    /// 创建托盘图标并启动消息线程。
    ///
    /// # Errors
    ///
    /// 线程创建失败时返回错误；Win32 侧的失败（注册窗口类、创建窗口）会以
    /// 静默失败处理 —— 没有托盘也不该让整个程序起不来。
    pub fn spawn() -> io::Result<Self> {
        let shared = Arc::new(Shared {
            events: Mutex::new(VecDeque::new()),
            status: Mutex::new((String::from("外设电量显示"), None)),
            menu: Mutex::new(MenuState::default()),
            current_icon: Mutex::new(0),
            painted: Mutex::new((0, false)),
        });
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<isize>();

        let worker = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("tray".to_owned())
            .spawn(move || run(worker, ready_tx))?;

        // 有界等待：托盘线程若卡在建窗口/挂图标上，界面线程不能陪着一起卡死。
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(hwnd) => Ok(Self {
                hwnd,
                shared,
                thread: Some(thread),
            }),
            Err(error) => {
                // 线程自己会继续走完；这里放弃持有它，界面层按「没有托盘」处理。
                crate::diag::log(&format!("等待托盘线程就绪超时：{error}"));
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "托盘线程 3 秒内没有就绪",
                ))
            }
        }
    }

    /// 更新托盘提示与图标上的数字。
    pub fn set_status(&self, tooltip: &str, percent: Option<u8>) {
        if let Ok(mut guard) = self.shared.status.lock() {
            *guard = (tooltip.to_owned(), percent);
        }
        post(self.hwnd, WM_REFRESH);
    }

    /// 更新菜单勾选状态。
    pub fn set_menu_state(&self, autostart: bool, interval_ms: u64) {
        if let Ok(mut guard) = self.shared.menu.lock() {
            *guard = MenuState {
                autostart,
                interval_ms,
            };
        }
        post(self.hwnd, WM_REFRESH);
    }

    /// 取走当前积压的事件。
    pub fn drain(&self) -> Vec<TrayEvent> {
        self.shared
            .events
            .lock()
            .map(|mut guard| guard.drain(..).collect())
            .unwrap_or_default()
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        post(self.hwnd, WM_SHUTDOWN);

        // 有界等待：托盘线程可能正卡在 `TrackPopupMenu` 的模态循环或
        // `Shell_NotifyIconW` 里，真卡住也不能把整个进程的退路堵死。
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_millis(600);
        while !thread.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            // 放弃等待，句柄 drop 即 detach —— 线程自己会走完并退出。
            crate::diag::log("托盘线程未在 600ms 内退出，放弃等待");
        }
    }
}

/// 发一条不需要参数的消息。
fn post(hwnd: isize, message: u32) {
    if hwnd == 0 {
        return;
    }
    // SAFETY: hwnd 由托盘线程创建，消息值都在 WM_APP 段内，由本模块的窗口过程处理。
    unsafe { PostMessageW(hwnd as HWND, message, 0, 0) };
}

/// 托盘线程主体。
fn run(shared: Arc<Shared>, ready: std::sync::mpsc::Sender<isize>) {
    let class_name = wide("PeripheralBatteryTrayWnd");
    // 先注册广播 id，再建窗口：Explorer 重启时会往所有顶层窗口发这条消息。
    // SAFETY: 字符串以 NUL 结尾，调用期间一直有效；注册失败返回 0，后面会跳过比较。
    let taskbar = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    TASKBAR_CREATED.store(taskbar, Ordering::Relaxed);
    // SAFETY: hInstance 传 0 表示当前进程；下面所有调用都用本线程创建的句柄。
    let hwnd = unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        let registered = RegisterClassW(&class);
        if registered == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            // 类已注册（例如上一个实例刚退出）不算错误，其它错误就让窗口建不出来。
            crate::diag::log(&format!("注册托盘窗口类失败：错误码 {}", GetLastError()));
            ptr::null_mut()
        } else {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                wide("PeripheralBattery").as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            )
        }
    };

    if hwnd.is_null() {
        let _ = ready.send(0);
        return;
    }

    // 窗口过程通过 GWLP_USERDATA 拿到共享状态；生命周期由 `Tray` 持有，托盘
    // 线程一定先于 `Tray` 结束（Drop 里先发退出消息再 join），因此裸指针安全。
    let shared_ptr = Arc::as_ptr(&shared).cast::<c_void>() as isize;
    // SAFETY: hwnd 有效，索引是系统定义的 GWLP_USERDATA。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, shared_ptr) };

    add_icon(hwnd, &shared);
    let _ = ready.send(hwnd as isize);

    // SAFETY: 标准 Win32 消息循环；msg 是本栈上的有效结构。
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    remove_icon(hwnd, &shared);
    // SAFETY: 窗口与窗口类都由本线程创建，在此配对销毁。
    unsafe {
        DestroyWindow(hwnd);
        UnregisterClassW(class_name.as_ptr(), GetModuleHandleW(ptr::null()));
    }
}

/// 把图标挂到托盘上。
fn add_icon(hwnd: HWND, shared: &Shared) {
    let (tooltip, percent) = shared
        .status
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let low = is_low(percent);
    let icon = make_icon(low);
    if icon.is_null() {
        // 画不出来就别注册：注册一个空图标，用户看到的是托盘里的透明占位。
        crate::diag::log("托盘图标创建失败，跳过注册");
        return;
    }

    let mut data = notify_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    fill_tip(&mut data, &tooltip);

    // SAFETY: data 已按系统要求填好 cbSize 与有效句柄。
    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
    if added == 0 {
        // 没挂上：图标没人用，销毁掉；顺手把「已画过」退回未画，下次刷新重试。
        destroy_icon(icon);
        if let Ok(mut guard) = shared.painted.lock() {
            *guard = (0, false);
        }
        crate::diag::log(&format!(
            "托盘图标注册失败：Shell_NotifyIcon(NIM_ADD)=0，HWND={hwnd:p}，提示={tooltip:?}"
        ));
        return;
    }

    // 版本 4 才有 NIN_SELECT / WM_CONTEXTMENU 语义，右键菜单定位才正常。
    // SETVERSION 只看 uVersion，顺手把 ADD 用的标志清掉。
    data.uFlags = 0;
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    // SAFETY: 同一份已填好 cbSize 的结构。
    let versioned = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
    crate::diag::log(&format!(
        "托盘图标注册：NIM_ADD=1，NIM_SETVERSION={versioned}，HWND={hwnd:p}，图标句柄={icon:p}，提示={tooltip:?}"
    ));

    store_icon(shared, icon);
    store_painted(shared, icon_size(), low);
}

/// 移除托盘图标。
fn remove_icon(hwnd: HWND, shared: &Shared) {
    let data = notify_data(hwnd);
    // SAFETY: 句柄有效；重复删除是无害的。
    unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    if let Ok(mut guard) = shared.current_icon.lock() {
        let old = *guard;
        *guard = 0;
        if old != 0 {
            destroy_icon(old as HICON);
        }
    }
}

/// 用最新状态刷新 tooltip 与图标。
fn refresh_icon(hwnd: HWND, shared: &Shared) {
    let (tooltip, percent) = shared
        .status
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    // tooltip 和菜单状态都会触发刷新，但只有「低电状态」或「DPI 尺寸」变了
    // 才值得重画图标：GDI 画一遍不便宜，而且换图标会让托盘闪一下。
    let low = is_low(percent);
    let size = icon_size();
    let repaint = store_painted(shared, size, low);

    let mut data = notify_data(hwnd);
    data.uFlags = NIF_TIP | NIF_SHOWTIP;
    fill_tip(&mut data, &tooltip);
    let icon = if repaint {
        let icon = make_icon(low);
        if icon.is_null() {
            // 画不出来：保持旧图标，把「已画过」退回未画，下次刷新再试。
            if let Ok(mut guard) = shared.painted.lock() {
                *guard = (0, false);
            }
            ptr::null_mut()
        } else {
            data.uFlags |= NIF_ICON;
            data.hIcon = icon;
            icon
        }
    } else {
        ptr::null_mut()
    };
    // SAFETY: data 填好 cbSize，需要图标时句柄也有效。
    let modified = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };

    if !repaint {
        return;
    }
    if icon.is_null() {
        return;
    }
    if modified == 0 {
        // 系统没接收这次替换：新图标没人用，销毁；旧图标还在托盘上，绝不能动。
        destroy_icon(icon);
        if let Ok(mut guard) = shared.painted.lock() {
            *guard = (0, false);
        }
        crate::diag::log("托盘图标刷新失败：Shell_NotifyIcon(NIM_MODIFY)=0");
        return;
    }
    if let Ok(mut guard) = shared.current_icon.lock() {
        let old = *guard;
        *guard = icon as isize;
        if old != 0 && old != icon as isize {
            destroy_icon(old as HICON);
        }
    }
}

fn store_icon(shared: &Shared, icon: HICON) {
    if let Ok(mut guard) = shared.current_icon.lock() {
        let old = *guard;
        *guard = icon as isize;
        // 重新注册（Explorer 重启）时会换一枚新图标，旧的必须销毁，否则 GDI 句柄越攒越多。
        if old != 0 && old != icon as isize {
            destroy_icon(old as HICON);
        }
    }
}

/// 记下「图标画的是什么」，返回是否需要重画。
///
/// 尺寸跟着系统 DPI 变（150% 下是 24px），所以除了低电状态还要比尺寸。
fn store_painted(shared: &Shared, size: i32, low: bool) -> bool {
    let Ok(mut guard) = shared.painted.lock() else {
        return true;
    };
    let wanted = (size, low);
    if *guard == wanted {
        return false;
    }
    *guard = wanted;
    true
}

/// 建一个指向本窗口的 NOTIFYICONDATAW。
fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW 是全零合法的 POD 结构。
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data
}

/// 把字符串写进 `szTip`（定长，必须自己截断）。
fn fill_tip(data: &mut NOTIFYICONDATAW, text: &str) {
    // 按字符而不是按 UTF-16 码元截断：`take(n)` 可能正好切在代理对中间，
    // 那样 tooltip 末尾会冒出一个替换字符。
    let limit = data.szTip.len() - 1;
    let mut used = 0;
    for ch in text.chars() {
        let mut buffer = [0u16; 2];
        let units = ch.encode_utf16(&mut buffer);
        if used + units.len() > limit {
            break;
        }
        data.szTip[used..used + units.len()].copy_from_slice(units);
        used += units.len();
    }
    data.szTip[used] = 0;
}

/// 窗口过程。
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Explorer 重启会广播 TaskbarCreated：托盘里的图标已被清空，必须重新挂一次。
    let taskbar = TASKBAR_CREATED.load(Ordering::Relaxed);
    if taskbar != 0 && message == taskbar {
        // SAFETY: 窗口由本线程创建，USERDATA 指向存活的 Shared。
        if let Some(shared) = unsafe { shared_of(hwnd) } {
            crate::diag::log("收到 TaskbarCreated：重新挂托盘图标");
            if let Ok(mut guard) = shared.painted.lock() {
                *guard = (0, false);
            }
            add_icon(hwnd, shared);
        }
        return 0;
    }

    match message {
        WM_TRAY => {
            // SAFETY: 由本线程的 CreateWindowExW 创建，USERDATA 指向存活的 Shared。
            let shared = unsafe { shared_of(hwnd) };
            let Some(shared) = shared else {
                return 0;
            };
            let event = (lparam & 0xFFFF) as u32;
            match event {
                // WM_LBUTTONUP / NIN_SELECT / NIN_KEYSELECT
                0x0202 | 0x0400 | 0x0401 => push(shared, TrayEvent::ToggleWindow),
                // WM_RBUTTONUP / WM_CONTEXTMENU：两种消息都只用光标位置，
                // 不碰 wParam（版本 4 下它在 WM_CONTEXTMENU 里没有意义）。
                0x0205 | 0x007B => show_menu(hwnd, shared),
                _ => {}
            }
            0
        }
        WM_REFRESH => {
            // SAFETY: 同上。
            if let Some(shared) = unsafe { shared_of(hwnd) } {
                refresh_icon(hwnd, shared);
            }
            0
        }
        WM_SHUTDOWN => {
            // 菜单正开着时模态循环不认 `WM_QUIT`，先 `EndMenu` 把它叫停。
            // SAFETY: 没有活动菜单时 `EndMenu` 是无害的空操作。
            unsafe {
                EndMenu();
                PostQuitMessage(0);
            }
            0
        }
        WM_SHOW_PANEL => {
            // SAFETY: 同上。
            if let Some(shared) = unsafe { shared_of(hwnd) } {
                push(shared, TrayEvent::ShowWindow);
            }
            0
        }
        // SAFETY: 其余消息交回系统默认处理。
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// 取回窗口上挂的共享状态。
unsafe fn shared_of(hwnd: HWND) -> Option<&'static Shared> {
    // SAFETY: 索引是系统定义的槽位；值由 `run` 写入，指向它自己持有的
    // `Arc<Shared>`。只要托盘线程还活着，那块内存就不会被释放。
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Shared;
    // SAFETY: 指针来自 Arc::as_ptr，在托盘线程存活期间一直有效。
    unsafe { raw.as_ref() }
}

fn push(shared: &Shared, event: TrayEvent) {
    if let Ok(mut guard) = shared.events.lock() {
        guard.push_back(event);
    }
}

/// 弹出右键菜单。
///
/// 锚点只用 `GetCursorPos`：`NOTIFYICON_VERSION_4` 只保证在
/// `NIN_POPUPOPEN` / `NIN_SELECT` / `NIN_KEYSELECT` 和鼠标消息里 `wParam`
/// 才是图标矩形，**`WM_CONTEXTMENU` 的 `wParam` 是未定义的** —— 旧代码
/// 拿它当坐标，于是菜单时而贴着图标、时而弹到屏幕角落几百像素外。
///
/// 位置也不再自己算：交给 `TPM_WORKAREA`，系统会把菜单翻到工作区内。
fn show_menu(hwnd: HWND, shared: &Shared) {
    let state = shared.menu.lock().map(|guard| *guard).unwrap_or_default();

    let handle = {
        // SAFETY: 菜单句柄在本函数内创建并销毁，菜单项的字符串在调用期间有效。
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            // 三个间隔只写间隔本身（「10 秒」），不再写「每 10 秒刷新」——
            // 长文案会把整条菜单撑宽。
            append(menu, MF_STRING, menu_id::SHOW, "显示");
            append(
                menu,
                MF_STRING | if state.autostart { MF_CHECKED } else { 0 },
                menu_id::AUTOSTART,
                "自启",
            );
            // 勾选状态也画成圆点，和下面三档间隔的样式保持一致。
            let autostart_info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE,
                fType: MFT_STRING | MFT_RADIOCHECK,
                ..Default::default()
            };
            SetMenuItemInfoW(menu, menu_id::AUTOSTART as u32, 0, &autostart_info);
            append(menu, MF_SEPARATOR, 0, "");

            // 三个互斥的刷新间隔用单选圆点：`AppendMenuW` 没有单选标志，
            // `MFT_RADIOCHECK` 只能通过 `SetMenuItemInfoW` 设到项类型上。
            for (index, (interval, label)) in crate::service::INTERVAL_CHOICES.iter().enumerate() {
                let id = menu_id::INTERVAL_BASE + index;
                let flags = MF_STRING
                    | if *interval == state.interval_ms {
                        MF_CHECKED
                    } else {
                        0
                    };
                append(menu, flags, id, label);
                let info = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE,
                    fType: MFT_STRING | MFT_RADIOCHECK,
                    ..Default::default()
                };
                SetMenuItemInfoW(menu, id as u32, 0, &info);
            }
            append(menu, MF_SEPARATOR, 0, "");
            append(menu, MF_STRING, menu_id::QUIT, "退出");

            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            // 不先抢前台，菜单点外面不会关闭 —— 微软文档里写的经典坑。
            SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_WORKAREA,
                point.x,
                point.y,
                0,
                hwnd,
                ptr::null(),
            );
            // 菜单关掉之后补一条 WM_NULL：不然第一次点击会被菜单的模态循环吃掉。
            PostMessageW(hwnd, 0, 0, 0);
            DestroyMenu(menu);
            crate::diag::log(&format!(
                "托盘菜单弹出：锚点=({}, {})，命令={command}",
                point.x, point.y
            ));
            command
        }
    };

    let command = usize::try_from(handle).unwrap_or(0);
    match command {
        0 => {}
        menu_id::SHOW => push(shared, TrayEvent::ShowWindow),
        menu_id::AUTOSTART => push(shared, TrayEvent::ToggleAutostart),
        menu_id::QUIT => push(shared, TrayEvent::Quit),
        other if other >= menu_id::INTERVAL_BASE => {
            if let Some((interval, _)) = crate::service::INTERVAL_CHOICES
                .get(other - menu_id::INTERVAL_BASE)
            {
                push(shared, TrayEvent::SetInterval(*interval));
            }
        }
        _ => {}
    }
}

/// 加一条菜单项。
fn append(menu: HMENU, flags: u32, id: usize, text: &str) {
    let text = wide(text);
    // SAFETY: menu 有效，text 在本调用期间一直有效。
    unsafe { AppendMenuW(menu, flags, id, text.as_ptr()) };
}

/// UTF-16 NUL 结尾字符串。
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{LOW_THRESHOLD, is_low};

    /// 阈值边界：19% 报警、20% 不报警，和面板的 `level_hex` 同一档。
    #[test]
    fn low_threshold_boundary() {
        assert!(is_low(Some(0)));
        assert!(is_low(Some(LOW_THRESHOLD - 1)));
        assert!(!is_low(Some(LOW_THRESHOLD)));
        assert!(!is_low(Some(100)));
    }

    /// 没有读数（设备休眠 / 未连接）不算低电，图标保持常态。
    #[test]
    fn no_reading_is_not_low() {
        assert!(!is_low(None));
    }
}
