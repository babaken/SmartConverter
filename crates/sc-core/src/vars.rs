//! 置換変数の展開（要件 §4.3）。
//!
//! 変数名は大文字小文字を区別しない。知らない変数は置き換えずに文字のまま残し、
//! 呼び出し側に知らせる（本家と同じ動作。ログと画面で警告する）。

use std::path::Path;

use chrono::NaiveDateTime;

use crate::naming;
use crate::paths;

/// 置換変数の名前（本家の 10 種類）。
pub const NAMES: [&str; 10] = [
    "InputFile",
    "InputFilePath",
    "InputFileBase",
    "InputFileExt",
    "OutputFile",
    "OutputPath",
    "Title",
    "TemporaryFile",
    "AppPath",
    "TimeStamp",
];

/// 置換変数の値。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vars {
    pub input_file: String,
    pub input_file_path: String,
    pub input_file_base: String,
    pub input_file_ext: String,
    pub output_file: String,
    pub output_path: String,
    pub title: String,
    pub temporary_file: String,
    pub app_path: String,
    pub timestamp: String,
}

impl Vars {
    /// 各パスと日時から値を作る。
    ///
    /// - `input`：入力ファイル（絶対パスにして使う）
    /// - `output_dir`：出力先フォルダ
    /// - `title`：タイトル（タイムスタンプ・連番を反映済み）
    /// - `temporary_base`：一時ファイル名（拡張子なし）
    /// - `app_dir`：実行ファイルのフォルダ
    /// - `mtime`：入力ファイルの更新日時（ローカル時刻）
    pub fn new(
        input: &Path,
        output_dir: &Path,
        title: &str,
        temporary_base: &Path,
        app_dir: &Path,
        mtime: &NaiveDateTime,
    ) -> Self {
        let input = paths::absolute(input);
        let output_dir = paths::absolute(output_dir);
        let input_dir = input.parent().unwrap_or(Path::new(""));
        let output_path = paths::with_trailing_separator(&output_dir);
        Self {
            input_file: input.to_string_lossy().into_owned(),
            input_file_path: paths::with_trailing_separator(input_dir),
            input_file_base: input
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            input_file_ext: input
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
            output_file: format!("{output_path}{title}"),
            output_path,
            title: title.to_owned(),
            temporary_file: temporary_base.to_string_lossy().into_owned(),
            app_path: paths::without_trailing_separator(app_dir),
            timestamp: naming::timestamp_value(mtime),
        }
    }

    /// 名前（大文字小文字を区別しない）から値を返す。
    pub fn get(&self, name: &str) -> Option<&str> {
        let v = match name.to_ascii_lowercase().as_str() {
            "inputfile" => &self.input_file,
            "inputfilepath" => &self.input_file_path,
            "inputfilebase" => &self.input_file_base,
            "inputfileext" => &self.input_file_ext,
            "outputfile" => &self.output_file,
            "outputpath" => &self.output_path,
            "title" => &self.title,
            "temporaryfile" => &self.temporary_file,
            "apppath" => &self.app_path,
            "timestamp" => &self.timestamp,
            _ => return None,
        };
        Some(v)
    }
}

/// 展開結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// 未定義の変数（`<%…%>` の形のまま、出てきた順・重複なし）
    pub unknown: Vec<String>,
}

/// `<%Name%>` を値に置き換える。
pub fn expand(template: &str, vars: &Vars) -> Expanded {
    let mut text = String::with_capacity(template.len());
    let unknown = walk(template, vars, |piece| match piece {
        Piece::Literal(s) | Piece::Value(s) => text.push_str(s),
    });
    Expanded { text, unknown }
}

/// POSIX シェルの単語分割（Linux のコマンド）用に、値を引用符の状態に合わせてエスケープして置き換える。
///
/// ファイル名に `"`・`'`・`\`・空白などが含まれていても、値はそのまま 1 つの引数の一部になり、
/// 引数が増えたり引用符が閉じたりしない（悪意のあるファイル名で引数を差し込まれないようにする）。
/// 引用符で囲んでいない値は `'…'` で囲む。
pub fn expand_posix(template: &str, vars: &Vars) -> Expanded {
    let mut text = String::with_capacity(template.len());
    let mut state = Quote::None;
    let unknown = walk(template, vars, |piece| match piece {
        Piece::Literal(s) => {
            text.push_str(s);
            state = state.after(s);
        }
        Piece::Value(v) => {
            // `\<%Var%>` の `\` は値の先頭をエスケープしてしまうので落とす。
            if matches!(state, Quote::Escape | Quote::DoubleEscape) {
                text.pop();
                state = if state == Quote::Escape {
                    Quote::None
                } else {
                    Quote::Double
                };
            }
            match state {
                Quote::None => {
                    text.push('\'');
                    text.push_str(&v.replace('\'', r"'\''"));
                    text.push('\'');
                }
                Quote::Single => text.push_str(&v.replace('\'', r"'\''")),
                _ => {
                    for c in v.chars() {
                        if matches!(c, '"' | '\\' | '$' | '`') {
                            text.push('\\');
                        }
                        text.push(c);
                    }
                }
            }
        }
    });
    Expanded { text, unknown }
}

/// POSIX シェル（shell-words）の引用符の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    /// 引用符の外で `\` の直後
    Escape,
    Single,
    Double,
    /// `"…"` の中で `\` の直後
    DoubleEscape,
}

impl Quote {
    fn after(mut self, literal: &str) -> Self {
        for c in literal.chars() {
            self = match (self, c) {
                (Quote::None, '\\') => Quote::Escape,
                (Quote::None, '\'') => Quote::Single,
                (Quote::None, '"') => Quote::Double,
                (Quote::None | Quote::Escape, _) => Quote::None,
                (Quote::Single, '\'') => Quote::None,
                (Quote::Single, _) => Quote::Single,
                (Quote::Double, '\\') => Quote::DoubleEscape,
                (Quote::Double, '"') => Quote::None,
                (Quote::Double | Quote::DoubleEscape, _) => Quote::Double,
            };
        }
        self
    }
}

enum Piece<'a> {
    /// テンプレートの文字どおりの部分（未定義の変数を含む）
    Literal(&'a str),
    /// 変数の値
    Value(&'a str),
}

/// テンプレートを先頭から読み、文字どおりの部分と変数の値を順に `f` に渡す。未定義の変数を返す。
fn walk<'a>(template: &'a str, vars: &'a Vars, mut f: impl FnMut(Piece<'a>)) -> Vec<String> {
    let mut unknown: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("<%") {
        f(Piece::Literal(&rest[..start]));
        let after = &rest[start + 2..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        if !name.is_empty() && after[name_len..].starts_with("%>") {
            let whole = &rest[start..start + 2 + name_len + 2];
            match vars.get(name) {
                Some(value) => f(Piece::Value(value)),
                None => {
                    f(Piece::Literal(whole));
                    if !unknown.iter().any(|u| u == whole) {
                        unknown.push(whole.to_owned());
                    }
                }
            }
            rest = &rest[start + whole.len()..];
        } else {
            f(Piece::Literal("<%"));
            rest = after;
        }
    }
    f(Piece::Literal(rest));
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vars {
        Vars {
            input_file: "IN".into(),
            title: "T".into(),
            output_file: "OUT".into(),
            ..Default::default()
        }
    }

    #[test]
    fn expands_known_variables_case_insensitively() {
        let e = expand("a <%InputFile%> <%inputfile%> <%TITLE%>.mp4", &sample());
        assert_eq!(e.text, "a IN IN T.mp4");
        assert!(e.unknown.is_empty());
    }

    #[test]
    fn keeps_unknown_variables() {
        let e = expand("<%OutPutFiel%>.mp4 <%OutPutFiel%> <%Foo%>", &sample());
        assert_eq!(e.text, "<%OutPutFiel%>.mp4 <%OutPutFiel%> <%Foo%>");
        assert_eq!(e.unknown, vec!["<%OutPutFiel%>", "<%Foo%>"]);
    }

    #[test]
    fn leaves_malformed_markers() {
        let e = expand("<% x %> <%%> <%InputFile <%InputFile%>", &sample());
        assert_eq!(e.text, "<% x %> <%%> <%InputFile IN");
        assert!(e.unknown.is_empty());
    }

    /// 悪意のあるファイル名でも、Linux のコマンドの引数が増えたり変わったりしない。
    #[test]
    fn posix_expansion_keeps_values_in_one_argument() {
        let nasty = [
            "a b.ts",
            r#"x" -y -f null "/tmp/evil.ts"#,
            "it's '; rm -rf ~ '.ts",
            r"back\slash\.ts",
            "$(touch pwned) `id` $HOME.ts",
            "line\nbreak.ts",
            "",
        ];
        let templates = [
            (r#"ffmpeg -i "<%InputFile%>" "<%OutputFile%>.mp4""#, 0),
            ("ffmpeg -i '<%InputFile%>' '<%OutputFile%>.mp4'", 0),
            ("ffmpeg -i <%InputFile%> <%OutputFile%>.mp4", 0),
            (r#"ffmpeg -i pre\<%InputFile%> "x\<%OutputFile%>.mp4""#, 1),
        ];
        for value in nasty {
            let vars = Vars {
                input_file: value.into(),
                output_file: format!("/out/{value}"),
                ..Default::default()
            };
            for (t, kind) in templates {
                let e = expand_posix(t, &vars);
                let words = shell_words::split(&e.text)
                    .unwrap_or_else(|err| panic!("{t} / {value:?}: {err} ({})", e.text));
                let (want_in, want_out) = if kind == 0 {
                    (value.to_owned(), format!("/out/{value}.mp4"))
                } else {
                    (format!("pre{value}"), format!("x/out/{value}.mp4"))
                };
                assert_eq!(
                    words,
                    vec!["ffmpeg".to_owned(), "-i".into(), want_in, want_out],
                    "{t} / {value:?} → {}",
                    e.text
                );
            }
        }
    }

    #[test]
    fn posix_expansion_leaves_plain_values_readable() {
        let e = expand_posix(r#"ffmpeg -i "<%InputFile%>""#, &sample());
        assert_eq!(e.text, r#"ffmpeg -i "IN""#);
        let e = expand_posix("x <%Foo%>", &sample());
        assert_eq!(e.text, "x <%Foo%>");
        assert_eq!(e.unknown, vec!["<%Foo%>"]);
    }

    #[test]
    fn every_name_is_known() {
        let v = sample();
        for n in NAMES {
            assert!(v.get(n).is_some(), "{n}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn builds_values_like_original() {
        // docs/original-behavior.md §1 の展開例と同じ形になること
        let mtime = chrono::NaiveDate::from_ymd_opt(2024, 3, 5)
            .unwrap()
            .and_hms_opt(7, 8, 9)
            .unwrap();
        let v = Vars::new(
            Path::new(r"C:\work\in\テスト 動画.ts"),
            Path::new(r"C:\work\out"),
            "[03-05] テスト 動画",
            Path::new(r"C:\Temp\sc_1_1_abcdef"),
            Path::new(r"C:\app\"),
            &mtime,
        );
        assert_eq!(v.input_file, r"C:\work\in\テスト 動画.ts");
        assert_eq!(v.input_file_path, r"C:\work\in\");
        assert_eq!(v.input_file_base, "テスト 動画");
        assert_eq!(v.input_file_ext, ".ts");
        assert_eq!(v.output_file, r"C:\work\out\[03-05] テスト 動画");
        assert_eq!(v.output_path, r"C:\work\out\");
        assert_eq!(v.title, "[03-05] テスト 動画");
        assert_eq!(v.temporary_file, r"C:\Temp\sc_1_1_abcdef");
        assert_eq!(v.app_path, r"C:\app");
        assert_eq!(v.timestamp, "2024-03-05 07:08:09");
    }
}
