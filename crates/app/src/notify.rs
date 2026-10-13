//! Desktop notifications (Windows toasts). Elsewhere they're skipped; the
//! status line in the window still shows the message.

#[cfg(windows)]
const APP_ID: &str = "RAMOptimizer.App";

/// Registers the app's name and icon so notifications show as "RAM
/// Optimizer". Call once at startup.
pub fn init() {
    #[cfg(windows)]
    ram_core::win::register_app_id(APP_ID, "RAM Optimizer", include_bytes!("../assets/icon.png"));
}

pub fn show(title: &str, body: &str) {
    #[cfg(windows)]
    {
        let result = tauri_winrt_notification::Toast::new(APP_ID)
            .title(title)
            .text1(body)
            .show();
        if let Err(err) = result {
            crate::log_error(&format!("notification failed: {err}"));
        }
    }
    #[cfg(not(windows))]
    let _ = (title, body);
}
