//! 2.4G 无线外设电量工具的协议层与 HID 访问层。
//!
//! 这个 crate 只负责「读」：枚举 HID 集合、打开句柄、收发报文、解析各厂商
//! 的电量应答，以及把结果汇总成 [`model::Snapshot`]。界面与托盘在 `app`
//! 里，不依赖本 crate 的任何 I/O 细节。

pub mod autostart;
pub mod diag;
pub mod error;
pub mod hid;
pub mod model;
pub mod prefs;
pub mod provider;
pub mod providers;
pub mod service;
pub mod tray;

pub use error::{DeviceError, HidError};
pub use model::{DeviceKind, DeviceState, Snapshot, Status};
pub use provider::{BatteryProvider, Reading};
pub use service::Monitor;
