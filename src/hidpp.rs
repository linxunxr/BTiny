//! 2.4G 罗技接收器电量：HID++ 协议（FR-05，V1.1 新增）。
//!
//! 通信模式与开源参考实现 cvuchener/hidpp 一致：output report（WriteFile）发请求、
//! input report（ReadFile overlapped + 超时）收响应，软件 ID 关联请求与响应。
//! 仅支持罗技 Unifying / Bolt / Lightspeed 接收器；达尔优等协议封闭品牌不枚举。

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps, HIDP_CAPS,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::bluetooth::DeviceStatus;

const REPORT_LONG: u8 = 0x11;
/// HID++ 报文长度（含 report ID）
const REPORT_LONG_LEN: usize = 20;
/// root.GetFeature(feature 0x0000, fn 0)：查 feature UUID 对应的 featureIdx
const ROOT_GET_FEATURE: u8 = 0x00;
const FEATURE_UNIFIED_BATTERY: [u8; 3] = [0x10, 0x04, 0x00];
const FEATURE_BATTERY_STATUS: [u8; 3] = [0x10, 0x00, 0x00];
const FEATURE_DEVICE_NAME: [u8; 3] = [0x00, 0x05, 0x00];
/// Unifying 5 台、Bolt 6 台配对位
const DEVICE_INDICES: std::ops::RangeInclusive<u8> = 0x01..=0x06;
const REQUEST_TIMEOUT: Duration = Duration::from_millis(900);
/// 2.4G 设备无蓝牙地址，用魔数 + 设备位合成稳定 key（仅用于低电量通知去重）
const HIDPP_ADDR_BASE: u64 = 0x4C4F4749_0000_0000;

/// 扫描全部罗技接收器上的已连接设备。任何失败静默跳过（尽力而为语义同 FR-01）。
pub fn scan_receivers() -> Vec<DeviceStatus> {
    let mut out = Vec::new();
    for channel in enumerate_channels() {
        for idx in DEVICE_INDICES {
            if let Some((name, battery)) = query_device(&channel, idx) {
                out.push(DeviceStatus {
                    name,
                    address: HIDPP_ADDR_BASE | idx as u64,
                    battery,
                });
            }
        }
    }
    out
}

struct Channel {
    handle: HANDLE,
    input_len: usize,
    output_len: usize,
    /// 软件计数器（请求-响应关联），Cell 足够：仅 spawn_blocking 单线程使用
    sw: Cell<u8>,
}

impl Drop for Channel {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.handle); };
    }
}

/// 枚举系统 HID 接口：VID 046D 且 top-level usage page 0xFF00（HID++ 通道）。
fn enumerate_channels() -> Vec<Channel> {
    unsafe {
        let hid_guid = HidD_GetHidGuid();
        let Ok(devinfo) = SetupDiGetClassDevsW(
            Some(&hid_guid),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        ) else {
            return vec![];
        };
        let mut channels = Vec::new();
        let mut index = 0u32;
        loop {
            let mut ifdata = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(devinfo, None, &hid_guid, index, &mut ifdata).is_err() {
                break;
            }
            index += 1;

            let mut needed = 0u32;
            // 探长调用返回 FALSE 属正常（ERROR_INSUFFICIENT_BUFFER），只看 needed
            let _ = SetupDiGetDeviceInterfaceDetailW(
                devinfo,
                &ifdata,
                None,
                0,
                Some(&mut needed),
                None,
            );
            if needed == 0 {
                continue;
            }
            // 缓冲区按 u16 对齐：头部是 SP_DEVICE_INTERFACE_DETAIL_DATA_W（x64 cbSize=8），后接路径
            let mut buffer = vec![0u16; (needed as usize) / 2 + 2];
            let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                devinfo,
                &ifdata,
                Some(detail),
                needed,
                None,
                None,
            )
            .is_err()
            {
                continue;
            }
            // 路径从字节偏移 4（u16 索引 2）开始，终止符也从那里起找（cbSize 字节 4-7 为 0）
            let path_len = buffer[2..]
                .iter()
                .position(|&c| c == 0)
                .map(|p| p + 2)
                .unwrap_or(buffer.len());
            let path = String::from_utf16_lossy(&buffer[2..path_len]);
            if !path.to_lowercase().contains("vid_046d") {
                continue;
            }

            // GENERIC_READ | GENERIC_WRITE
            const GENERIC_READ_WRITE: u32 = 0xC000_0000;
            let Ok(handle) = CreateFileW(
                PCWSTR(buffer[2..].as_ptr()),
                GENERIC_READ_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            ) else {
                continue; // 被罗技官方驱动等占用，跳过
            };

            let mut preparsed = Default::default();
            if !HidD_GetPreparsedData(handle, &mut preparsed) {
                let _ = CloseHandle(handle);
                continue;
            }
            let mut caps = HIDP_CAPS::default();
            let caps_ok = HidP_GetCaps(preparsed, &mut caps).is_ok();
            let _ = HidD_FreePreparsedData(preparsed);
            // HID++ 通道的 top-level collection：vendor usage page 0xFF00，usage 1（short）/2（long）
            if !caps_ok
                || caps.UsagePage != 0xFF00
                || (caps.Usage != 1 && caps.Usage != 2)
                || (caps.InputReportByteLength as usize) < REPORT_LONG_LEN
            {
                let _ = CloseHandle(handle);
                continue;
            }
            channels.push(Channel {
                handle,
                input_len: caps.InputReportByteLength as usize,
                output_len: caps.OutputReportByteLength as usize,
                sw: Cell::new(0),
            });
        }
        channels
    }
}

/// 查询单个设备位：在线则返回（设备名, 电量）。离线/深睡/不支持返回 None。
fn query_device(channel: &Channel, idx: u8) -> Option<(String, Option<u8>)> {
    // 唤醒脉冲：深睡设备对首条请求不响应（实测），发一条不等回包
    let mut wake = [0u8; 16];
    wake[..3].copy_from_slice(&FEATURE_UNIFIED_BATTERY);
    let _ = channel.request(idx, 0x00, ROOT_GET_FEATURE, &wake, Duration::from_millis(30));
    std::thread::sleep(Duration::from_millis(600));
    // 在线探测兼电量读取：直接走电量降级链，能应答即在线
    let battery = read_battery(channel, idx)?;
    let name = read_name(channel, idx).unwrap_or_else(|| format!("罗技设备 {idx}"));
    Some((name, battery))
}

fn read_battery(channel: &Channel, idx: u8) -> Option<Option<u8>> {
    // 0x1004 Unified Battery：fn1 GetStatus → params[0] = 精确百分比
    if let Some(fidx) = get_feature_index(channel, idx, FEATURE_UNIFIED_BATTERY) {
        if fidx != 0 {
            let resp = channel.request(idx, fidx, 0x01, &[0; 16], REQUEST_TIMEOUT)?;
            let level = resp[4];
            return Some(if level == 0 { None } else { Some(level.min(100)) });
        }
    }
    // 0x1000 Battery Level Status：fn0 GetStatus → params[0] = discharge_level 百分比（0=未知）
    if let Some(fidx) = get_feature_index(channel, idx, FEATURE_BATTERY_STATUS) {
        if fidx != 0 {
            let resp = channel.request(idx, fidx, 0x00, &[0; 16], REQUEST_TIMEOUT)?;
            let level = resp[4];
            return Some(if level == 0 { None } else { Some(level.min(100)) });
        }
    }
    None
}

fn read_name(channel: &Channel, idx: u8) -> Option<String> {
    let fidx = get_feature_index(channel, idx, FEATURE_DEVICE_NAME)?;
    if fidx == 0 {
        return None;
    }
    // fn1 GetNameLength → params[0] = 字节长度
    let resp = channel.request(idx, fidx, 0x01, &[0; 16], REQUEST_TIMEOUT)?;
    let len = resp[4] as usize;
    if len == 0 || len > 64 {
        return None;
    }
    // fn2 GetName(offset: u16) → params[2..] = 名字片段（每段最多 14 字节）
    let mut name = Vec::with_capacity(len);
    while name.len() < len {
        let mut params = [0u8; 16];
        let offset = name.len() as u16;
        params[0] = offset as u8;
        params[1] = (offset >> 8) as u8;
        let resp = channel.request(idx, fidx, 0x02, &params, REQUEST_TIMEOUT)?;
        let chunk = &resp[6..REPORT_LONG_LEN.min(6 + (len - name.len()))];
        if chunk.is_empty() {
            break;
        }
        name.extend_from_slice(chunk);
    }
    Some(String::from_utf8_lossy(&name[..name.len().min(len)]).into_owned())
}

/// root.GetFeature：feature UUID → featureIdx（0 = 设备不支持该 feature）。
fn get_feature_index(channel: &Channel, idx: u8, feature: [u8; 3]) -> Option<u8> {
    let mut params = [0u8; 16];
    params[..3].copy_from_slice(&feature);
    let resp = channel.request(idx, 0x00, ROOT_GET_FEATURE, &params, REQUEST_TIMEOUT)?;
    Some(resp[4])
}

impl Channel {
    /// 发一条 long report 请求并等待 sw_id 匹配的响应。
    /// 中途收到的不匹配帧（异步通知等）跳过继续等，总时长受 timeout 控制。
    fn request(
        &self,
        idx: u8,
        feature: u8,
        function: u8,
        params: &[u8; 16],
        timeout: Duration,
    ) -> Option<[u8; REPORT_LONG_LEN]> {
        unsafe {
            let sw = self.next_sw();
            let mut out = vec![0u8; self.output_len.max(REPORT_LONG_LEN)];
            out[0] = REPORT_LONG;
            out[1] = idx;
            out[2] = feature;
            out[3] = (function << 4) | sw;
            out[4..REPORT_LONG_LEN].copy_from_slice(params);

            let _event_guard = EventGuard::new()?;
            let mut ov = OVERLAPPED::default();
            ov.hEvent = _event_guard.0;
            // windows 0.62 把 ERROR_IO_PENDING 也包装成 Err——那是 overlapped 正常挂起，须当已提交
            if let Err(e) = WriteFile(self.handle, Some(&out), None, Some(&mut ov)) {
                if e.code().0 != -0x7FF8_FC1B {
                    return None;
                }
            }
            if WaitForSingleObject(ov.hEvent, 200) != WAIT_OBJECT_0 {
                let _ = CancelIoEx(self.handle, None);
                return None;
            }

            let deadline = Instant::now() + timeout;
            loop {
                let remain = deadline.saturating_duration_since(Instant::now());
                if remain.is_zero() {
                    return None;
                }
                let mut inp = vec![0u8; self.input_len];
                let guard = EventGuard::new()?;
                let mut ov2 = OVERLAPPED::default();
                ov2.hEvent = guard.0;
                if let Err(e) = ReadFile(self.handle, Some(&mut inp), None, Some(&mut ov2)) {
                    if e.code().0 != -0x7FF8_FC1B {
                        return None;
                    }
                }
                if WaitForSingleObject(ov2.hEvent, remain.as_millis().min(u32::MAX as u128) as u32)
                    != WAIT_OBJECT_0
                {
                    let _ = CancelIoEx(self.handle, Some(&mut ov2));
                    return None;
                }
                let mut read = 0u32;
                if GetOverlappedResult(self.handle, &mut ov2, &mut read, false).is_err() {
                    return None;
                }
                if read < 4 {
                    continue; // 残帧，继续等
                }
                // HID++ 1.0 错误帧 featureIdx 位置为 0x8F；2.0 响应按 sw 匹配
                if inp[2] == 0x8F {
                    return None;
                }
                if inp[0] == REPORT_LONG && inp[1] == idx && (inp[3] & 0x0F) == sw {
                    let mut resp = [0u8; REPORT_LONG_LEN];
                    resp.copy_from_slice(&inp[..REPORT_LONG_LEN]);
                    return Some(resp);
                }
                // 不匹配（广播/通知帧）：丢弃继续等
            }
        }
    }

    fn next_sw(&self) -> u8 {
        let next = self.sw.get() % 15 + 1; // 1..=15 循环，0 保留
        self.sw.set(next);
        next
    }
}

/// OVERLAPPED 不拥有 event，Drop 不会关句柄，需要显式 guard 防 caller 侧泄漏。
struct EventGuard(HANDLE);

impl EventGuard {
    unsafe fn new() -> Option<Self> {
        CreateEventW(None, false, false, None).ok().map(EventGuard)
    }
}

impl Drop for EventGuard {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); };
    }
}
