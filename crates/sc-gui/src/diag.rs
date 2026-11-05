//! 操作ログ：起動・終了・操作・ジョブの開始と終了・パニックを `Log/SmartConverter.log` に残す（基本設計 §10）。
//!
//! 画面が急に消えたときに、直前に何をしていたか、パニックしたのかを後から追えるようにする。
//! リリースビルドの Windows 版はコンソールを持たないので、標準のパニック表示（標準エラー出力）は
//! どこにも残らないため、パニックの内容もここに書く。最後の行が「終了」でなければ、異常終了している。
//!
//! 置き場所は設定ファイルと同じフォルダの `Log/`（Windows では、実行ファイルのフォルダに
//! 書き込めればその隣、書き込めなければ `%APPDATA%\SmartConverter\Log\`）。
//! 入力ファイルの名前やパスは書かない（ジョブの番号とプリセット名だけ）。

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// これより大きくなったら `SmartConverter.log.old` に移して、新しく書き始める。
const MAX_LEN: u64 = 1024 * 1024;

static LOG: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 操作ログの場所を決めて、パニックのフックを入れる。標準のパニック表示もそのまま行う。
pub fn init(settings_path: &Path) {
    let path = log_path(settings_path);
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(&panic_report(info));
        default_hook(info);
    }));
}

/// 操作を 1 行書く。書けなくても何もしない。
pub fn op(text: impl AsRef<str>) {
    write(&format!("{} {}\n", now(), text.as_ref()));
}

/// 操作ログの場所（設定ファイルと同じフォルダの `Log/`）。
pub fn log_path(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("Log")
        .join("SmartConverter.log")
}

fn now() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

fn write(text: &str) {
    // パニックの中でも書けるよう、毒された Mutex もそのまま使う。
    let path = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = path.as_ref() {
        let _ = append(path, text);
    }
}

fn panic_report(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "（内容なし）".into());
    let location = info
        .location()
        .map_or_else(|| "（不明）".into(), |l| l.to_string());
    format!(
        "{} パニック: スレッド {} / 場所 {location} / 内容 {message}\nバックトレース:\n{}\n",
        now(),
        thread.name().unwrap_or("（名前なし）"),
        std::backtrace::Backtrace::force_capture(),
    )
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LEN) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    let mut f = File::options().create(true).append(true).open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_next_to_settings() {
        let p = log_path(Path::new("/conf/SmartConverter.ini"));
        assert_eq!(p, Path::new("/conf/Log/SmartConverter.log"));
    }

    #[test]
    fn appends_and_rotates() {
        let dir = std::env::temp_dir().join(format!("sc_diag_test_{}", std::process::id()));
        let path = dir.join("Log").join("SmartConverter.log");
        append(&path, "one\n").unwrap();
        append(&path, "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");

        // 大きくなったら .old に移して新しく書き始める
        std::fs::write(&path, vec![b'x'; (MAX_LEN + 1) as usize]).unwrap();
        append(&path, "three\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "three\n");
        assert!(path.with_extension("log.old").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn records_operations_and_panics() {
        let dir = std::env::temp_dir().join(format!("sc_diag_hook_{}", std::process::id()));
        let settings = dir.join("SmartConverter.ini");
        let saved = std::panic::take_hook();
        init(&settings);
        op("テスト用の操作");
        let r = std::thread::Builder::new()
            .name("diag-test".into())
            .spawn(|| panic!("テスト用のパニック"))
            .unwrap()
            .join();
        std::panic::set_hook(saved);
        *LOG.lock().unwrap_or_else(|e| e.into_inner()) = None;
        assert!(r.is_err());
        let text = std::fs::read_to_string(log_path(&settings)).unwrap();
        assert!(text.contains("テスト用の操作"), "{text}");
        assert!(text.contains("スレッド diag-test"), "{text}");
        assert!(text.contains("内容 テスト用のパニック"), "{text}");
        assert!(text.contains("diag.rs"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
