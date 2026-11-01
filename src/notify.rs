//! 低电量通知：每连接会话仅一次，断开重连后重置（FR-03）。

use std::collections::HashSet;

use crate::bluetooth::DeviceStatus;

const LOW_THRESHOLD: u8 = 20;

#[derive(Default)]
pub struct LowBatteryNotifier {
    /// 已通知过的设备地址集合；设备离开已连接集合即移除（= 会话结束，重连可再触发）
    notified: HashSet<u64>,
}

impl LowBatteryNotifier {
    /// 每轮扫描后调用；对首次降至阈值的设备逐台回调 `send(标题, 正文)`。
    pub fn on_scan(&mut self, devices: &[DeviceStatus], mut send: impl FnMut(&str, &str)) {
        let live: HashSet<u64> = devices.iter().map(|d| d.address).collect();
        self.notified.retain(|addr| live.contains(addr));
        for d in devices {
            if d.battery.is_some_and(|v| v <= LOW_THRESHOLD) && !self.notified.contains(&d.address) {
                send(
                    "BTiny 低电量提醒",
                    &format!("{} 电量 {}%，电量偏低，建议充电", d.name, d.battery.unwrap()),
                );
                self.notified.insert(d.address);
            }
        }
    }
}
