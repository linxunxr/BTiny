//! 托盘状态决策：图标四态优先级与 tooltip 组装（FR-02）。
//! V1.1 起输入为蓝牙 + 2.4G 罗技合并后的设备列表。

use crate::bluetooth::DeviceStatus;
use crate::icon::IconContent;

const TOOLTIP_MAX_DEVICES: usize = 5;

pub enum TrayState {
    /// 图标内容 + tooltip 全文
    Rendered(IconContent, String),
    /// 蓝牙无线电不可用（并入"无法读取"态，FR-02 蓝牙关闭；
    /// 注意 2.4G 设备在时不会进入此态——仍有可显示数据）
    RadioOff,
}

/// 状态优先级：低电量 > 无法读取 > 正常；空态独立（FR-02 图标状态）。
pub fn decide(radio_ok: bool, devices: &[DeviceStatus]) -> TrayState {
    if devices.is_empty() {
        if radio_ok {
            TrayState::Rendered(IconContent::Empty, "无已连接设备".to_string())
        } else {
            TrayState::RadioOff
        }
    } else {
        let min = devices.iter().filter_map(|d| d.battery).min();
        let content = match min {
            Some(v) => IconContent::Percent(v),
            None => IconContent::Unreadable,
        };
        TrayState::Rendered(content, tooltip(devices))
    }
}

/// Tooltip：电量升序、不可得垫底；每行一台；超过 5 台截断并补"…及其他 N 台"（FR-02）。
fn tooltip(devices: &[DeviceStatus]) -> String {
    let mut sorted: Vec<&DeviceStatus> = devices.iter().collect();
    sorted.sort_by_key(|d| d.battery.unwrap_or(255));
    let mut lines: Vec<String> = sorted
        .iter()
        .take(TOOLTIP_MAX_DEVICES)
        .map(|d| match d.battery {
            Some(v) => format!("{}  {}%", d.name, v),
            None => format!("{}  电量不可得", d.name),
        })
        .collect();
    if sorted.len() > TOOLTIP_MAX_DEVICES {
        lines.push(format!("…及其他 {} 台", sorted.len() - TOOLTIP_MAX_DEVICES));
    }
    lines.join("\n")
}
