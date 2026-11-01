// Spike 空壳：验证 Tauri 2 tray-only 的体积/内存基线与 toast 可用性。
// 无窗口、无 WebView；托盘菜单仅两项（测试通知 / 退出）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let notify = MenuItem::with_id(app, "notify", "测试通知", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&notify, &quit])?;

            let _tray = TrayIconBuilder::with_id("main")
                .icon(spike_icon())
                .menu(&menu)
                .tooltip("BTiny spike")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "notify" => {
                        fire_toast(app);
                    }
                    _ => {}
                })
                .build(app)?;

            // 启动 3 秒后自动发一次 toast 并把 API 结果落盘，供无人工参与判定
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                fire_toast(&handle);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri app")
        .run(|_app, _event| {});
}

fn fire_toast(app: &tauri::AppHandle) {
    use tauri_plugin_notification::NotificationExt;
    let result = app
        .notification()
        .builder()
        .title("BTiny spike")
        .body("toast 可用：绿色单 exe 下 AUMID 正常")
        .show();
    let report = match &result {
        Ok(()) => "toast API: Ok（视觉弹出需人工确认屏幕）".to_string(),
        Err(e) => format!("toast API: Err({e})"),
    };
    let _ = std::fs::write("spike-toast-result.txt", &report);
    if result.is_err() {
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(report));
        }
    }
}

/// 程序生成 32x32 RGBA 图标，不引入图像解码依赖（正式版将替换为点阵字体数字）。
fn spike_icon() -> tauri::image::Image<'static> {
    let (w, h) = (32u32, 32u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _y in 0..h {
        for x in 0..w {
            let v = (x * 8) as u8;
            rgba.extend_from_slice(&[v, 120, 220, 255]);
        }
    }
    tauri::image::Image::new_owned(rgba, w, h)
}
