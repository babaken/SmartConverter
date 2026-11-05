//! パスの補助関数。

use std::path::{Path, PathBuf, MAIN_SEPARATOR};

/// 絶対パスにする（存在しなくてもよい。`..` などは残る）。
pub fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

/// ドライブのルート（`D:\`、`\\server\share\`、`/`）かどうか（要件 F-QU-08）。
pub fn is_root(path: &Path) -> bool {
    absolute(path).parent().is_none()
}

/// 末尾に区切り文字を付けた文字列にする（`<%InputFilePath%>`、`<%OutputPath%>` 用）。
pub fn with_trailing_separator(path: &Path) -> String {
    let mut s = path.to_string_lossy().into_owned();
    if !s.ends_with(MAIN_SEPARATOR) && !(cfg!(windows) && s.ends_with('/')) {
        s.push(MAIN_SEPARATOR);
    }
    s
}

/// 末尾の区切り文字を取り除いた文字列にする（`<%AppPath%>` 用）。ルートはそのまま。
pub fn without_trailing_separator(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if is_root(path) {
        return s;
    }
    s.trim_end_matches(MAIN_SEPARATOR).to_owned()
}

/// ファイルを書き換える。同じフォルダの一時ファイルに書いてから名前を変えるので、
/// 途中で落ちても元のファイルが半端な内容にならない。
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = std::fs::write(&tmp, contents).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_file() {
        let dir = std::env::temp_dir().join(format!("sc_paths_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.ini");
        write_atomic(&f, "one").unwrap();
        write_atomic(&f, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_roots() {
        assert!(is_root(Path::new(r"D:\")));
        assert!(is_root(Path::new(r"\\server\share\")));
        assert!(!is_root(Path::new(r"D:\out")));
        assert_eq!(with_trailing_separator(Path::new(r"D:\out")), r"D:\out\");
        assert_eq!(with_trailing_separator(Path::new(r"D:\out\")), r"D:\out\");
        assert_eq!(without_trailing_separator(Path::new(r"D:\app\")), r"D:\app");
    }

    #[cfg(unix)]
    #[test]
    fn unix_roots() {
        assert!(is_root(Path::new("/")));
        assert!(!is_root(Path::new("/tmp")));
        assert_eq!(with_trailing_separator(Path::new("/tmp")), "/tmp/");
        assert_eq!(
            without_trailing_separator(Path::new("/opt/app/")),
            "/opt/app"
        );
    }
}
