//! BTiny（蓝量）：Windows 托盘蓝牙电量工具。tray-only，无窗口无 WebView。
//! 结构对应规格 4.2/4.3：扫描全部在 tokio 后台，托盘事件循环永不阻塞。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bluetooth;
mod icon;
mod notify;
mod tray;

use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

use bluetooth::ScanOutcome;
use tray::TrayState;

const SCAN_INTERVAL: Duration = Duration::from_secs(30);
const TRAY_ID: &str = "main";

fn main() {
    // 进程级 MTA：一次递增永不释放，tokio 工作线程无需逐线程 CoInitialize 即可用 WinRT
    unsafe {
        let _ = windows::Win32::System::Com::CoIncrementMTAUsage();
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let refresh = MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)?;
            let autostart = CheckMenuItem::with_id(
                app,
                "autostart",
                "开机启动",
                true,
                app.autolaunch().is_enabled().unwrap_or(false),
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&refresh, &autostart, &quit])?;

            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon::render(tray_icon_size(), &icon::IconContent::Empty))
                .menu(&menu)
                .tooltip("BTiny")
                .on_menu_event(handle_menu_event)
                .build(app)?;

            // 30 秒轮询状态机（规格 4.3）：首轮立即执行；select 同时等待周期与手动触发
            let (tx, rx) = tokio::sync::mpsc::channel::<()>(4);
            app.manage(RefreshHandle(tx));
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                scan_loop(handle, rx).await;
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri app")
        .run(|_app, _event| {});
}

struct RefreshHandle(tokio::sync::mpsc::Sender<()>);

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "quit" => app.exit(0),
        "refresh" => {
            if let Some(h) = app.try_state::<RefreshHandle>() {
                let _ = h.0.try_send(());
            }
        }
        "autostart" => {
            // 勾选切换开机启动（FR-04）：写 HKCU Run，无需管理员
            let la = app.autolaunch();
            let enabled = la.is_enabled().unwrap_or(false);
            let result = if enabled { la.disable() } else { la.enable() };
            if let Err(e) = result {
                let _ = app
                    .notification()
                    .builder()
                    .title("BTiny")
                    .body(format!("切换开机启动失败：{e}"))
                    .show();
            }
        }
        _ => {}
    }
}

async fn scan_loop(app: AppHandle, mut rx: tokio::sync::mpsc::Receiver<()>) {
    let mut notifier = notify::LowBatteryNotifier::default();
    loop {
        let outcome = bluetooth::scan_once().await;
        debug_log(&outcome);
        update_tray(&app, &outcome);
        if let ScanOutcome::Devices(devices) = &outcome {
            let notify_app = app.clone();
            notifier.on_scan(devices, |title, body| {
                let _ = notify_app.notification().builder().title(title).body(body).show();
            });
        }
        tokio::select! {
            _ = tokio::time::sleep(SCAN_INTERVAL) => {}
            _ = rx.recv() => {}
        }
    }
}

fn update_tray(app: &AppHandle, outcome: &ScanOutcome) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    match tray::decide(outcome) {
        TrayState::RadioOff => {
            let _ = tray.set_icon(Some(icon::render(tray_icon_size(), &icon::IconContent::Unreadable)));
            let _ = tray.set_tooltip(Some("蓝牙已关闭或不可用"));
        }
        TrayState::Rendered(content, tooltip) => {
            let _ = tray.set_icon(Some(icon::render(tray_icon_size(), &content)));
            let _ = tray.set_tooltip(Some(tooltip));
        }
    }
}

/// 托盘小图标尺寸按系统 DPI 取（FR-02 图标绘制），不硬编码像素。
fn tray_icon_size() -> u32 {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON,
        ) as u32
    }
    .max(16)
}

/// 调试日志：仅在环境变量 BTINY_DEBUG 存在时把每轮扫描结果追加到 btiny-debug.log，
/// 正常运行零输出零磁盘写入（NFR-01 依赖项约束）。DoD 功能验收用。
fn debug_log(outcome: &ScanOutcome) {
    if std::env::var_os("BTINY_DEBUG").is_none() {
        return;
    }
    let line = match outcome {
        ScanOutcome::RadioUnavailable => "蓝牙不可用".to_string(),
        ScanOutcome::Devices(v) => format!(
            "{} 台: {}",
            v.len(),
            v.iter()
                .map(|d| {
                    let a = d.address;
                    format!(
                        "{} [{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}] = {:?}",
                        d.name,
                        (a >> 40) as u8,
                        (a >> 32) as u8,
                        (a >> 24) as u8,
                        (a >> 16) as u8,
                        (a >> 8) as u8,
                        a as u8,
                        d.battery
                    )
                })
                .collect::<Vec<_>>()
                .join(";  ")
        ),
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("btiny-debug.log")
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{ts}] {line}");
    }
}
