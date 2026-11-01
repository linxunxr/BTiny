//! 经典蓝牙电量：SetupAPI 读系统填充的 DEVPKEY_Device_BatteryLevel（FR-01 尽力而为路径）。
//!
//! 该属性由 Windows 蓝牙栈解析厂商上报后填充（Windows 设置页电量同源），
//! 只做一次属性查询，无任何主动协议探测与重试。

use std::collections::HashMap;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDevicePropertyW, DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA,
};
use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BYTE, DEVPROP_TYPE_UINT64, DEVPROPTYPE};
use windows::Win32::Foundation::DEVPROPKEY;

// DEVPKEY_Device_BatteryLevel 未收录于公开 devpkey.h，值经社区多项目交叉验证
// （AutomatedLab/DeviceManagement、joric/bluetooth-battery-monitor），规格 4.2 有记录。
const DEVPKEY_DEVICE_BATTERY_LEVEL: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};

// DEVPKEY_DeviceAddress（标准键）：经典蓝牙 devnode 上为 UINT64 的 BTH 地址；
// LE devnode 为字符串类型，类型校验不过自然跳过——正好只服务经典路径。
const DEVPKEY_DEVICE_ADDRESS: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 30,
};

// GUID_DEVCLASS_BLUETOOTH（devguid.h）
const GUID_DEVCLASS_BLUETOOTH: GUID = GUID::from_u128(0xe0cbf06c_cd8b_4647_bb8a_263b43f0f974);

/// 扫一遍蓝牙设备类 devnode，返回 地址 → 电量（只含能读到电量的设备）。
/// 一次枚举摊给本轮全部设备使用（调用方 bluetooth::scan_once）。
pub fn battery_map() -> HashMap<u64, Option<u8>> {
    let mut map = HashMap::new();
    unsafe {
        let class = GUID_DEVCLASS_BLUETOOTH;
        let Ok(devinfo) =
            SetupDiGetClassDevsW(Some(&class), PCWSTR::null(), None, DIGCF_PRESENT)
        else {
            return map;
        };
        let mut index: u32 = 0;
        loop {
            let mut data = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(devinfo, index, &mut data).is_err() {
                break;
            }
            index += 1;

            let address = match read_u64(devinfo, &data, &DEVPKEY_DEVICE_ADDRESS) {
                Some(v) => v,
                None => continue, // 类型不符（LE 字符串地址）或读不到：非经典 devnode
            };
            if let Some(level) = read_byte(devinfo, &data, &DEVPKEY_DEVICE_BATTERY_LEVEL) {
                map.insert(address, Some(level));
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(devinfo);
    }
    map
}

unsafe fn read_u64(
    devinfo: HDEVINFO,
    data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<u64> {
    let mut value = 0u64;
    let mut prop_type = DEVPROPTYPE::default();
    let mut required = 0u32;
    let buf = std::slice::from_raw_parts_mut(&mut value as *mut u64 as *mut u8, 8);
    let ok = SetupDiGetDevicePropertyW(
        devinfo,
        data,
        key,
        &mut prop_type,
        Some(buf),
        Some(&mut required),
        0,
    )
    .is_ok();
    (ok && prop_type == DEVPROP_TYPE_UINT64).then_some(value)
}

unsafe fn read_byte(
    devinfo: HDEVINFO,
    data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<u8> {
    let mut value = 0u8;
    let mut prop_type = DEVPROPTYPE::default();
    let mut required = 0u32;
    let buf = std::slice::from_raw_parts_mut(&mut value as *mut u8, 1);
    let ok = SetupDiGetDevicePropertyW(
        devinfo,
        data,
        key,
        &mut prop_type,
        Some(buf),
        Some(&mut required),
        0,
    )
    .is_ok();
    (ok && prop_type == DEVPROP_TYPE_BYTE).then_some(value)
}
