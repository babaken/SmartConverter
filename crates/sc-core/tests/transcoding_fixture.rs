//! 本家の書き方の Transcoding.ini（Shift-JIS）での互換テスト（受け入れテスト AT-PR-01〜04）。
//!
//! `fixtures/ini/transcoding_sjis.ini` は、同梱のプリセット例（携帯動画変換君の既定の設定を今の ffmpeg で書き直したもの）を
//! もとに、本家の書き方（コマンドの外側の `"…"`、削除の行、形式の違うセクション）を入れたもの。

use sc_core::ini::IniDocument;
use sc_core::preset::PresetSet;
use sc_core::text::{self, TextEncoding};

const SJIS: &[u8] = include_bytes!("fixtures/ini/transcoding_sjis.ini");

fn load() -> PresetSet {
    let (text, enc) = text::decode(SJIS);
    assert_eq!(enc, TextEncoding::ShiftJis);
    PresetSet::parse(&text, enc)
}

#[test]
fn lists_eight_presets() {
    let set = load();
    let titles: Vec<&str> = set.presets.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        titles,
        vec![
            "QVGA標準画質15fpsモノラル",
            "QVGA標準画質15fpsステレオ（2 段）",
            "QVGA高画質15fpsステレオ",
            "QCIF標準画質15fpsモノラル",
            "QCIF標準画質15fpsステレオ",
            "QCIF高画質15fpsステレオ",
            "音だけ AAC 96kbps",
            "音だけ Opus 96kbps",
        ]
    );
    assert_eq!(set.info.title, "互換テスト用");
    assert_eq!(
        set.ignored_sections,
        vec!["*テンプ or ゴミ", "0Item", "1Item"]
    );
}

#[test]
fn commands_have_outer_quotes_removed() {
    let set = load();
    let p = &set.presets[1];
    assert_eq!(p.commands.len(), 5);
    assert!(p.commands[0].starts_with(r#""ffmpeg" -y -i "<%InputFile%>""#));
    assert!(p.commands[0].ends_with(r#"-b:v 190k "<%TemporaryFile%>.m4v""#));
    assert_eq!(p.commands[4], r#"cmd /c "del "<%TemporaryFile%>*.*"""#);
}

#[test]
fn round_trip_keeps_text_identical() {
    let (text, _) = text::decode(SJIS);
    assert_eq!(IniDocument::parse(&text).to_text(), text);
}

#[test]
fn utf8_and_lf_variants_give_same_presets() {
    let base = load();
    let (text, _) = text::decode(SJIS);
    let utf8 = text.as_bytes().to_vec();
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend_from_slice(&utf8);
    let lf = text.replace("\r\n", "\n").into_bytes();
    for (bytes, enc) in [
        (utf8, TextEncoding::Utf8),
        (bom, TextEncoding::Utf8Bom),
        (lf, TextEncoding::Utf8),
    ] {
        let (t, e) = text::decode(&bytes);
        assert_eq!(e, enc);
        let set = PresetSet::parse(&t, e);
        assert_eq!(set.presets, base.presets);
    }
}

// ---------------------------------------------------------------------------
// プリセット編集の保存（受け入れテスト AT-PE-01〜02）

/// 元のファイルの中から、あるセクション（見出しから次の見出しの前まで）の文字列を取り出す。
fn section_text<'a>(text: &'a str, header: &str) -> &'a str {
    let start = text.find(header).expect("見出しがない");
    let rest = &text[start + header.len()..];
    let end = rest.find("\r\n[").map_or(rest.len(), |i| i + 2);
    &text[start..start + header.len() + end]
}

#[test]
fn save_without_changes_keeps_presets_and_other_sections() {
    let set = load();
    let out = set.render(&set.info, &set.presets);
    let re = PresetSet::parse(&out, TextEncoding::Utf8);
    assert_eq!(re.info, set.info);
    assert_eq!(re.presets, set.presets);
    assert_eq!(re.ignored_sections, set.ignored_sections);
    // プリセット以外のセクションは 1 文字も変わらない
    let (orig, _) = text::decode(SJIS);
    for header in ["[*テンプ or ゴミ]", "[0Item]", "[1Item]"] {
        assert!(
            out.contains(section_text(&orig, header)),
            "{header} が変わっている"
        );
    }
    // 改行は元のファイルと同じ CRLF
    assert!(!out.replace("\r\n", "").contains('\n'));
}

#[test]
fn save_renumbers_after_reorder_delete_and_add() {
    let set = load();
    let mut presets = set.presets.clone();
    presets.swap(0, 1); // 並べ替え
    presets.remove(2); // 「QVGA高画質」を削除
    let mut copy = presets[0].clone(); // 複製して末尾に追加
    copy.title = "QVGA ステレオのコピー".into();
    presets.push(copy);
    let mut info = set.info.clone();
    info.title = "新しい名前".into();

    let out = set.render(&info, &presets);
    let re = PresetSet::parse(&out, TextEncoding::Utf8);
    let titles: Vec<&str> = re.presets.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        titles,
        vec![
            "QVGA標準画質15fpsステレオ（2 段）",
            "QVGA標準画質15fpsモノラル",
            "QCIF標準画質15fpsモノラル",
            "QCIF標準画質15fpsステレオ",
            "QCIF高画質15fpsステレオ",
            "音だけ AAC 96kbps",
            "音だけ Opus 96kbps",
            "QVGA ステレオのコピー",
        ]
    );
    assert_eq!(re.presets[7].commands, set.presets[1].commands);
    assert_eq!(re.info.title, "新しい名前");
    assert_eq!(re.ignored_sections, set.ignored_sections);
    // 番号は 0 から振り直される（8 件なので [Item7] まで）
    assert!(out.contains("[Item7]\r\nTitle=QVGA ステレオのコピー\r\n"));
    assert!(!out.contains("[Item8]"));
}

#[test]
fn save_writes_utf8_and_backup() {
    let dir = std::env::temp_dir().join(format!("sc_preset_save_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Transcoding.ini");
    std::fs::write(&path, SJIS).unwrap();

    let set = PresetSet::load(&path).unwrap();
    let mut presets = set.presets.clone();
    presets[0].title = "名前を変えた".into();
    set.save(&path, &set.info, &presets).unwrap();

    // 元のファイル（Shift-JIS）がそのまま .bak に残る
    assert_eq!(
        std::fs::read(dir.join("Transcoding.ini.bak")).unwrap(),
        SJIS
    );
    // 保存したファイルは UTF-8（BOM なし）で、読み直すと変更が入っている
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
    let re = PresetSet::load(&path).unwrap();
    assert_eq!(re.encoding, TextEncoding::Utf8);
    assert_eq!(re.presets[0].title, "名前を変えた");
    assert_eq!(re.presets[1..], set.presets[1..]);
    std::fs::remove_dir_all(&dir).unwrap();
}
