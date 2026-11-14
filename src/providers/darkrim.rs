//! DARKRIM SHADOW 耳机 —— `3654:6901`，杰理（JieLi）方案的 2.4G dongle。
//!
//! ```text
//! frame[0]      = 0x00
//! frame[1]      = 0xFE    帧头
//! frame[2]      = 序号（每次请求 +1）
//! frame[3]      = 0x10 本机侧 / 0x11 耳机侧
//! frame[4]      = 0x00
//! frame[5]      = 0x02 / 0x03
//! frame[6]      = 分片序号
//! frame[7]      = 0x00    单帧请求；非零表示后面还有分片
//! frame[8..10]  = 载荷长度（小端）
//! frame[10..]   = 载荷（TLV：type + len + value）
//! 载荷之后      = CRC16（高字节在前）
//! ```
//!
//! 三个坑：
//!
//! 1. 只能走 interrupt OUT（`WriteFile`）。走控制管道写会返回 ERROR_INVALID_PARAMETER(87)。
//! 2. 耳机没开机 / 没连上时 dongle **完全沉默**，不返回错误 —— 这是休眠，不是故障。
//! 3. CRC 是 CRC16-CCITT-FALSE（poly `0x1021`、init `0xFFFF`、不反转），
//!    覆盖 `frame[1 .. 10 + len)`，**大端存放**。

use std::time::Duration;

use crate::error::DeviceError;
use crate::hid::HidDevice;
use crate::model::DeviceKind;
use crate::provider::{BatteryProvider, Link, Reading};

/// USB 厂商号。
pub const VENDOR_ID: u16 = 0x3654;
/// USB 产品号。
pub const PRODUCT_ID: u16 = 0x6901;
/// 厂商自定义用途页。
pub const USAGE_PAGE: u16 = 0xFF02;
/// 集合用途号：只看用途页会把同页的其它集合也当成耳机。
pub const USAGE: u16 = 0x0002;
/// 报文固定长度。
pub const FRAME_LEN: usize = 65;

const HEADER: u8 = 0xFE;
/// 本机侧；应答帧里也可能是耳机侧 0x11，两个都算合法。
const LOCAL_PEER: u8 = 0x10;
const REMOTE_PEER: u8 = 0x11;
/// 单帧数据类型；0x03 在多分片会话里出现。
const KIND_REQUEST: u8 = 0x02;
const KIND_DATA: u8 = 0x03;
const TLV_STATUS: u8 = 0x01;
/// 载荷起始偏移。
const PAYLOAD_OFFSET: usize = 10;
/// 等应答的窗口；耳机唤醒本身就要几百毫秒，给得比键盘宽。
const REPLY_BUDGET: Duration = Duration::from_millis(900);

/// CRC16-CCITT-FALSE。
#[must_use]
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// 构造查询帧：载荷是 `TLV(0x01, len 0)`，即「把状态报给我」。
#[must_use]
pub fn build_query(seq: u8) -> [u8; FRAME_LEN] {
    build_frame(seq, &[TLV_STATUS, 0x00])
}

/// 按协议拼一帧（含 CRC），供查询与将来其它 TLV 复用。
#[must_use]
pub fn build_frame(seq: u8, payload: &[u8]) -> [u8; FRAME_LEN] {
    let mut frame = [0u8; FRAME_LEN];
    frame[1] = HEADER;
    frame[2] = seq;
    frame[3] = LOCAL_PEER;
    frame[5] = 0x02;
    // 载荷放不下就截断，并且**长度字段跟着截断后的真实长度走** —— 早先长度
    // 字段写的是原值，对端会按超出实际数据的长度去读 CRC，整帧对不上。
    let capacity = FRAME_LEN - 2 - PAYLOAD_OFFSET;
    let payload = &payload[..payload.len().min(capacity)];
    let len = u16::try_from(payload.len()).unwrap_or(0);
    frame[8..10].copy_from_slice(&len.to_le_bytes());
    let end = PAYLOAD_OFFSET + payload.len();
    frame[PAYLOAD_OFFSET..end].copy_from_slice(payload);

    let crc = crc16(&frame[1..end]);
    frame[end] = (crc >> 8) as u8;
    frame[end + 1] = crc as u8;
    frame
}

/// 帧头是否像一张完整的单帧报文。
///
/// 序号不做相等校验：抓包里的应答序号不必然等于请求序号（固件会自己排），
/// 卡这一条只会把真应答全部丢掉。这里只挡「方向/分片/类型」明显不对的帧，
/// 免得把多分片会话的中间片当成状态帧解析。
fn header_is_plausible(frame: &[u8]) -> bool {
    frame[0] == 0x00
        && frame[1] == HEADER
        && (frame[3] == LOCAL_PEER || frame[3] == REMOTE_PEER)
        && (frame[5] == KIND_REQUEST || frame[5] == KIND_DATA)
        // frame[7] 非零表示后面还有分片：单帧读取不该把它当最终结果。
        && frame[7] == 0x00
}

/// 解析应答帧。校验失败或 TLV 不是状态项时返回 `None`。
#[must_use]
pub fn parse(frame: &[u8]) -> Option<Reading> {
    if frame.len() < PAYLOAD_OFFSET + 2 || !header_is_plausible(frame) {
        return None;
    }

    let len = usize::from(u16::from_le_bytes([frame[8], frame[9]]));
    let end = PAYLOAD_OFFSET.checked_add(len)?;
    if end + 2 > frame.len() {
        return None;
    }

    let expected = u16::from_be_bytes([frame[end], frame[end + 1]]);
    if crc16(&frame[1..end]) != expected {
        return None;
    }

    let payload = &frame[PAYLOAD_OFFSET..end];
    let mut cursor = 0usize;
    while cursor + 2 <= payload.len() {
        let kind = payload[cursor];
        let size = usize::from(payload[cursor + 1]);
        let value_start = cursor + 2;
        let value_end = value_start + size;
        if value_end > payload.len() {
            return None;
        }
        if kind == TLV_STATUS && size >= 1 {
            let percent = payload[value_start];
            if percent > 100 {
                return None;
            }
            return Some(Reading {
                percent,
                charging: None,
                status_byte: payload[value_end - 1],
            });
        }
        cursor = value_end;
    }
    None
}

/// DARKRIM SHADOW。
pub struct DarkrimShadow;

impl BatteryProvider for DarkrimShadow {
    fn key(&self) -> &'static str {
        "darkrim-shadow"
    }

    fn name(&self) -> &'static str {
        "DARKRIM SHADOW"
    }

    fn kind(&self) -> DeviceKind {
        DeviceKind::Headset
    }

    fn transport(&self) -> &'static str {
        "2.4G · RCSP"
    }

    fn matches(&self, device: &HidDevice) -> bool {
        device.vendor_id == VENDOR_ID
            && device.product_id == PRODUCT_ID
            && device.usage_page == USAGE_PAGE
    }

    fn read(&self, link: &mut Link) -> Result<Reading, DeviceError> {
        let seq = link.next_seq();
        let query = build_query(seq);

        let session = link.session();
        session.flush(20);
        session.write(&query, 300)?;

        link.read_until(REPLY_BUDGET, parse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机抓到的查询帧（序号 0）。
    const REAL_QUERY: [u8; FRAME_LEN] = [
        0x00, 0xFE, 0x00, 0x10, 0x00, 0x02, 0x00, 0x00, 0x02, 0x00, 0x01, 0x00, 0x58, 0x09, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// 真机抓到的应答（80%）。
    const REAL_REPLY: [u8; 16] = [
        0x00, 0xFE, 0x00, 0x10, 0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x01, 0x01, 0x50, 0x11, 0x68,
        0x00,
    ];

    #[test]
    fn query_matches_captured_bytes() {
        assert_eq!(build_query(0), REAL_QUERY);
    }

    #[test]
    fn crc_matches_captured_frame() {
        // 抓包里 CRC 是 0x1168，覆盖到载荷末尾。
        assert_eq!(crc16(&REAL_QUERY[1..12]), 0x5809);
        assert_eq!(crc16(&REAL_REPLY[1..13]), 0x1168);
    }

    #[test]
    fn parses_real_reply() {
        let reading = parse(&REAL_REPLY).expect("real reply should parse");
        assert_eq!(reading.percent, 80);
        assert_eq!(reading.charging, None);
    }

    #[test]
    fn rejects_query_echo() {
        // 查询帧本身载荷是 TLV(0x01, len 0)，没有数值，不能当 0% 读。
        assert!(parse(&REAL_QUERY).is_none());
    }

    #[test]
    fn rejects_corrupted_crc() {
        let mut frame = REAL_REPLY;
        frame[13] ^= 0xFF;
        assert!(parse(&frame).is_none());
    }

    #[test]
    fn rejects_truncated_payload() {
        let mut frame = REAL_REPLY;
        frame[8] = 0x40; // 声称 64 字节载荷，实际没有
        assert!(parse(&frame).is_none());
    }

    #[test]
    fn rejects_fragment_continuation() {
        let mut frame = REAL_REPLY;
        frame[7] = 0x01; // 「后面还有分片」
        assert!(parse(&frame).is_none());
    }

    #[test]
    fn rejects_unknown_peer() {
        let mut frame = REAL_REPLY;
        frame[3] = 0x7F;
        assert!(parse(&frame).is_none());
    }

    #[test]
    fn rejects_wrong_frame_kind() {
        let mut frame = REAL_REPLY;
        frame[5] = 0x09;
        assert!(parse(&frame).is_none());
    }

    #[test]
    fn oversized_payload_keeps_length_field_consistent() {
        // 载荷比剩余空间大：长度字段必须等于实际写入的字节数。
        let payload = [0xAB_u8; FRAME_LEN];
        let frame = build_frame(1, &payload);
        let len = usize::from(u16::from_le_bytes([frame[8], frame[9]]));
        assert_eq!(len, FRAME_LEN - 2 - PAYLOAD_OFFSET);
        let expected = u16::from_be_bytes([frame[PAYLOAD_OFFSET + len], frame[PAYLOAD_OFFSET + len + 1]]);
        assert_eq!(crc16(&frame[1..PAYLOAD_OFFSET + len]), expected);
    }

    #[test]
    fn seq_is_carried_through() {
        let frame = build_query(7);
        assert_eq!(frame[2], 7);
    }
}
