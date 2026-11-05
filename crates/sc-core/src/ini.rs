//! 行をそのまま保持する INI パーサ・ライタ。
//!
//! 本家（Delphi の TIniFile ＝ Windows の GetPrivateProfileString）と同じ読み方をする。
//! 本家での確認結果は `docs/original-behavior.md` を参照。
//!
//! - セクション名・キー名は大文字小文字を区別しない
//! - 同じ名前のセクションが複数あるときは、最初のセクションだけを読む
//! - 同じセクションに同じキーが複数あるときは、最初のものを使う
//! - キーと値の前後の空白は取り除く
//! - 値の全体が `"…"` または `'…'` で囲まれていれば、外側の引用符を外す
//!
//! 書き戻すときは、変更していない行を 1 文字も変えずに出す（コメントや知らないセクションも残る）。

/// INI の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
enum LineKind {
    /// `[名前]`
    Section(String),
    /// `キー=値`（キーと値は前後の空白を除いたもの。値の引用符は外していない）
    Entry { key: String, value: String },
    /// 空行・コメント・解釈できない行
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    kind: LineKind,
    /// 行の内容（改行は含まない）
    text: String,
    /// 行末の改行（`\r\n`、`\n`、ファイル末尾なら空）
    eol: String,
}

/// 読み込み時の注意（解釈できなかった行など）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniWarning {
    /// 1 始まりの行番号
    pub line: usize,
    pub message: String,
}

/// 行を保持する INI ドキュメント。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IniDocument {
    lines: Vec<Line>,
}

impl IniDocument {
    /// 文字列を解析する（失敗はしない。解釈できない行は [`IniDocument::warnings`] で分かる）。
    pub fn parse(text: &str) -> Self {
        let mut lines = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let (content, eol, next) = match rest.find('\n') {
                Some(i) => {
                    let (content, eol) = if i > 0 && rest.as_bytes()[i - 1] == b'\r' {
                        (&rest[..i - 1], "\r\n")
                    } else {
                        (&rest[..i], "\n")
                    };
                    (content, eol, &rest[i + 1..])
                }
                None => (rest, "", ""),
            };
            lines.push(Line {
                kind: classify(content),
                text: content.to_owned(),
                eol: eol.to_owned(),
            });
            rest = next;
        }
        Self { lines }
    }

    /// 読み込んだときと同じ形で文字列に戻す（変更した行だけが変わる）。
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(&line.text);
            out.push_str(&line.eol);
        }
        out
    }

    /// 解釈できなかった行の一覧。
    pub fn warnings(&self) -> Vec<IniWarning> {
        let mut current_section = false;
        let mut warnings = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            match &line.kind {
                LineKind::Section(_) => current_section = true,
                LineKind::Entry { .. } if !current_section => warnings.push(IniWarning {
                    line: i + 1,
                    message: "セクションの外にあるキーは無視されます".into(),
                }),
                LineKind::Other => {
                    let t = line.text.trim();
                    if !(t.is_empty() || t.starts_with(';') || t.starts_with('#')) {
                        warnings.push(IniWarning {
                            line: i + 1,
                            message: format!("解釈できない行です: {t}"),
                        });
                    }
                }
                _ => {}
            }
        }
        warnings
    }

    /// セクション名を出てきた順にすべて返す（重複もそのまま）。
    pub fn section_names(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|l| match &l.kind {
                LineKind::Section(name) => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// セクションがあるか（大文字小文字を区別しない）。
    pub fn has_section(&self, section: &str) -> bool {
        self.section_range(section).is_some()
    }

    /// 値を取り出す（外側の引用符を外したもの）。
    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        self.get_raw(section, key).map(|v| unquote(v).to_owned())
    }

    /// 値を取り出す（引用符を外さない）。
    pub fn get_raw(&self, section: &str, key: &str) -> Option<&str> {
        self.entry_index(section, key)
            .and_then(|i| match &self.lines[i].kind {
                LineKind::Entry { value, .. } => Some(value.as_str()),
                _ => None,
            })
    }

    /// セクション内のキーを出てきた順に返す（最初のセクションのみ・重複は最初のものだけ）。
    pub fn keys(&self, section: &str) -> Vec<&str> {
        let Some((start, end)) = self.section_range(section) else {
            return Vec::new();
        };
        let mut keys: Vec<&str> = Vec::new();
        for line in &self.lines[start + 1..end] {
            if let LineKind::Entry { key, .. } = &line.kind {
                if !keys.iter().any(|k| eq_name(k, key)) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// 値を設定する。キーがあればその行を書き換え、なければセクションの末尾に追加する。
    /// セクションがなければファイルの末尾に作る。値はそのまま書くので、必要なら
    /// [`encode_value`] を通してから渡す。
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        if let Some(i) = self.entry_index(section, key) {
            let line = &mut self.lines[i];
            let LineKind::Entry { key: old_key, .. } = &line.kind else {
                unreachable!("entry_index はキーの行を返す");
            };
            let old_key = old_key.clone();
            line.text = format!("{old_key}={value}");
            line.kind = LineKind::Entry {
                key: old_key,
                value: value.to_owned(),
            };
            return;
        }

        let eol = self.default_eol();
        let new_line = Line {
            kind: LineKind::Entry {
                key: key.to_owned(),
                value: value.to_owned(),
            },
            text: format!("{key}={value}"),
            eol: eol.clone(),
        };

        if let Some((start, end)) = self.section_range(section) {
            // セクション内の最後の「空行でない行」の直後に入れる。
            let mut insert_at = start + 1;
            for i in start + 1..end {
                if !self.lines[i].text.trim().is_empty() {
                    insert_at = i + 1;
                }
            }
            self.ensure_eol(insert_at);
            self.lines.insert(insert_at, new_line);
        } else {
            let len = self.lines.len();
            self.ensure_eol(len);
            if self.lines.last().is_some_and(|l| !l.text.trim().is_empty()) {
                self.lines.push(Line {
                    kind: LineKind::Other,
                    text: String::new(),
                    eol: eol.clone(),
                });
            }
            self.lines.push(Line {
                kind: LineKind::Section(section.to_owned()),
                text: format!("[{section}]"),
                eol,
            });
            self.lines.push(new_line);
        }
    }

    /// セクションを差し替えた文字列を作る。
    ///
    /// `remove(名前, 同じ名前の中で何番目か（0 始まり）)` が true のセクション（見出しから次の見出しの前まで）を
    /// 取り除き、最初に取り除いた位置に `insert` を入れる。取り除くものがなければ末尾に足す。
    /// それ以外の行（コメント・ほかのセクション）は 1 文字も変えない。
    pub fn rewrite_sections(&self, remove: impl Fn(&str, usize) -> bool, insert: &str) -> String {
        let eol = self.default_eol();
        let mut out = String::new();
        let mut seen: Vec<(String, usize)> = Vec::new();
        let mut removing = false;
        let mut inserted = false;
        for line in &self.lines {
            if let LineKind::Section(name) = &line.kind {
                let key = name.to_lowercase();
                let nth = match seen.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, n)) => {
                        *n += 1;
                        *n
                    }
                    None => {
                        seen.push((key, 0));
                        0
                    }
                };
                removing = remove(name, nth);
                if removing && !inserted {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push_str(&eol);
                    }
                    out.push_str(insert);
                    inserted = true;
                }
            }
            if !removing {
                out.push_str(&line.text);
                out.push_str(&line.eol);
            }
        }
        if !inserted {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(&eol);
            }
            let last_blank = out
                .trim_end_matches(['\r', '\n'])
                .rsplit('\n')
                .next()
                .is_none_or(|l| l.trim().is_empty());
            if !out.is_empty() && !last_blank {
                out.push_str(&eol);
            }
            out.push_str(insert);
        }
        out
    }

    /// 最初の同名セクションの範囲（見出し行の位置, 次のセクションの見出し行の位置）。
    fn section_range(&self, section: &str) -> Option<(usize, usize)> {
        let start = self
            .lines
            .iter()
            .position(|l| matches!(&l.kind, LineKind::Section(n) if eq_name(n, section)))?;
        let end = self.lines[start + 1..]
            .iter()
            .position(|l| matches!(l.kind, LineKind::Section(_)))
            .map_or(self.lines.len(), |p| start + 1 + p);
        Some((start, end))
    }

    fn entry_index(&self, section: &str, key: &str) -> Option<usize> {
        let (start, end) = self.section_range(section)?;
        (start + 1..end).find(
            |&i| matches!(&self.lines[i].kind, LineKind::Entry { key: k, .. } if eq_name(k, key)),
        )
    }

    /// ファイル内で使われている改行（最初に見つかったもの）。なければ OS の標準。
    pub fn default_eol(&self) -> String {
        self.lines
            .iter()
            .find(|l| !l.eol.is_empty())
            .map(|l| l.eol.clone())
            .unwrap_or_else(|| if cfg!(windows) { "\r\n" } else { "\n" }.to_owned())
    }

    /// `index` の位置に行を足す前に、直前の行に改行が付いていることを保証する。
    fn ensure_eol(&mut self, index: usize) {
        let eol = self.default_eol();
        if index > 0 {
            if let Some(prev) = self.lines.get_mut(index - 1) {
                if prev.eol.is_empty() {
                    prev.eol = eol;
                }
            }
        }
    }
}

fn classify(content: &str) -> LineKind {
    let t = content.trim();
    if let Some(inner) = t.strip_prefix('[') {
        let name = match inner.find(']') {
            Some(i) => &inner[..i],
            None => inner,
        };
        return LineKind::Section(name.trim().to_owned());
    }
    if t.starts_with(';') || t.starts_with('#') {
        return LineKind::Other;
    }
    match t.find('=') {
        Some(i) => LineKind::Entry {
            key: t[..i].trim().to_owned(),
            value: t[i + 1..].trim().to_owned(),
        },
        None => LineKind::Other,
    }
}

/// セクション名・キー名の比較（大文字小文字を区別しない）。
fn eq_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// 値の全体が `"…"` または `'…'` で囲まれていれば、外側の引用符を外す。
pub fn unquote(value: &str) -> &str {
    let b = value.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// 読み戻したときに同じ値になるように、必要なら値全体を `"…"` で囲む。
///
/// 前後に空白がある値や、先頭と末尾が同じ引用符の値は、そのまま書くと読み込み時に変わってしまう。
pub fn encode_value(value: &str) -> String {
    let needs_quote = value != value.trim() || unquote(value) != value;
    if needs_quote {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "; comment\r\n[Info]\r\nTitle=テスト用プリセット\r\n\r\n[item0]\r\nTitle=  TEST  \r\nTitle=DUP\r\nCommand0 = \"\"ffmpeg\" -i \"<%InputFile%>\"\"\r\n[Item0]\r\nTitle=SECOND\r\n";

    #[test]
    fn round_trip_is_identical() {
        let doc = IniDocument::parse(SAMPLE);
        assert_eq!(doc.to_text(), SAMPLE);
        let lf = SAMPLE.replace("\r\n", "\n");
        assert_eq!(IniDocument::parse(&lf).to_text(), lf);
        let no_trailing = "[A]\nk=v";
        assert_eq!(IniDocument::parse(no_trailing).to_text(), no_trailing);
    }

    #[test]
    fn names_are_case_insensitive_and_first_wins() {
        let doc = IniDocument::parse(SAMPLE);
        assert_eq!(doc.get("ITEM0", "title").as_deref(), Some("TEST"));
        assert_eq!(
            doc.get("Info", "Title").as_deref(),
            Some("テスト用プリセット")
        );
        assert_eq!(doc.keys("Item0"), vec!["Title", "Command0"]);
    }

    #[test]
    fn outer_quotes_are_removed() {
        let doc = IniDocument::parse(SAMPLE);
        assert_eq!(
            doc.get("Item0", "Command0").as_deref(),
            Some("\"ffmpeg\" -i \"<%InputFile%>\"")
        );
        assert_eq!(unquote("'abc'"), "abc");
        assert_eq!(unquote("\"a\" b"), "\"a\" b");
        assert_eq!(unquote("\""), "\"");
    }

    #[test]
    fn set_replaces_existing_value_only() {
        let mut doc = IniDocument::parse(SAMPLE);
        doc.set("Info", "Title", "新しい名前");
        let text = doc.to_text();
        assert!(text.contains("[Info]\r\nTitle=新しい名前\r\n"));
        assert!(text.starts_with("; comment\r\n"));
        assert!(text.ends_with("Title=SECOND\r\n"));
    }

    #[test]
    fn set_appends_key_and_section() {
        let mut doc = IniDocument::parse("[A]\nk=v\n\n[B]\nx=1");
        doc.set("A", "new", "2");
        doc.set("C", "y", "3");
        assert_eq!(doc.to_text(), "[A]\nk=v\nnew=2\n\n[B]\nx=1\n\n[C]\ny=3\n");
    }

    #[test]
    fn encode_value_round_trips() {
        for v in [
            "plain",
            " lead",
            "\"q\" x \"y\"",
            "'s'",
            "\"ffmpeg\" -i \"a\"",
        ] {
            let mut doc = IniDocument::default();
            doc.set("S", "k", &encode_value(v));
            let re = IniDocument::parse(&doc.to_text());
            assert_eq!(re.get("S", "k").as_deref(), Some(v), "value {v:?}");
        }
    }

    #[test]
    fn rewrite_replaces_sections_in_place() {
        let doc = IniDocument::parse(
            "; head\r\n[A]\r\nk=1\r\n\r\n[B]\r\nx=1\r\n[A]\r\nk=2\r\n[C]\r\ny=1\r\n",
        );
        // 最初の [A] と [B] を消して、その位置に差し込む。2 つ目の [A] は残す
        let out = doc.rewrite_sections(
            |name, nth| (name == "A" && nth == 0) || name == "B",
            "[N]\r\nz=1\r\n\r\n",
        );
        assert_eq!(
            out,
            "; head\r\n[N]\r\nz=1\r\n\r\n[A]\r\nk=2\r\n[C]\r\ny=1\r\n"
        );
        // 消すものがなければ末尾に足す（空行を 1 つ挟む）
        let out = IniDocument::parse("[A]\nk=1").rewrite_sections(|_, _| false, "[N]\nz=1\n");
        assert_eq!(out, "[A]\nk=1\n\n[N]\nz=1\n");
    }

    #[test]
    fn warnings_report_line_numbers() {
        let doc = IniDocument::parse("orphan=1\n[A]\n; ok\ngarbage line\n");
        let w = doc.warnings();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].line, 1);
        assert_eq!(w[1].line, 4);
    }
}
