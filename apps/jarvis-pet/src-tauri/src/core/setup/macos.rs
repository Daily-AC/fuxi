use tauri::{App, Manager};
use tauri_nspanel::WebviewWindowExt;
use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior;

// NSPanel-only style mask（cocoa crate 的 NSWindowStyleMask 不含 NSNonactivatingPanelMask）
// AppKit 原值：NSBorderlessWindowMask = 0, NSNonactivatingPanelMask = 1 << 7
const NS_BORDERLESS_WINDOW_MASK: i32 = 0;
const NS_NONACTIVATING_PANEL_MASK: i32 = 1 << 7;

pub fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    app.set_dock_visibility(false);

    let main = app
        .get_webview_window("main")
        .ok_or("找不到 main window —— tauri.conf.json 里 windows 数组缺")?;
    let panel = main.to_panel()?;

    panel.set_level(7);
    panel.set_style_mask(NS_BORDERLESS_WINDOW_MASK | NS_NONACTIVATING_PANEL_MASK);

    // 透明 / 无阴影：to_panel() 完成 class swizzle 后，必须显式 set_opaque(false)，
    // 否则 NSPanel 视 opaque 标志为 true，Tauri conf.json 里 transparent:true 设的
    // NSColor.clearColor backgroundColor 被覆盖成系统默认白底——T16 白底 bug 的根因。
    // 同步关 shadow，避免透明窗外的 macOS 默认投影矩形。
    panel.set_opaque(false);
    panel.set_has_shadow(false);

    let mut behaviour = NSWindowCollectionBehavior::empty();
    behaviour.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces);
    behaviour.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary);
    behaviour.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorIgnoresCycle);
    panel.set_collection_behaviour(behaviour);

    panel.set_floating_panel(true);

    Ok(())
}
