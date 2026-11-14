//! 迈从 G87 键盘 —— `41E4:2001`，产品名 `MCHOSE 2.4G Wireless`。
//!
//! ```text
//! frame[0]     = 0x13    report id（不是额外前缀，就是这个字节）
//! frame[1]     = 0x4A    查询电量
//! frame[2..3]  = 01 00
//! frame[4..18] = 载荷
//! frame[19]    = 校验和 = sum(frame[0..18]) & 0xFF
//! ```
//!
//! 两个必须记住的坑：
//!
//! 1. 只能走 `HID SET_REPORT`（控制管道）。interrupt OUT 写下去没反应。
//! 2. 接收器会先把查询帧**原样回显**一遍（`frame[4] == 0`），必须跳过，
//!    否则会把回显当成 0% 读出来。

use std::time::Duration;

use crate::error::DeviceError;
use crate::hid::HidDevice;
use crate::model::DeviceKind;
use crate::provider::{BatteryProvider, Link, Reading};

/// USB 厂商号（注意不是 Voltide 记录的 `3554` / `3837`）。
pub const VENDOR_ID: u16 = 0x41E4;
/// USB 产品号。
pub const PRODUCT_ID: u16 = 0x2001;
/// 厂商自定义用途页。
pub const USAGE_PAGE: u16 = 0xFF02;
/// 集合用途号。只看用途页会把同页的其它集合（例如厂商调试口）一起认成键盘。
pub const USAGE: u16 = 0x0002;
/// 报文固定长度。
pub const FRAME_LEN: usize = 20;

const REPORT_ID: u8 = 0x13;
const CMD_BATTERY: u8 = 0x4A;
/// 收到应答后等下一帧的窗口。
const REPLY_BUDGET: Duration = Duration::from_millis(400);
/// 最多问几轮。接收器自己会休眠，第一帧往往只能把它叫醒而拿不到数据。
const ATTEMPTS: usize = 3;

/// 校验和：前 19 字节按字节相加取低 8 位。
#[must_use]
pub fn checksum(frame: &[u8]) -> u8 {
    frame
        .iter()
        .take(FRAME_LEN - 1)
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte))
}

/// 构造查询帧。
#[must_use]
pub fn build_query() -> [u8; FRAME_LEN] {
    let mut frame = [0u8; FRAME_LEN];
    frame[0] = REPORT_ID;
    frame[1] = CMD_BATTERY;
    frame[2] = 0x01;
    frame[3] = 0x00;
    frame[FRAME_LEN - 1] = checksum(&frame);
    frame
}

/// 解析应答帧。不合格的帧（含查询回显）一律返回 `None`。
#[must_use]
pub fn parse(frame: &[u8]) -> Option<Reading> {
    if frame.len() < FRAME_LEN {
        return None;
    }
    let frame = &frame[..FRAME_LEN];

    if frame[0] != REPORT_ID || frame[1] != CMD_BATTERY {
        return None;
    }
    // frame[4] == 0 是查询帧回显；只有长度 2 的载荷才是真应答。
    if frame[2] != 0x01 || frame[3] != 0x00 || frame[4] != 0x02 {
        return None;
    }
    if checksum(frame) != frame[FRAME_LEN - 1] {
        return None;
    }

    let percent = frame[5];
    if percent > 100 {
        return None;
    }
    // 低半字节 = fullStatus，高半字节 = chargeStatus（1 表示在充电）。
    let status = frame[6];
    Some(Reading {
        percent,
        charging: Some(status >> 4 == 1),
        status_byte: status,
    })
}

/// 迈从 G87。
pub struct MchoseG87;

impl BatteryProvider for MchoseG87 {
    fn key(&self) -> &'static str {
        "mchose-g87"
    }

    fn name(&self) -> &'static str {
        "MCHOSE G87"
    }

    fn kind(&self) -> DeviceKind {
        DeviceKind::Keyboard
    }

    fn matches(&self, device: &HidDevice) -> bool {
        device.vendor_id == VENDOR_ID
            && device.product_id == PRODUCT_ID
            && device.usage_page == USAGE_PAGE
            && device.usage == USAGE
    }

    fn read(&self, link: &mut Link) -> Result<Reading, DeviceError> {
        let query = build_query();
        let mut last = DeviceError::NoResponse;

        // 键盘闲置一会儿接收器就睡了，第一帧常被吞掉 —— 重发几次比延长超时更有效，
        // 因为唤醒靠的是「有帧到达」而不是「等得够久」。
        for _ in 0..ATTEMPTS {
            let session = link.session();
            // 先清干净，否则会读到上一轮残留的状态报文。
            session.flush(20);
            session.set_output_report(&query)?;

            match link.read_until(REPLY_BUDGET, parse) {
                Ok(reading) => return Ok(reading),
                Err(DeviceError::NoResponse) => last = DeviceError::NoResponse,
                Err(other) => return Err(other),
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机抓到的应答（99%）。
    const REAL_REPLY: [u8; FRAME_LEN] = [
        0x13, 0x4A, 0x01, 0x00, 0x02, 0x63, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0xC4,
    ];

    #[test]
    fn query_matches_captured_bytes() {
        let expected = [
            0x13, 0x4A, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x5E,
        ];
        assert_eq!(build_query(), expected);
    }

    #[test]
    fn parses_real_reply() {
        let reading = parse(&REAL_REPLY).expect("real reply should parse");
        assert_eq!(reading.percent, 99);
        assert_eq!(reading.charging, Some(false));
        assert_eq!(reading.status_byte, 0x01);
    }

    #[test]
    fn rejects_echo_frame() {
        // 查询帧原样回显：frame[4] == 0，绝不能当成 0% 读出来。
        assert!(parse(&build_query()).is_none());
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut frame = REAL_REPLY;
        frame[FRAME_LEN - 1] ^= 0xFF;
        assert!(parse(&frame).is_none());
    }

    #[test]
    fn rejects_out_of_range_percent() {
        let mut frame = REAL_REPLY;
        frame[5] = 101;
        frame[FRAME_LEN - 1] = checksum(&frame);
        assert!(parse(&frame).is_none());
    }

    #[test]
    fn rejects_short_frame() {
        assert!(parse(&REAL_REPLY[..8]).is_none());
    }
}
