mod core;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
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
