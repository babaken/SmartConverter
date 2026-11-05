//! ファイルマネージャでフォルダを開く。

use std::path::Path;
use std::process::Command;

/// フォルダを開く。`select` を渡すと、そのファイルを選んだ状態で開く（Windows のみ）。
pub fn open_folder(dir: &Path, select: Option<&Path>) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("explorer");
        match select.filter(|p| p.exists()) {
            Some(file) => {
                cmd.raw_arg(format!("/select,\"{}\"", file.display()));
            }
            None => {
                cmd.arg(dir);
            }
        }
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = select;
        let _ = Command::new("xdg-open").arg(dir).spawn();
    }
}
