//! 蓝牙电量扫描：30 秒轮询的一轮完整流程（FR-01）。
//!
//! 路径优先级：BLE GATT（一等公民，2 秒超时）→ 失败落经典蓝牙 DEVPKEY 属性。

pub mod ble;
pub mod classic;
pub mod enumerate;

use std::time::Duration;

/// 一台已连接设备的扫描结果。`battery == None` 表示电量不可得（FR-02 置灰标注）。
pub struct DeviceStatus {
    pub name: String,
    pub address: u64,
    pub battery: Option<u8>,
}

/// 扫描上下文：区分"无设备"与"蓝牙不可用"（FR-02 空态 vs 蓝牙关闭态）。
pub enum ScanOutcome {
    Devices(Vec<DeviceStatus>),
    RadioUnavailable,
}

/// 单轮扫描。全部阻塞调用都隔离在 spawn_blocking，不阻塞 tauri 事件循环（规格 4.2 关键约束）。
pub async fn scan_once() -> ScanOutcome {
    if !enumerate::radio_available() {
        return ScanOutcome::RadioUnavailable;
    }
    let paired = match tokio::task::spawn_blocking(enumerate::connected_paired).await {
        Ok(v) => v,
        Err(_) => return ScanOutcome::RadioUnavailable,
    };
    if paired.is_empty() {
        return ScanOutcome::Devices(vec![]);
    }

    // 经典蓝牙的属性表一轮只扫一次，摊给所有设备（SetupDi 枚举相对昂贵）
    let classic_map = tokio::task::spawn_blocking(classic::battery_map)
        .await
        .unwrap_or_default();

    let mut out = Vec::with_capacity(paired.len());
    for d in paired {
        // BLE GATT：设备串行尝试（一般 1–3 台，单台 ≤2s，远小于 30s 周期）
        let ble = tokio::time::timeout(Duration::from_secs(2), ble::read_battery_level(d.address))
            .await;
        let battery = match ble {
            Ok(Some(v)) => Some(v),
            // 超时/GATT 失败 → 经典蓝牙属性兜底；读不到即 None，不重试（FR-01）
            Err(_) | Ok(None) => classic_map.get(&d.address).copied().flatten(),
        };
        out.push(DeviceStatus { name: d.name, address: d.address, battery });
    }
    ScanOutcome::Devices(out)
}
