//! CP932（Windows-31J）の変換。WHATWG Encoding Standard の Shift_JIS と同じ動き
//! （encoding_rs の `SHIFT_JIS` と同じ結果）にしてある。
//!
//! encoding_rs はすべての文字コードの表を持っていて実行ファイルが大きくなるので、
//! 2 バイト文字の表（約 22 KB）だけを `build.rs` で作って使う。

/// 2 バイト文字の表（WHATWG の「pointer」の順。値は Unicode、0 は対応する文字が無い）。
static TABLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sjis_table.bin"));

/// 2 バイト目の種類の数（0x40..=0x7E と 0x80..=0xFC）。
const TRAILS: usize = 188;
/// 外字（ユーザー定義文字）の pointer。Unicode の私用領域 U+E000 から順に対応する。
const PUA: std::ops::RangeInclusive<usize> = 8836..=10715;
/// 文字から変換するときに使わない pointer（NEC が選んだ IBM 拡張文字。同じ文字が IBM 拡張文字の側にもある）。
const ENCODE_SKIP: std::ops::RangeInclusive<usize> = 8272..=8835;

fn table(pointer: usize) -> Option<char> {
    let i = pointer * 2;
    let code = u16::from_le_bytes([*TABLE.get(i)?, *TABLE.get(i + 1)?]);
    (code != 0).then(|| char::from_u32(code.into())).flatten()
}

/// 2 バイト文字の先頭のバイトか。
fn is_lead(b: u8) -> bool {
    matches!(b, 0x81..=0x9F | 0xE0..=0xFC)
}

/// CP932 のバイト列を文字列にする。変換できないところは U+FFFD にする。
pub fn decode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        match b {
            0x00..=0x80 => out.push(char::from(b)),
            0xA1..=0xDF => out.push(char::from_u32(0xFF61 + u32::from(b - 0xA1)).unwrap()),
            _ if is_lead(b) => {
                let Some(&t) = bytes.get(i) else {
                    out.push('\u{FFFD}');
                    break;
                };
                let lead_offset = if b < 0xA0 { 0x81 } else { 0xC1 };
                let c = match t {
                    0x40..=0x7E | 0x80..=0xFC => {
                        let trail_offset = if t < 0x7F { 0x40 } else { 0x41 };
                        let pointer =
                            usize::from(b - lead_offset) * TRAILS + usize::from(t - trail_offset);
                        table(pointer)
                    }
                    _ => None,
                };
                match c {
                    Some(c) => {
                        out.push(c);
                        i += 1;
                    }
                    None => {
                        out.push('\u{FFFD}');
                        // 2 バイト目が ASCII なら、その文字は捨てずに次の文字として読む。
                        if t >= 0x80 {
                            i += 1;
                        }
                    }
                }
            }
            _ => out.push('\u{FFFD}'),
        }
    }
    out
}

/// 文字列を CP932 のバイト列にする。表せない文字は `&#10進数;` にし、そのときは 2 つ目の値が true。
pub fn encode(text: &str) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(text.len());
    let mut unmappable = false;
    for c in text.chars() {
        let code = u32::from(c);
        match code {
            0x00..=0x80 => out.push(code as u8),
            0xA5 => out.push(0x5C),
            0x203E => out.push(0x7E),
            0xFF61..=0xFF9F => out.push((code - 0xFF61 + 0xA1) as u8),
            _ => {
                let c = if c == '\u{2212}' { '\u{FF0D}' } else { c };
                match encode_pointer(c) {
                    Some(pointer) => {
                        let (lead, trail) = (pointer / TRAILS, pointer % TRAILS);
                        out.push((lead + if lead < 0x1F { 0x81 } else { 0xC1 }) as u8);
                        out.push((trail + if trail < 0x3F { 0x40 } else { 0x41 }) as u8);
                    }
                    None => {
                        unmappable = true;
                        out.extend_from_slice(format!("&#{code};").as_bytes());
                    }
                }
            }
        }
    }
    (out, unmappable)
}

/// 文字に対応する pointer（いちばん小さいもの）。外字と [`ENCODE_SKIP`] は使わない。
/// スクリプトを書くときにしか使わず、文字数も少ないので、表を順に探す。
fn encode_pointer(c: char) -> Option<usize> {
    (0..TABLE.len() / 2)
        .filter(|p| !PUA.contains(p) && !ENCODE_SKIP.contains(p))
        .find(|&p| table(p) == Some(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::SHIFT_JIS;

    fn expected_decode(bytes: &[u8]) -> String {
        SHIFT_JIS.decode_without_bom_handling(bytes).0.into_owned()
    }

    #[test]
    fn decode_matches_encoding_rs_for_all_byte_pairs() {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let bytes = [a, b];
                assert_eq!(decode(&bytes), expected_decode(&bytes), "{bytes:02X?}");
                // 2 バイト目の後ろに文字が続くとき（2 バイト目を読み直すかどうか）
                let bytes = [a, b, b'A', 0x82, 0xA0];
                assert_eq!(decode(&bytes), expected_decode(&bytes), "{bytes:02X?}");
            }
            // 先頭のバイトで終わるとき
            assert_eq!(decode(&[b'x', a]), expected_decode(&[b'x', a]), "{a:02X}");
        }
    }

    #[test]
    fn decode_matches_encoding_rs_for_random_bytes() {
        // 決まった種から作る擬似乱数（xorshift）で、長いバイト列を比べる。
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..2000 {
            let bytes: Vec<u8> = (0..64)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                })
                .collect();
            assert_eq!(decode(&bytes), expected_decode(&bytes), "{bytes:02X?}");
        }
    }

    #[test]
    fn encode_matches_encoding_rs_for_all_chars() {
        let chars = (0..=0xFFFFu32)
            .chain([0x1F600, 0x20B9F, 0x10FFFF])
            .filter_map(char::from_u32);
        for c in chars {
            let s = c.to_string();
            let (bytes, _, unmappable) = SHIFT_JIS.encode(&s);
            assert_eq!(
                encode(&s),
                (bytes.into_owned(), unmappable),
                "U+{:04X}",
                u32::from(c)
            );
        }
    }

    #[test]
    fn encodes_and_decodes_text() {
        let text = "C:\\動画\\①ｶﾀｶﾅ～髙.avs";
        let (bytes, unmappable) = encode(text);
        assert!(!unmappable);
        assert_eq!(bytes, SHIFT_JIS.encode(text).0.as_ref());
        assert_eq!(decode(&bytes), text);
    }
}
