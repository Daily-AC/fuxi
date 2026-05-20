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

/// 玄女眼睛 screen 拍帧——绕开 Tauri 2 macOS WKWebView 的 getDisplayMedia
/// 限制（实测在 release 包里抛 generic 错且不弹系统权限弹窗），改用 macOS
/// 内置 `screencapture` 命令。这条命令首次调用时会触发标准的「屏幕录制」
/// TCC 弹窗，归属到 XuannvPet bundle，用户授权一次后永久生效。
///
/// `-x` 静默（不响快门音），`-t png` 输出 PNG，写到 /tmp 一个 uuid 命名的临时
/// 文件，读出 bytes 后立删，不在磁盘留垃圾。失败时把 stderr 一起抛回 JS 让
/// PetCanvas 统一走 `capture_failed` 路径——但通常用户拒绝是 stderr "user
/// cancelled"，未授权是 stderr "could not create image from display"。
#[tauri::command]
fn pet_capture_screen_png() -> Result<Vec<u8>, String> {
    let tmp = std::env::temp_dir().join(format!("xuannv-pet-screen-{}.png", uuid::Uuid::new_v4()));
    let out = std::process::Command::new("/usr/sbin/screencapture")
        .arg("-x")
        .arg("-t")
        .arg("png")
        .arg(&tmp)
        .output()
        .map_err(|e| format!("spawn screencapture 失败：{e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "screencapture exit={:?} stderr={stderr}",
            out.status.code()
        ));
    }
    let bytes = std::fs::read(&tmp).map_err(|e| format!("读 {} 失败：{e}", tmp.display()))?;
    let _ = std::fs::remove_file(&tmp);
    if bytes.is_empty() {
        return Err("screencapture 写出空文件——多半 TCC 拒了但没非零退出".into());
    }
    Ok(bytes)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_os::init());

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .invoke_handler(tauri::generate_handler![pet_log, pet_capture_screen_png])
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
