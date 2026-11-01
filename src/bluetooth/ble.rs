//! BLE 电量读取：WinRT GATT 读 BAS（0x180F / 0x2A19），规格 4.2 首选路径。
//!
//! windows-future 0.3 的 IAsyncOperation 仅实现 IntoFuture（无阻塞 get），
//! 故本模块为 async fn，由上层 tokio timeout 控制单设备 2 秒上限。
//! COM 线程模型由 main 的进程级 CoIncrementMTAUsage 保证，此处不再逐线程初始化。

use windows::Devices::Bluetooth::BluetoothLEDevice;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattCharacteristicUuids, GattCommunicationStatus, GattDeviceService,
    GattServiceUuids,
};

/// 读 BLE 电量（0–100）。任何一步失败返回 None——设备无 BAS、经典蓝牙地址、
/// GATT 通信失败等统一按"本轮不可得"处理，下轮自然重试（规格 7 章）。
pub async fn read_battery_level(address: u64) -> Option<u8> {
    // WinRT 地址参数为 u64 位型
    let device = BluetoothLEDevice::FromBluetoothAddressAsync(address)
        .ok()?
        .await
        .ok()?;
    let services = device
        .GetGattServicesForUuidAsync(GattServiceUuids::Battery().ok()?)
        .ok()?
        .await
        .ok()?;
    if services.Status().ok()? != GattCommunicationStatus::Success {
        let _ = device.Close();
        return None;
    }
    let service: GattDeviceService = services.Services().ok()?.First().ok()?.Current().ok()?;
    let characteristics_result = service
        .GetCharacteristicsForUuidAsync(GattCharacteristicUuids::BatteryLevel().ok()?)
        .ok()?
        .await
        .ok()?;
    if characteristics_result.Status().ok()? != GattCommunicationStatus::Success {
        let _ = service.Close();
        let _ = device.Close();
        return None;
    }
    let characteristic: GattCharacteristic = characteristics_result
        .Characteristics()
        .ok()?
        .First()
        .ok()?
        .Current()
        .ok()?;
    // 0.62 绑定仅生成无参 ReadValueAsync（WinRT 重载折叠），缓存模式取默认
    let read = characteristic.ReadValueAsync().ok()?.await.ok()?;
    let battery = if read.Status().ok()? == GattCommunicationStatus::Success {
        let buffer = read.Value().ok()?;
        let reader = windows::Storage::Streams::DataReader::FromBuffer(&buffer);
        reader.ok()?.ReadByte().ok()
    } else {
        None
    };
    // 规格 NFR-02：设备对象每轮显式释放，不依赖 drop 时机
    let _ = service.Close();
    let _ = device.Close();
    battery
}
