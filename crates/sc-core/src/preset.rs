//! プリセット定義ファイル（Transcoding.ini）の読み込み（要件 §4.1）。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::ini::{encode_value, IniDocument, IniWarning};
use crate::text::{self, TextEncoding};

/// `[Info]` セクション。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresetInfo {
    pub title: String,
    pub description: String,
}

/// プリセット 1 つ（`[ItemN]`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preset {
    /// `[ItemN]` の N
    pub index: usize,
    pub title: String,
    /// `Command0..` の連番（外側の引用符を外したもの）
    pub commands: Vec<String>,
    /// Title・CommandN 以外のキー（キー, 書かれていたままの値）。保存しても残す
    pub extra: Vec<(String, String)>,
}

/// プリセット定義ファイル全体。
#[derive(Debug, Clone)]
pub struct PresetSet {
    /// 読み込んだファイル（文字列から作ったときは `None`）
    pub path: Option<PathBuf>,
    /// 読み込んだときの文字コード
    pub encoding: TextEncoding,
    pub info: PresetInfo,
    pub presets: Vec<Preset>,
    /// プリセットとして読まれないセクション（`[0Item]` など）。保存しても残る
    pub ignored_sections: Vec<String>,
    /// 解釈できなかった行
    pub warnings: Vec<IniWarning>,
    doc: IniDocument,
}

impl PresetSet {
    /// ファイルから読み込む。
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let (text, encoding) = text::decode(&bytes);
        let mut set = Self::parse(&text, encoding);
        set.path = Some(path.to_owned());
        if set.presets.is_empty() {
            return Err(Error::NoPresets {
                path: path.to_owned(),
            });
        }
        Ok(set)
    }

    /// 文字列から作る（プリセットが 0 件でもエラーにしない）。
    pub fn parse(text: &str, encoding: TextEncoding) -> Self {
        let doc = IniDocument::parse(text);
        let info = PresetInfo {
            title: doc.get("Info", "Title").unwrap_or_default(),
            description: doc.get("Info", "Description").unwrap_or_default(),
        };

        // [Item0] から連番で読み、欠番で終える（本家互換）。
        let mut presets = Vec::new();
        for index in 0.. {
            let section = format!("Item{index}");
            if !doc.has_section(&section) {
                break;
            }
            let title = doc.get(&section, "Title").unwrap_or_default();
            let mut commands = Vec::new();
            for n in 0.. {
                match doc.get(&section, &format!("Command{n}")) {
                    Some(cmd) => commands.push(cmd),
                    None => break,
                }
            }
            let extra = doc
                .keys(&section)
                .into_iter()
                .filter(|k| !k.eq_ignore_ascii_case("Title") && command_index(k).is_none())
                .filter_map(|k| {
                    doc.get_raw(&section, k)
                        .map(|v| (k.to_owned(), v.to_owned()))
                })
                .collect();
            presets.push(Preset {
                index,
                title,
                commands,
                extra,
            });
        }

        let count = presets.len();
        let mut ignored_sections: Vec<String> = Vec::new();
        for name in doc.section_names() {
            let is_info = name.eq_ignore_ascii_case("Info");
            let is_used_item = item_index(name).is_some_and(|n| n < count);
            if !is_info && !is_used_item && !ignored_sections.iter().any(|s| s == name) {
                ignored_sections.push(name.to_owned());
            }
        }

        Self {
            path: None,
            encoding,
            info,
            presets,
            ignored_sections,
            warnings: doc.warnings(),
            doc,
        }
    }

    /// `[ItemN]` の N で探す。
    pub fn by_index(&self, index: usize) -> Option<&Preset> {
        self.presets.get(index)
    }

    /// タイトルで探す（完全一致）。
    pub fn by_title(&self, title: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.title == title)
    }

    /// 元の行を保持した INI ドキュメント。
    pub fn document(&self) -> &IniDocument {
        &self.doc
    }

    /// 編集した内容で、ファイルの中身を作る（要件 F-PE-04）。
    ///
    /// - `[ItemN]` は `presets` の並び順で 0 から振り直す（`index` は無視する）
    /// - 元の `[ItemN]` があった位置に書き、それ以外のセクション（`[0Item]` など）・コメントは変えない
    /// - コマンドは本家と同じく値全体を `"…"` で囲んで書く
    /// - 改行は元のファイルに合わせる
    pub fn render(&self, info: &PresetInfo, presets: &[Preset]) -> String {
        let eol = self.doc.default_eol();
        let mut items = String::new();
        for (n, p) in presets.iter().enumerate() {
            items.push_str(&format!("[Item{n}]{eol}"));
            items.push_str(&format!("Title={}{eol}", encode_value(&p.title)));
            for (i, c) in p.commands.iter().enumerate() {
                items.push_str(&format!("Command{i}=\"{c}\"{eol}"));
            }
            for (k, v) in &p.extra {
                items.push_str(&format!("{k}={v}{eol}"));
            }
            items.push_str(&eol);
        }

        // 今プリセットとして読まれている [ItemN]（同じ名前の最初のもの）だけを差し替える。
        let old_count = self.presets.len();
        let text = self.doc.rewrite_sections(
            |name, nth| nth == 0 && item_index(name).is_some_and(|n| n < old_count),
            &items,
        );

        // [Info] を更新する（無ければ先頭に作る）。
        let mut doc = IniDocument::parse(&text);
        if doc.has_section("Info") {
            doc.set("Info", "Title", &encode_value(&info.title));
            doc.set("Info", "Description", &encode_value(&info.description));
            doc.to_text()
        } else if info.title.is_empty() && info.description.is_empty() {
            text
        } else {
            format!(
                "[Info]{eol}Title={}{eol}Description={}{eol}{eol}{text}",
                encode_value(&info.title),
                encode_value(&info.description)
            )
        }
    }

    /// 編集した内容を UTF-8 で保存する。保存前に今のファイルを `<名前>.bak` にコピーする。
    pub fn save(&self, path: &Path, info: &PresetInfo, presets: &[Preset]) -> Result<()> {
        let text = self.render(info, presets);
        if path.is_file() {
            let mut bak = path.as_os_str().to_owned();
            bak.push(".bak");
            std::fs::copy(path, PathBuf::from(bak)).map_err(|source| Error::Write {
                path: path.to_owned(),
                source,
            })?;
        }
        crate::paths::write_atomic(path, text).map_err(|source| Error::Write {
            path: path.to_owned(),
            source,
        })
    }
}

/// 保存できない理由（画面に出す）。問題がなければ空。
pub fn validate(presets: &[Preset]) -> Vec<String> {
    let mut problems = Vec::new();
    if presets.is_empty() {
        problems.push("プリセットが 1 つもありません".to_owned());
    }
    for (i, p) in presets.iter().enumerate() {
        let title = p.title.trim();
        if title.is_empty() {
            problems.push(format!("{} 番目のプリセットのタイトルが空です", i + 1));
        } else if presets[..i].iter().any(|q| q.title.trim() == title) {
            problems.push(format!("タイトル「{title}」が重複しています"));
        }
        if p.commands.iter().all(|c| c.trim().is_empty()) {
            let name = if title.is_empty() {
                "（タイトルなし）"
            } else {
                title
            };
            problems.push(format!("「{name}」にコマンドがありません"));
        }
    }
    problems
}

/// `CommandN` なら N を返す。
fn command_index(key: &str) -> Option<usize> {
    let rest = key
        .get(..7)?
        .eq_ignore_ascii_case("command")
        .then(|| &key[7..])?;
    rest.parse().ok()
}

/// `ItemN`（大文字小文字を区別しない、N は先頭に 0 のない 10 進数）なら N を返す。
fn item_index(name: &str) -> Option<usize> {
    let rest = name
        .get(..4)?
        .eq_ignore_ascii_case("item")
        .then(|| &name[4..])?;
    if rest.is_empty() || (rest.len() > 1 && rest.starts_with('0')) {
        return None;
    }
    rest.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "[Info]\nTitle=T\nDescription=D\n\n[Item0]\nTitle=A\nCommand0=\"\"ffmpeg\" -i \"<%InputFile%>\"\"\nCommand1=b\nCommand3=skipped\n\n[Item1]\nTitle=B\n\n[Item3]\nTitle=gap\n[0Item]\nTitle=X\n[*temp]\nTitle=Y\n";

    #[test]
    fn reads_items_and_commands_until_gap() {
        let set = PresetSet::parse(TEXT, TextEncoding::Utf8);
        assert_eq!(set.info.title, "T");
        assert_eq!(set.info.description, "D");
        assert_eq!(set.presets.len(), 2);
        assert_eq!(
            set.presets[0].commands,
            vec!["\"ffmpeg\" -i \"<%InputFile%>\"".to_owned(), "b".to_owned()]
        );
        assert!(set.presets[1].commands.is_empty());
        assert_eq!(set.ignored_sections, vec!["Item3", "0Item", "*temp"]);
    }

    #[test]
    fn lookup_by_index_and_title() {
        let set = PresetSet::parse(TEXT, TextEncoding::Utf8);
        assert_eq!(set.by_index(1).map(|p| p.title.as_str()), Some("B"));
        assert_eq!(set.by_title("A").map(|p| p.index), Some(0));
        assert!(set.by_index(2).is_none());
        assert!(set.by_title("a").is_none());
    }

    #[test]
    fn extra_keys_are_kept() {
        let text = "[Item0]\nTitle=A\nNote=\"keep me\"\nCommand0=x\nCommand1=y\n";
        let set = PresetSet::parse(text, TextEncoding::Utf8);
        assert_eq!(
            set.presets[0].extra,
            vec![("Note".to_owned(), "\"keep me\"".to_owned())]
        );
        let out = set.render(&set.info, &set.presets);
        assert!(out.contains("Note=\"keep me\"\n"), "{out}");
        assert!(out.contains("Command0=\"x\"\nCommand1=\"y\"\n"), "{out}");
    }

    #[test]
    fn info_is_created_at_top_when_missing() {
        let set = PresetSet::parse("[Item0]\nTitle=A\nCommand0=x\n", TextEncoding::Utf8);
        let info = PresetInfo {
            title: "名前".into(),
            description: String::new(),
        };
        let out = set.render(&info, &set.presets);
        assert!(
            out.starts_with("[Info]\nTitle=名前\nDescription=\n\n[Item0]"),
            "{out}"
        );
        // 空のままなら作らない
        let out = set.render(&PresetInfo::default(), &set.presets);
        assert!(!out.contains("[Info]"));
    }

    #[test]
    fn titles_with_spaces_or_quotes_round_trip() {
        let set = PresetSet::parse("[Item0]\nTitle=A\nCommand0=x\n", TextEncoding::Utf8);
        let mut presets = set.presets.clone();
        presets[0].title = " 前後に空白 ".into();
        presets[0].commands = vec!["\"ffmpeg\" -i \"<%InputFile%>\"".into()];
        let re = PresetSet::parse(&set.render(&set.info, &presets), TextEncoding::Utf8);
        assert_eq!(re.presets[0].title, " 前後に空白 ");
        assert_eq!(re.presets[0].commands, presets[0].commands);
    }

    #[test]
    fn validation_messages() {
        let p = |title: &str, cmds: &[&str]| Preset {
            title: title.into(),
            commands: cmds.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert!(validate(&[p("A", &["x"]), p("B", &["y"])]).is_empty());
        assert_eq!(validate(&[]), vec!["プリセットが 1 つもありません"]);
        let problems = validate(&[p("A", &["x"]), p("A", &["y"]), p(" ", &[]), p("C", &["  "])]);
        assert_eq!(
            problems,
            vec![
                "タイトル「A」が重複しています",
                "3 番目のプリセットのタイトルが空です",
                "「（タイトルなし）」にコマンドがありません",
                "「C」にコマンドがありません",
            ]
        );
    }

    #[test]
    fn item_index_parsing() {
        assert_eq!(item_index("Item12"), Some(12));
        assert_eq!(item_index("item0"), Some(0));
        assert_eq!(item_index("Item01"), None);
        assert_eq!(item_index("0Item"), None);
        assert_eq!(item_index("Item"), None);
    }
}
