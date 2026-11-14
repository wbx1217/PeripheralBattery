//! 命令行读数：不启动界面，直接打印两台设备的电量。
//!
//! 用途有两个：一是排查「界面显示不对，到底是协议还是 UI 的问题」；二是写
//! 开机自检脚本。界面出问题时先跑这个，能省掉一半的定位时间。
//!
//! ```text
//! cargo run --bin status
//! ```

use std::thread;
use std::time::Duration;

use peripheral_battery::{Monitor, provider};

fn main() {
    let monitor = Monitor::start(provider::defaults());
    monitor.refresh_now();

    // 第一轮要等设备应答（休眠的耳机最长 900ms），多给一点余量。
    thread::sleep(Duration::from_millis(2_500));

    let snapshot = monitor.snapshot();
    if snapshot.devices.is_empty() {
        println!("没有读到任何设备");
        return;
    }

    for device in &snapshot.devices {
        let level = match device.status.percent() {
            Some(percent) => format!("{percent:>3}%"),
            None => "  --".to_owned(),
        };
        println!(
            "{:<16} {}  {:<10} ({} · 更新 {})",
            device.name,
            level,
            device.status.caption(),
            device.transport,
            device.updated_at.as_deref().unwrap_or("--:--")
        );
    }
}
