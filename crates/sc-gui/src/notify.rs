//! OS の通知（画面設計 N-03〜N-05）。表示に時間がかかることがあるので別スレッドで出す。

pub fn show(summary: &str, body: &str) {
    let summary = summary.to_owned();
    let body = body.to_owned();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname(sc_core::APP_NAME)
            .summary(&summary)
            .body(&body)
            .show();
    });
}
