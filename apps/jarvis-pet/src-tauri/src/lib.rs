mod core;

use std::io::Write;

/// 前端 `console.log` 是 webview 进程内的，主进程 stdout / mac log stream
/// 看不到。dev 用 Safari Inspector 可以，release 没。所以加一个 IPC 命令把
/// 前端日志 append 到 `/tmp/jarvis-pet.log`，调试时 `tail -f` 直接看。
#[tauri::command]
fn pet_log(msg: String) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/jarvis-pet.log")
    {
        let now = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(f, "[{now}] {msg}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_os::init());

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .invoke_handler(tauri::generate_handler![pet_log])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                core::setup::macos::setup(app)?;
            }
            let _ = app;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动 jarvis-pet 失败");
}
