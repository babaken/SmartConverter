//! CP932（WHATWG の Shift_JIS）の 2 バイト文字の表を作る（`src/sjis.rs` が使う）。
//!
//! 表はビルドのときに encoding_rs から作る。encoding_rs は文字コードの表をすべて持っていて
//! 実行ファイルが大きくなるので、実行時には使わない。

use std::io::Write;

/// 先頭のバイトの種類の数（0x81..=0x9F と 0xE0..=0xFC）。
const LEADS: usize = 60;
/// 2 バイト目の種類の数（0x40..=0x7E と 0x80..=0xFC）。
const TRAILS: usize = 188;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let mut table = Vec::with_capacity(LEADS * TRAILS * 2);
    for pointer in 0..LEADS * TRAILS {
        let (lead, trail) = (pointer / TRAILS, pointer % TRAILS);
        let lead = (lead + if lead < 0x1F { 0x81 } else { 0xC1 }) as u8;
        let trail = (trail + if trail < 0x3F { 0x40 } else { 0x41 }) as u8;
        let bytes = [lead, trail];
        let (text, had_errors) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(&bytes);
        let mut chars = text.chars();
        // 1 文字に変換できたものだけを表に入れる（0 は「対応する文字が無い」）。
        let code = match (had_errors, chars.next(), chars.next()) {
            (false, Some(c), None) => u16::try_from(u32::from(c)).expect("BMP の外の文字は無い"),
            _ => 0,
        };
        table.extend_from_slice(&code.to_le_bytes());
    }

    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("sjis_table.bin");
    std::fs::File::create(out)
        .and_then(|mut f| f.write_all(&table))
        .expect("表を書き出せない");
}
