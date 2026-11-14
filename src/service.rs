//! 后台轮询服务。
//!
//! 一条独立线程负责「枚举 → 匹配 → 查询 → 汇总」，UI 只读共享快照。
//! 线程与 UI 之间只有一把锁和一个版本号，没有任何回调 —— 界面卡不卡跟
//! HID IO 完全无关，设备休眠导致的 900ms 超时也影响不到渲染。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use crate::error::DeviceError;
use crate::hid;
use crate::model::{DeviceState, Snapshot, Status};
use crate::provider::{BatteryProvider, Link};

/// 默认刷新间隔：和 [`INTERVAL_CHOICES`] 里的「30 分」对应。
///
/// 面板开着还是收着都按这一个值来 —— 用户选了什么就是什么，
/// 软件不该在背后偷偷换成别的节奏。
pub const DEFAULT_INTERVAL_MS: u64 = 30 * 60 * 1000;

/// 刷新间隔的可选项，与面板底部的三个按钮一一对应。
pub const INTERVAL_CHOICES: [(u64, &str); 3] = [
    (5 * 60 * 1000, "5 分"),
    (30 * 60 * 1000, "30 分"),
    (60 * 60 * 1000, "60 分"),
];

/// 刷新间隔下限（毫秒）：再快也没有意义，只会让接收器忙到休眠不了。
pub const MIN_INTERVAL_MS: u64 = 1000;

/// 线程收到的指令。
enum Command {
    /// 立刻轮询一轮。
    Refresh,
    /// 刷新间隔变了，立刻重算下一轮等待时长。
    IntervalChanged,
    /// 退出线程。
    Stop,
}

/// 轮询线程共享的状态。
struct Shared {
    snapshot: Mutex<Snapshot>,
    interval_ms: AtomicU64,
}

/// 后台轮询器。`Drop` 时通知线程退出。
pub struct Monitor {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Monitor {
    /// 启动轮询线程。
    pub fn start(providers: Vec<Box<dyn BatteryProvider>>) -> Self {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot::default()),
            interval_ms: AtomicU64::new(DEFAULT_INTERVAL_MS),
        });
        let (commands, inbox) = mpsc::channel();

        let worker = Arc::clone(&shared);
        // 线程起不来不该让整个程序崩掉：没有后台轮询，界面还能显示「读取失败」，
        // 至少用户能看见程序在跑、能关掉它。
        let thread = thread::Builder::new()
            .name("battery-poll".to_owned())
            .spawn(move || run(providers, worker, inbox))
            .map_err(|error| {
                crate::diag::log(&format!("启动轮询线程失败：{error}"));
                error
            })
            .ok();

        Self {
            shared,
            commands,
            thread,
        }
    }

    /// 取一份当前快照。
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .snapshot
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 请求立刻刷新。
    pub fn refresh_now(&self) {
        let _ = self.commands.send(Command::Refresh);
    }

    /// 改刷新间隔（毫秒）。
    ///
    /// 小于 [`MIN_INTERVAL_MS`] 的请求会被抬到下限：`set_interval(0)` 会让
    /// `recv_timeout` 立刻返回，变成满速轮询把 CPU 和接收器一起打满。
    pub fn set_interval(&self, interval_ms: u64) {
        let interval_ms = clamp_interval(interval_ms);
        self.shared.interval_ms.store(interval_ms, Ordering::Relaxed);
        let _ = self.commands.send(Command::IntervalChanged);
    }

    /// 当前刷新间隔（毫秒）。
    #[must_use]
    pub fn interval_ms(&self) -> u64 {
        self.shared.interval_ms.load(Ordering::Relaxed)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 线程主循环。
fn run(providers: Vec<Box<dyn BatteryProvider>>, shared: Arc<Shared>, inbox: Receiver<Command>) {
    let mut version = 0u64;
    // 每台设备最后一次成功读到的时刻，失败时保留旧值 —— 「手机没信号」不该让
    // 上一次读数的时间戳消失。
    let mut stamps: Vec<Option<String>> = vec![None; providers.len()];
    // 枚举整体失败时保留上一轮结果：一次 API 抖动不该让界面集体显示「未检测到设备」。
    let mut last: Vec<DeviceState> = Vec::new();

    loop {
        let interval = shared.interval_ms.load(Ordering::Relaxed);
        match inbox.recv_timeout(Duration::from_millis(interval)) {
            Ok(Command::Stop) => break,
            Ok(Command::IntervalChanged | Command::Refresh) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // 换间隔的指令会立刻触发一次刷新，避免等完旧间隔才生效。
        if let Some(devices) = poll_once(&providers, &mut stamps) {
            crate::diag::log(
                &devices
                    .iter()
                    .map(|device| format!("{}={:?}", device.key, device.status))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            last = devices;
        }
        version += 1;
        let snapshot = Snapshot {
            devices: last.clone(),
            version,
        };
        if let Ok(mut guard) = shared.snapshot.lock() {
            *guard = snapshot;
        }
    }
}

/// 跑一轮：枚举一次，所有 provider 共享结果。
///
/// `None` 表示这一轮连设备列表都没拿到，调用方应保留上一轮的结果。
fn poll_once(
    providers: &[Box<dyn BatteryProvider>],
    stamps: &mut [Option<String>],
) -> Option<Vec<DeviceState>> {
    let found = match hid::enumerate() {
        Ok(found) => found,
        Err(error) => {
            crate::diag::log(&format!("枚举 HID 设备失败：{error}（本轮保持上一轮结果）"));
            return None;
        }
    };

    Some(
        providers
            .iter()
            .enumerate()
            .map(|(index, provider)| {
                let status = match found.iter().find(|device| provider.matches(device)) {
                    None => Status::Missing,
                    Some(device) => match open_link(device) {
                        // 枚举能看见、却打不开：权限、独占占用、句柄失效都可能。
                        // 这不是「没插」，也不是「休眠」。
                        Err(error) => {
                            crate::diag::log(&format!("打开 {} 失败：{error}", provider.key()));
                            Status::Error
                        }
                        Ok(mut link) => match provider.read(&mut link) {
                            Ok(reading) => {
                                stamps[index] = Some(now_hhmm());
                                Status::Online {
                                    percent: reading.percent,
                                    charging: reading.charging,
                                }
                            }
                            // 设备不应答/回了看不懂的帧：2.4G 外设休眠时的正常表现。
                            Err(DeviceError::NoResponse | DeviceError::Malformed(_)) => {
                                Status::Asleep
                            }
                            // 打开成功、IO 却失败：这是故障，不该伪装成休眠。
                            Err(DeviceError::Hid(error)) => {
                                crate::diag::log(&format!("读 {} 失败：{error}", provider.key()));
                                Status::Error
                            }
                        },
                    },
                };

                DeviceState {
                    key: provider.key(),
                    name: provider.name().to_owned(),
                    kind: provider.kind(),
                    transport: provider.transport(),
                    status,
                    updated_at: stamps[index].clone(),
                }
            })
            .collect(),
    )
}

/// 打开一台设备的厂商集合。
fn open_link(device: &hid::HidDevice) -> Result<Link, crate::error::HidError> {
    let session = hid::HidSession::open(&device.path, true)?.with_report_lengths(
        device.input_len,
        device.output_len,
        device.feature_len,
    );
    Ok(Link::new(session))
}

/// 本地时间的 `HH:MM`。
#[must_use]
pub fn now_hhmm() -> String {
    // SAFETY: 出参是本栈上的有效结构，函数不持有任何指针。
    let local = unsafe {
        let mut time = std::mem::zeroed();
        GetLocalTime(&mut time);
        time
    };
    format!("{:02}:{:02}", local.wHour, local.wMinute)
}

/// 把请求的间隔夹到合法区间。
///
/// 小于 [`MIN_INTERVAL_MS`] 会变成满速轮询：`recv_timeout` 立刻返回，CPU 和接收器
/// 一起被打满；`0` 尤其危险，所以统一在这里兜底。
#[must_use]
pub fn clamp_interval(interval_ms: u64) -> u64 {
    interval_ms.max(MIN_INTERVAL_MS)
}

#[cfg(test)]
mod tests {
    use super::{INTERVAL_CHOICES, MIN_INTERVAL_MS, clamp_interval};

    /// 0 和 1 毫秒都该被抬到下限，否则就是满速轮询。
    #[test]
    fn clamps_tiny_intervals() {
        assert_eq!(clamp_interval(0), MIN_INTERVAL_MS);
        assert_eq!(clamp_interval(1), MIN_INTERVAL_MS);
        assert_eq!(clamp_interval(u64::MAX), u64::MAX);
    }

    /// 三档候选都得是合法值，否则用户选了也会被夹走。
    #[test]
    fn every_choice_is_above_the_floor() {
        for (value, _) in INTERVAL_CHOICES {
            assert!(value >= MIN_INTERVAL_MS, "{value} 低于下限");
        }
    }
}
