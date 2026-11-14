//! 电量查询的厂商适配层。
//!
//! 一个 provider 对应一个协议（而不是一个品牌）：迈从键盘和 DARKRIM 耳机各
//! 一种。新增设备只需实现 [`BatteryProvider`] 并在 [`defaults`] 里注册。
//!
//! 约定：**帧格式的构造与解析必须是纯函数**（`build_query` / `parse`），
//! 只有「发出去、等回来」这一步才碰 IO。这样协议就能被字节固件单测覆盖，
//! 不需要真机。

use std::time::{Duration, Instant};

use crate::error::DeviceError;
use crate::hid::{HidDevice, HidSession};
use crate::model::DeviceKind;

/// 一次成功的电量读数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// 0–100。
    pub percent: u8,
    /// 是否在充电。
    pub charging: Option<bool>,
    /// 设备回的状态字节，保留原始值便于日志排查。
    pub status_byte: u8,
}

/// 一个打开的设备通道，附带协议需要的会话状态（如帧序号）。
pub struct Link {
    session: HidSession,
    seq: u8,
}

impl Link {
    /// 用给定的会话包装一条链路。
    #[must_use]
    pub fn new(session: HidSession) -> Self {
        Self { session, seq: 0 }
    }

    /// 取下一个帧序号（`0..=255` 循环）。
    pub fn next_seq(&mut self) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    /// 底层会话，用于 flush / 读写。
    #[must_use]
    pub fn session(&self) -> &HidSession {
        &self.session
    }

    /// 在 `budget` 内反复读，直到 `accept` 返回 `Some`。
    ///
    /// 设备会先原样回显查询帧，所以第一个报文几乎总是要丢掉的 —— 这也是
    /// 协议里写「必须跳过回显」的原因。
    ///
    /// # Errors
    ///
    /// 读操作本身失败时返回 [`DeviceError::Hid`]；一直没读到合格报文时
    /// 返回 [`DeviceError::NoResponse`]。
    pub fn read_until<T>(
        &self,
        budget: Duration,
        accept: impl Fn(&[u8]) -> Option<T>,
    ) -> Result<T, DeviceError> {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let slice = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX).max(60);
            let Some(report) = self.session.read(slice)? else {
                continue;
            };
            if let Some(value) = accept(&report) {
                return Ok(value);
            }
        }
        Err(DeviceError::NoResponse)
    }
}

/// 一种协议的电量读取器。
///
/// 实现必须是 `Send + Sync`：轮询线程独占调用，但托盘与 UI 会通过
/// `Arc` 持有它做展示。
pub trait BatteryProvider: Send + Sync + 'static {
    /// 稳定标识，用作 UI 上的 key 与配置持久化的名字。
    fn key(&self) -> &'static str;

    /// 型号显示名。
    fn name(&self) -> &'static str;

    /// 设备大类。
    fn kind(&self) -> DeviceKind;

    /// 连接方式的短描述，显示在卡片底部。
    fn transport(&self) -> &'static str {
        "2.4G HID"
    }

    /// 这台 HID 集合是不是本 provider 要的那一个。
    ///
    /// 只看 VID/PID 和用途页 —— 一个 dongle 会暴露多个集合，靠用途页挑出
    /// 厂商自定义的那个。
    fn matches(&self, device: &HidDevice) -> bool;

    /// 发一次查询并解析应答。
    ///
    /// # Errors
    ///
    /// 设备无应答、报文非法或 HID 出错时返回 [`DeviceError`]。
    fn read(&self, link: &mut Link) -> Result<Reading, DeviceError>;
}

/// 当前支持的设备清单。
///
/// 只收用户实际持有的两台；参考项目里那十几个品牌适配器不在本次范围内。
#[must_use]
pub fn defaults() -> Vec<Box<dyn BatteryProvider>> {
    vec![
        Box::new(crate::providers::mchose::MchoseG87),
        Box::new(crate::providers::darkrim::DarkrimShadow),
    ]
}
