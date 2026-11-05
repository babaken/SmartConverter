//! 日本語フォントの読み込み（基本設計 §9.3）。
//!
//! OS の日本語 UI フォントを探して、egui の既定フォントの後ろ（足りない文字の代わり）に加える。
//! 見つからなければ何もしない（egui の既定フォントだけになる）。

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// (ファイル, TTC 内の番号) の候補。上から順に探す。
#[cfg(windows)]
fn candidates() -> Vec<(std::path::PathBuf, u32)> {
    let fonts = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    vec![
        // Yu Gothic UI（YuGothM.ttc の 1 番目が UI 用）
        (fonts.join("YuGothM.ttc"), 1),
        (fonts.join("YuGothR.ttc"), 1),
        // Meiryo UI（meiryo.ttc の 2 番目）
        (fonts.join("meiryo.ttc"), 2),
        // MS UI Gothic（msgothic.ttc の 2 番目）
        (fonts.join("msgothic.ttc"), 2),
    ]
}

#[cfg(unix)]
fn candidates() -> Vec<(std::path::PathBuf, u32)> {
    // fontconfig に日本語のゴシック体を聞く。
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", "sans-serif:lang=ja"])
        .output();
    let mut list = Vec::new();
    if let Ok(out) = out {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        if let (Some(file), Some(index)) = (lines.next(), lines.next()) {
            list.push((file.into(), index.trim().parse().unwrap_or(0)));
        }
    }
    list.push((
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".into(),
        0,
    ));
    list.push((
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into(),
        0,
    ));
    list
}

/// 日本語フォントを読み込んで登録する。使ったフォントのパスを返す。
pub fn install(ctx: &egui::Context) -> Option<String> {
    for (path, index) in candidates() {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert("jp".into(), data.into());
        // 文字は egui の既定フォントを優先し、無い文字（日本語）だけ日本語フォントで描く。
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("jp".into());
        }
        ctx.set_fonts(fonts);
        return Some(path.display().to_string());
    }
    None
}
