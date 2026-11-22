//! The unread count where the system shows it: on the Dock icon on macOS,
//! in the window title elsewhere. A number only, never a name.

use eframe::egui;

#[cfg(target_os = "macos")]
pub fn show(_ctx: &egui::Context, unread: u64) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    // egui calls us on the main thread; elsewhere there is nothing to do.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let label = (unread > 0).then(|| NSString::from_str(&unread.min(999).to_string()));
    NSApplication::sharedApplication(mtm)
        .dockTile()
        .setBadgeLabel(label.as_deref());
}

#[cfg(not(target_os = "macos"))]
pub fn show(ctx: &egui::Context, unread: u64) {
    let title = match unread {
        0 => "termchat.app".to_string(),
        n => format!("termchat.app ({})", n.min(999)),
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
}
