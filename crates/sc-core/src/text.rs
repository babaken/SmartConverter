//! 文字コードの判別と変換（要件 F-EN-01、F-EN-04）。

/// 読み込んだテキストの文字コード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// UTF-8（先頭に BOM あり）
    Utf8Bom,
    /// UTF-8（BOM なし）
    Utf8,
    /// Shift-JIS（Windows の CP932）
    ShiftJis,
}

impl TextEncoding {
    /// 画面に出す名前。
    pub fn display_name(self) -> &'static str {
        match self {
            TextEncoding::Utf8Bom => "UTF-8 (BOM)",
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::ShiftJis => "Shift-JIS",
        }
    }
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// バイト列を文字列にする。
///
/// 判別の順番：① UTF-8 の BOM があれば UTF-8 ② UTF-8 として正しければ UTF-8 ③ それ以外は CP932。
pub fn decode(bytes: &[u8]) -> (String, TextEncoding) {
    if let Some(rest) = bytes.strip_prefix(UTF8_BOM) {
        return (
            String::from_utf8_lossy(rest).into_owned(),
            TextEncoding::Utf8Bom,
        );
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_owned(), TextEncoding::Utf8),
        Err(_) => (decode_cp932(bytes), TextEncoding::ShiftJis),
    }
}

/// 外部ツールの出力 1 行分を文字列にする（UTF-8 として正しくなければ CP932 とみなす）。
pub fn decode_line(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => decode_cp932(bytes),
    }
}

/// 端末用の制御文字（色付けなどの `ESC [ … 文字`）を取り除く（MP4Box などが出す）。
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for x in chars.by_ref() {
                    if x.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn decode_cp932(bytes: &[u8]) -> String {
    // encoding_rs の SHIFT_JIS は WHATWG の定義で、CP932（Windows-31J）の拡張文字も含む。
    let (text, _had_errors) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(bytes);
    text.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_utf8_bom() {
        let (s, enc) = decode(b"\xEF\xBB\xBF[Info]");
        assert_eq!(enc, TextEncoding::Utf8Bom);
        assert_eq!(s, "[Info]");
    }

    #[test]
    fn detects_utf8_without_bom() {
        let (s, enc) = decode("タイトル".as_bytes());
        assert_eq!(enc, TextEncoding::Utf8);
        assert_eq!(s, "タイトル");
    }

    #[test]
    fn falls_back_to_cp932() {
        // 「改　QVGA」の CP932 表現（全角スペースを含む）
        let bytes = [0x89, 0xFC, 0x81, 0x40, b'Q', b'V', b'G', b'A'];
        let (s, enc) = decode(&bytes);
        assert_eq!(enc, TextEncoding::ShiftJis);
        assert_eq!(s, "改　QVGA");
    }

    #[test]
    fn decodes_cp932_specific_characters() {
        // ① は CP932 の拡張文字（0x8740）
        let (s, enc) = decode(&[0x87, 0x40]);
        assert_eq!(enc, TextEncoding::ShiftJis);
        assert_eq!(s, "①");
    }

    #[test]
    fn ascii_is_utf8() {
        assert_eq!(decode(b"Title=TEST").1, TextEncoding::Utf8);
    }

    #[test]
    fn strips_ansi_sequences() {
        assert_eq!(
            strip_ansi("\u{1b}[32mTrack\u{1b}[0m Import"),
            "Track Import"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn decode_line_handles_both() {
        assert_eq!(decode_line("エラー".as_bytes()), "エラー");
        assert_eq!(decode_line(&[0x83, 0x47, 0x83, 0x89, 0x81, 0x5B]), "エラー");
    }
}
