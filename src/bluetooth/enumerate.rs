//! Win32 蓝牙枚举：已配对且当前已连接的设备（FR-01 设备枚举）。
//! windows 0.62 的枚举 API 返回 Result 形态（Err 即枚举结束/不可用）。

use windows::Win32::Devices::Bluetooth::{
    BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice,
    BluetoothFindFirstRadio, BluetoothFindRadioClose, BLUETOOTH_DEVICE_INFO,
    BLUETOOTH_DEVICE_SEARCH_PARAMS, BLUETOOTH_FIND_RADIO_PARAMS,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};

pub struct PairedDevice {
    pub name: String,
    pub address: u64,
}

/// 蓝牙无线电是否存在（供 FR-02 "蓝牙已关闭或不可用"态判定）。
pub fn radio_available() -> bool {
    unsafe {
        let mut radio = HANDLE::default();
        // BLUETOOTH_FIND_RADIO_PARAMS 为空结构，dwSize 填自身大小
        let params = BLUETOOTH_FIND_RADIO_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
        };
        let Ok(find) = BluetoothFindFirstRadio(&params, &mut radio) else {
            return false;
        };
        let _ = BluetoothFindRadioClose(find);
        let _ = CloseHandle(radio);
        true
    }
}

/// 枚举已配对（authenticated）且已连接的设备。不发起 inquiry（fIssueInquiry=FALSE），
/// 只做本地配对存储查询，单次成本为轻量系统调用集合（NFR-03）。
pub fn connected_paired() -> Vec<PairedDevice> {
    unsafe {
        let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnAuthenticated: windows::core::BOOL(1),
            fReturnRemembered: windows::core::BOOL(0),
            fReturnUnknown: windows::core::BOOL(0),
            fReturnConnected: windows::core::BOOL(1),
            fIssueInquiry: windows::core::BOOL(0),
            cTimeoutMultiplier: 1,
            hRadio: HANDLE::default(),
        };
        let mut info = BLUETOOTH_DEVICE_INFO {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
            ..Default::default()
        };
        // Err = 无匹配设备或蓝牙栈不可用，两种情况都返回空集（无线电有无由 radio_available 判定）
        let Ok(find) = BluetoothFindFirstDevice(&params, &mut info) else {
            return vec![];
        };
        let mut out = Vec::new();
        loop {
            if info.fConnected.as_bool() {
                let len = info
                    .szName
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(info.szName.len());
                out.push(PairedDevice {
                    name: String::from_utf16_lossy(&info.szName[..len]),
                    address: info.Address.Anonymous.ullLong,
                });
            }
            if BluetoothFindNextDevice(find, &mut info).is_err() {
                break;
            }
        }
        let _ = BluetoothFindDeviceClose(find);
        out
    }
}
