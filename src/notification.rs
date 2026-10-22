//! Best-effort native notifications; never block the UI on the desktop service.

pub fn show(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            static INIT: std::sync::Once = std::sync::Once::new();
            INIT.call_once(|| {
                if let Err(err) = notify_rust::set_application("com.smartvoices.rust") {
                    eprintln!("Warning: Couldn't identify the notification app: {err}");
                }
            });
        }
        if let Err(err) = notify_rust::Notification::new()
            .appname("Smart Voices")
            .summary(&summary)
            .body(&body)
            .show()
        {
            eprintln!("Warning: Couldn't show a desktop notification: {err}");
        }
    });
}
