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

    let mut behaviour = NSWindowCollectionBehavior::empty();
    behaviour.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces);
    behaviour.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary);
    behaviour.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorIgnoresCycle);
    panel.set_collection_behaviour(behaviour);

    panel.set_floating_panel(true);

    Ok(())
}
