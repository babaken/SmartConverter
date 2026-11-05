//! コマンド文字列の解釈とツールのパス解決（基本設計 §6.1〜6.2）。
//!
//! - Windows：先頭のトークン（プログラム名）だけを Windows の規則で取り出し、残りは加工せずに渡す。
//!   本家（CreateProcess）と同じ解釈になり、既存プリセットの引用符がそのまま動く。
//! - Linux：POSIX シェルの単語分割で argv にする（シェルは経由しない）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// 解釈したコマンド。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    /// プログラム名（引用符を外したもの）
    pub program: String,
    /// Windows：プログラム名の後ろの文字列（そのまま渡す）
    pub raw_args: String,
    /// Linux：分割した引数
    pub args: Vec<String>,
}

/// コマンド文字列を解釈する（実行中の OS の規則で）。
pub fn parse(command: &str) -> Result<ParsedCommand> {
    if cfg!(windows) {
        let (program, rest) = split_program_windows(command);
        if program.is_empty() {
            return Err(Error::CommandLine(command.to_owned()));
        }
        Ok(ParsedCommand {
            program,
            raw_args: rest.to_owned(),
            args: Vec::new(),
        })
    } else {
        let mut words = shell_words::split(command)
            .map_err(|e| Error::CommandLine(format!("{command} ({e})")))?;
        if words.is_empty() {
            return Err(Error::CommandLine(command.to_owned()));
        }
        let program = words.remove(0);
        Ok(ParsedCommand {
            program,
            raw_args: String::new(),
            args: words,
        })
    }
}

/// Windows の規則（CommandLineToArgvW の argv[0]）でプログラム名を取り出す。
///
/// 先頭が `"` なら次の `"` まで、そうでなければ空白まで。残りは先頭の空白を除いて返す。
pub fn split_program_windows(command: &str) -> (String, &str) {
    let s = command.trim_start();
    let (program, rest) = if let Some(inner) = s.strip_prefix('"') {
        match inner.find('"') {
            Some(i) => (&inner[..i], &inner[i + 1..]),
            None => (inner, ""),
        }
    } else {
        match s.find([' ', '\t']) {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        }
    };
    (program.to_owned(), rest.trim_start())
}

/// 画面やログに出すツール名（パスと拡張子を除いたもの。例 `"C:\x\ffmpeg.exe"` → `ffmpeg`）。
pub fn tool_name(program: &str) -> String {
    let file = program.rsplit(['/', '\\']).next().unwrap_or(program);
    match file.rsplit_once('.') {
        Some((stem, ext)) if is_exec_ext(ext) => stem.to_owned(),
        _ => file.to_owned(),
    }
}

fn is_exec_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe" | "com" | "bat" | "cmd"
    )
}

/// ツールのパスを探す。見つからなければ `None`。
///
/// 1. `tools`（設定の `[Tools]`。名前は大文字小文字・`.exe` の有無を問わない）
/// 2. 名前に区切り文字を含むならそのパス
/// 3. `app_dir` と `app_dir/tools`
/// 4. 環境変数 `PATH`（Windows は `PATHEXT` も考慮）
pub fn resolve_program(
    program: &str,
    tools: &BTreeMap<String, String>,
    app_dir: &Path,
    path_env: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let name = tool_name(program);
    if let Some((_, p)) = tools.iter().find(|(k, _)| {
        let k = tool_name(k);
        if cfg!(windows) {
            k.eq_ignore_ascii_case(&name)
        } else {
            k == name
        }
    }) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }

    if program.contains('/') || (cfg!(windows) && program.contains('\\')) {
        return candidates(Path::new(program))
            .into_iter()
            .find(|p| p.is_file());
    }

    let mut dirs = vec![app_dir.to_owned(), app_dir.join("tools")];
    // Windows の CreateProcess（本家）と同じく、PATH より先にシステムフォルダを探す。
    // これがないと、PATH の並び次第で 32 ビット版の cmd.exe（SysWOW64）が選ばれてしまう。
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let root = PathBuf::from(root);
            dirs.push(root.join("System32"));
            dirs.push(root.clone());
        }
    }
    if let Some(path) = path_env {
        dirs.extend(std::env::split_paths(path));
    }
    dirs.iter()
        .flat_map(|d| candidates(&d.join(program)))
        .find(|p| p.is_file())
}

fn candidates(base: &Path) -> Vec<PathBuf> {
    let mut list = vec![base.to_owned()];
    if cfg!(windows)
        && base
            .extension()
            .is_none_or(|e| !is_exec_ext(&e.to_string_lossy()))
    {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        for ext in pathext.split(';').filter(|e| !e.is_empty()) {
            let mut s = base.as_os_str().to_owned();
            s.push(ext.to_ascii_lowercase());
            list.push(PathBuf::from(s));
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_program_splitting() {
        let (p, rest) = split_program_windows(r#""QSVEncC" --avhw -i "a b.ts""#);
        assert_eq!(p, "QSVEncC");
        assert_eq!(rest, r#"--avhw -i "a b.ts""#);

        let (p, rest) = split_program_windows(r#"cmd /c "del "x*.*"""#);
        assert_eq!(p, "cmd");
        assert_eq!(rest, r#"/c "del "x*.*"""#);

        let (p, rest) = split_program_windows(r#"  "C:\Program Files\GPAC\mp4box.exe""#);
        assert_eq!(p, r"C:\Program Files\GPAC\mp4box.exe");
        assert_eq!(rest, "");
    }

    #[test]
    fn tool_names() {
        assert_eq!(tool_name("QSVEncC"), "QSVEncC");
        assert_eq!(tool_name(r"C:\x\ffmpeg.exe"), "ffmpeg");
        assert_eq!(tool_name("/usr/bin/fdkaac"), "fdkaac");
        assert_eq!(tool_name("x.264"), "x.264");
    }

    #[cfg(unix)]
    #[test]
    fn posix_parsing() {
        let c = parse(r#""ffmpeg" -i "a b.ts" -c:a aac 'out file.m4a'"#).unwrap();
        assert_eq!(c.program, "ffmpeg");
        assert_eq!(c.args, vec!["-i", "a b.ts", "-c:a", "aac", "out file.m4a"]);
        assert!(parse(r#""unterminated"#).is_err());
    }

    #[test]
    fn resolves_from_tools_and_app_dir() {
        let dir = std::env::temp_dir().join(format!("sc_cmdline_test_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tools")).unwrap();
        let exe = if cfg!(windows) {
            "mytool.exe"
        } else {
            "mytool"
        };
        std::fs::write(dir.join("tools").join(exe), b"").unwrap();

        let empty = BTreeMap::new();
        assert_eq!(
            resolve_program("mytool", &empty, &dir, None),
            Some(dir.join("tools").join(exe))
        );
        assert_eq!(
            resolve_program("missing_tool_xyz", &empty, &dir, None),
            None
        );

        let mut tools = BTreeMap::new();
        let target = dir.join("tools").join(exe);
        tools.insert("other".to_owned(), target.to_string_lossy().into_owned());
        assert_eq!(resolve_program("other", &tools, &dir, None), Some(target));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn system_folder_comes_before_path() {
        // PATH に SysWOW64 を先に入れても、System32 の cmd.exe が選ばれること
        let root = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let path = std::env::join_paths([root.join("SysWOW64"), root.join("System32")]).unwrap();
        let found = resolve_program("cmd", &BTreeMap::new(), Path::new("Z:\\none"), Some(&path));
        assert_eq!(
            found.map(|p| p.to_string_lossy().to_lowercase()),
            Some(
                root.join("System32")
                    .join("cmd.exe")
                    .to_string_lossy()
                    .to_lowercase()
            )
        );
    }
}
