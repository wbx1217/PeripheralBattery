//! 设备与读数模型。
//!
//! UI 只认这里的类型，不接触任何协议细节；协议层只产出 [`Reading`]，
//! 由 [`crate::service`] 拼装成 [`Snapshot`] 交给界面。

/// 设备大类，决定卡片上的图标与文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// 键盘。
    Keyboard,
    /// 耳机。
    Headset,
}

impl DeviceKind {
    /// 中文分类名，用于托盘菜单与提示。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Keyboard => "键盘",
            Self::Headset => "耳机",
        }
    }
}

/// 一台设备此刻的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// 读到了电量。
    Online {
        /// 0–100 的百分比。
        percent: u8,
        /// 是否正在充电；`None` = 设备没告诉我们（别猜）。
        charging: Option<bool>,
    },
    /// 设备在，但不应答 —— 2.4G 外设休眠时的正常表现，并不代表故障。
    Asleep,
    /// 设备根本没插（或者拔掉了）。
    Missing,
    /// 设备在、也没睡，但这次读失败（权限、句柄失效、报文异常）。
    ///
    /// 和 `Asleep` 分开：休眠是外设的正常行为，读取失败要能被看见，否则排查时
    /// 只会看到「设备休眠中」，把 IO 问题当成硬件睡觉。
    Error,
}

impl Status {
    /// 电量百分比；非在线状态返回 `None`。
    #[must_use]
    pub fn percent(&self) -> Option<u8> {
        match *self {
            Self::Online { percent, .. } => Some(percent),
            Self::Asleep | Self::Missing | Self::Error => None,
        }
    }

    /// 卡片副标题的文案。
    #[must_use]
    pub fn caption(&self) -> &'static str {
        match *self {
            Self::Online {
                charging: Some(true),
                ..
            } => "正在充电",
            Self::Online {
                charging: Some(false),
                ..
            } => "电池放电中",
            // 协议里没有充电位（DARKRIM 的耳机只回百分比）：不编，直接不显示。
            Self::Online { charging: None, .. } => "电池状态未知",
            Self::Asleep => "设备休眠中",
            Self::Missing => "未检测到设备",
            Self::Error => "读取失败",
        }
    }
}

/// 一次查询后给 UI 的完整状态。
#[derive(Clone, Debug)]
pub struct DeviceState {
    /// provider 的稳定标识，用于做 UI 上的差异化处理。
    pub key: &'static str,
    /// 型号显示名，例如 `MCHOSE G87`。
    pub name: String,
    /// 设备大类。
    pub kind: DeviceKind,
    /// 连接方式，例如 `2.4G HID`。
    pub transport: &'static str,
    /// 当前状态。
    pub status: Status,
    /// 最后一次成功读到电量的时刻（`HH:MM`）。
    pub updated_at: Option<String>,
}

/// 一整轮轮询的结果。
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// 所有被管理的设备，顺序与 provider 注册顺序一致。
    pub devices: Vec<DeviceState>,
    /// 单调递增的版本号，UI 用它判断「有没有变」。
    pub version: u64,
}

impl Snapshot {
    /// 版本号是否比 `other` 新。
    #[must_use]
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.version > other.version
    }

    /// 在线设备数量。
    #[must_use]
    pub fn online_count(&self) -> usize {
        self.devices
            .iter()
            .filter(|device| matches!(device.status, Status::Online { .. }))
            .count()
    }

    /// 最该被关注的那台：托盘图标上画它的电量。
    ///
    /// 「最该关注」= 电量最低的在线设备，但**正在充电的不算** —— 那台已经被照顾
    /// 着了，再让它在托盘上常驻一个红色数字只是噪音。全都在充电时才退回按电量挑。
    ///
    /// 全部离线（休眠或没插）时返回 `None`，托盘改画通用图标。
    #[must_use]
    pub fn most_urgent(&self) -> Option<&DeviceState> {
        let online: Vec<&DeviceState> = self
            .devices
            .iter()
            .filter(|device| device.status.percent().is_some())
            .collect();
        let discharging: Vec<&DeviceState> = online
            .iter()
            .copied()
            .filter(|device| {
                matches!(
                    device.status,
                    Status::Online {
                        charging: Some(false),
                        ..
                    }
                )
            })
            .collect();

        // 有正在放电的就只在它们里面挑；全在充电（或不充电信息不可得）才放宽。
        let pool = if discharging.is_empty() {
            online
        } else {
            discharging
        };
        pool.into_iter()
            .min_by_key(|device| device.status.percent().unwrap_or(u8::MAX))
    }

    /// 托盘图标上要画的数字。
    #[must_use]
    pub fn tray_percent(&self) -> Option<u8> {
        self.most_urgent().and_then(|device| device.status.percent())
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceKind, DeviceState, Snapshot, Status};

    fn device(key: &'static str, status: Status) -> DeviceState {
        DeviceState {
            key,
            name: key.to_owned(),
            kind: DeviceKind::Keyboard,
            transport: "2.4G",
            status,
            updated_at: None,
        }
    }

    fn online(percent: u8, charging: bool) -> Status {
        Status::Online {
            percent,
            charging: Some(charging),
        }
    }

    /// 读失败不能伪装成休眠：一个是 IO 问题，一个是设备省电。
    #[test]
    fn error_status_is_not_asleep() {
        assert_eq!(Status::Error.caption(), "读取失败");
        assert_eq!(Status::Error.percent(), None);
        assert_ne!(Status::Error.caption(), Status::Asleep.caption());
    }

    /// 协议不带充电位的设备，说“未知”，不替它猜“放电中”。
    #[test]
    fn unknown_charging_says_unknown() {
        let status = Status::Online {
            percent: 80,
            charging: None,
        };
        assert_eq!(status.caption(), "电池状态未知");
        assert_eq!(status.percent(), Some(80));
    }

    /// 挑电量最低的那台。
    #[test]
    fn picks_the_lowest_charge() {
        let snapshot = Snapshot {
            devices: vec![
                device("kbd", online(90, false)),
                device("bud", online(30, false)),
            ],
            version: 1,
        };
        assert_eq!(snapshot.tray_percent(), Some(30));
    }

    /// 正在充电的低电量设备不该抢走托盘的注意力。
    #[test]
    fn skips_devices_that_are_charging() {
        let snapshot = Snapshot {
            devices: vec![
                device("kbd", online(80, false)),
                device("bud", online(5, true)),
            ],
            version: 1,
        };
        assert_eq!(snapshot.tray_percent(), Some(80));
    }

    /// 全在充电时只能退回按电量挑，否则托盘会没有数字。
    #[test]
    fn falls_back_when_everything_charges() {
        let snapshot = Snapshot {
            devices: vec![
                device("kbd", online(80, true)),
                device("bud", online(45, true)),
            ],
            version: 1,
        };
        assert_eq!(snapshot.tray_percent(), Some(45));
    }

    /// 全部离线时没有数字可画。
    #[test]
    fn nothing_online_means_no_number() {
        let snapshot = Snapshot {
            devices: vec![device("kbd", Status::Asleep), device("bud", Status::Missing)],
            version: 1,
        };
        assert_eq!(snapshot.tray_percent(), None);
        assert_eq!(snapshot.most_urgent().map(|d| d.key), None);
        assert_eq!(snapshot.online_count(), 0);
    }
}
