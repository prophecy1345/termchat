//! The unread count where the system shows it: on the Dock icon on macOS,
//! in the window title elsewhere. A number only, never a name. And the
//! system notification, always "New message" (ADR-008).

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

/// Asks macOS once for permission to notify, so the first message already
/// comes with the app's own icon. Nothing to ask elsewhere.
#[cfg(target_os = "macos")]
pub fn allow() {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};
    static ASKED: std::sync::Once = std::sync::Once::new();
    ASKED.call_once(|| {
        if !bundled() {
            return;
        }
        let done = RcBlock::new(|granted: Bool, _: *mut NSError| {
            GRANTED.store(granted.as_bool(), Ordering::Relaxed);
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert,
                &done,
            );
    });
}

#[cfg(not(target_os = "macos"))]
pub fn allow() {}

/// "New message" with termchat.app's icon. osascript would show Script
/// Editor's, so it is only the fallback: outside the bundle or without
/// permission.
#[cfg(target_os = "macos")]
pub fn alert() {
    use objc2_foundation::NSString;
    use objc2_user_notifications::{
        UNMutableNotificationContent, UNNotificationRequest, UNUserNotificationCenter,
    };
    if !bundled() || !GRANTED.load(Ordering::Relaxed) {
        termchat_engine::notify::system();
        return;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str("termchat.app"));
    content.setBody(&NSString::from_str("New message"));
    // One id: a new notice replaces the last one instead of piling up.
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str("new-message"),
        &content,
        None,
    );
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, None);
}

#[cfg(not(target_os = "macos"))]
pub fn alert() {
    termchat_engine::notify::system();
}

#[cfg(target_os = "macos")]
static GRANTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;

/// Run from termchat.app, not a bare binary: without a bundle id the
/// notification center raises an Objective-C exception.
#[cfg(target_os = "macos")]
fn bundled() -> bool {
    objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some()
}
