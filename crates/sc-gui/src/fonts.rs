//! 日本語フォントの読み込み（基本設計 §9.3）。
//!
//! OS の日本語 UI フォントを探して、egui の既定フォントの後ろ（足りない文字の代わり）に加える。
//! それでも足りない文字（ハングル・簡体字・タイ文字など）は、[`OsFallback`] が OS のフォントから探す。
//! どちらも OS に入っているフォントを使い、フォントは実行ファイルに入れない。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use eframe::epaint::text::{
    FallbackRequest, FontInsert, FontPriority, FontProvider, InsertFontFamily,
};

/// 日本語フォントの候補（ファイル, TTC 内の番号, フォント名）。上から順に探す。
/// フォント名は、Windows の代わりのフォントの一覧（FontLink）を引くのに使う。
#[cfg(windows)]
fn candidates() -> Vec<(PathBuf, u32, &'static str)> {
    let fonts = windows_fonts_dir();
    vec![
        // Yu Gothic UI（YuGothM.ttc の 1 番目が UI 用）
        (fonts.join("YuGothM.ttc"), 1, "Yu Gothic UI"),
        (fonts.join("YuGothR.ttc"), 1, "Yu Gothic UI"),
        // Meiryo UI（meiryo.ttc の 2 番目）
        (fonts.join("meiryo.ttc"), 2, "Meiryo UI"),
        // MS UI Gothic（msgothic.ttc の 2 番目）
        (fonts.join("msgothic.ttc"), 2, "MS UI Gothic"),
    ]
}

#[cfg(windows)]
fn windows_fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts")
}

#[cfg(unix)]
fn candidates() -> Vec<(PathBuf, u32, &'static str)> {
    // fontconfig に日本語のゴシック体を聞く。
    let mut list: Vec<_> = fc_match("sans-serif:lang=ja")
        .map(|(file, index)| (file, index, ""))
        .into_iter()
        .collect();
    list.push((
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".into(),
        0,
        "",
    ));
    list.push((
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into(),
        0,
        "",
    ));
    list
}

/// fontconfig（`fc-match`）で、パターンにいちばん合うフォントの (ファイル, TTC 内の番号) を得る。
#[cfg(unix)]
fn fc_match(pattern: &str) -> Option<(PathBuf, u32)> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", pattern])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let file = lines.next().filter(|f| !f.is_empty())?;
    let index = lines
        .next()
        .and_then(|i| i.trim().parse().ok())
        .unwrap_or(0);
    Some((file.into(), index))
}

/// 日本語フォントを読み込んで登録する。使ったフォントのパスを返す。
pub fn install(ctx: &egui::Context) -> Option<String> {
    let mut used = None;
    for (path, index, name) in candidates() {
        // 丸ごと読み込まずにメモリマップする（実際に使う字形のところだけがメモリに載る）。
        let Some(bytes) = sc_core::platform::map_file_static(&path) else {
            continue;
        };
        let mut data = FontData::from_static(bytes);
        data.index = index;
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert("jp".into(), data.into());
        // 文字は egui の既定フォントを優先し、無い文字（日本語）だけ日本語フォントで描く。
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("jp".into());
        }
        ctx.set_fonts(fonts);
        used = Some((path, name));
        break;
    }
    // 日本語フォントが見つからなくても、足りない文字は OS のフォントから探す。
    let base_name = used.as_ref().map_or("", |(_, name)| *name);
    ctx.add_font_provider(Arc::new(OsFallback::new(base_name)));
    used.map(|(path, _)| path.display().to_string())
}

/// 足りない文字を描けるフォントを、OS に入っているフォントから探す（egui の [`FontProvider`]）。
///
/// - Windows：OS の「代わりのフォント」の一覧（レジストリの FontLink\SystemLink）の順に探す。
///   一覧は OS の言語ごとに違う（韓国語の Windows なら韓国語のフォントが先になる）。
/// - Linux：fontconfig（`fc-match`）に、その文字を含むフォントを聞く。
///
/// egui は、同じ文字について 1 回しか聞かない。見つけたフォントは、それ以降の文字にも使われる。
struct OsFallback {
    /// 探すフォントのファイル（Windows のみ。順番どおり）。
    #[cfg_attr(unix, allow(dead_code))]
    files: Vec<PathBuf>,
    /// 読んだ文字の一覧（cmap）。読めなかったフォントは `None`。
    cmaps: Mutex<CmapCache>,
}

/// (ファイル, TTC 内の番号) ごとの文字の一覧。
type CmapCache = HashMap<(PathBuf, u32), Option<Arc<Cmap>>>;

impl OsFallback {
    fn new(base_name: &str) -> Self {
        Self {
            files: Self::fallback_files(base_name),
            cmaps: Mutex::default(),
        }
    }

    #[cfg(windows)]
    fn fallback_files(base_name: &str) -> Vec<PathBuf> {
        const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\FontLink\SystemLink";
        let dir = windows_fonts_dir();
        let mut files: Vec<PathBuf> = Vec::new();
        // 日本語フォントの一覧のあとに、Segoe UI（欧文の UI フォント）の一覧も探す（タイ文字の Tahoma などが入っている）。
        let linked = [base_name, "Segoe UI"]
            .into_iter()
            .filter(|n| !n.is_empty())
            .flat_map(|n| sc_core::platform::registry_multi_string(KEY, n));
        // 一覧に無い文字のための、Windows に標準で入っているフォント
        // （欧文・記号・インドの文字・タイ文字・アフリカの文字・カナダ先住民の文字）。
        let standard = [
            "segoeui.ttf",
            "seguisym.ttf",
            "Nirmala.ttc",
            "LeelawUI.ttf",
            "ebrima.ttf",
            "gadugi.ttf",
        ]
        .map(String::from);
        // 一覧の行は「ファイル名,フォント名[,大きさの調整…]」。
        for entry in linked.chain(standard) {
            let file = entry.split(',').next().unwrap_or_default().trim();
            if file.is_empty() {
                continue;
            }
            let path = if file.contains(['\\', '/']) {
                PathBuf::from(file)
            } else {
                dir.join(file)
            };
            if !files
                .iter()
                .any(|f| f.as_os_str().eq_ignore_ascii_case(path.as_os_str()))
            {
                files.push(path);
            }
        }
        files
    }

    #[cfg(unix)]
    fn fallback_files(_base_name: &str) -> Vec<PathBuf> {
        Vec::new()
    }

    /// そのフォントの文字の一覧（読んだものは覚えておく）。
    fn cmap(&self, path: &PathBuf, index: u32) -> Option<Arc<Cmap>> {
        let mut cmaps = self.cmaps.lock().unwrap_or_else(|e| e.into_inner());
        cmaps
            .entry((path.clone(), index))
            .or_insert_with(|| read_cmap(path, index).ok().map(Arc::new))
            .clone()
    }

    /// 文字 `c` を描けるフォントの (ファイル, TTC 内の番号)。
    #[cfg(windows)]
    fn find(&self, c: char) -> Option<(PathBuf, u32)> {
        for path in &self.files {
            let faces = face_count(path).unwrap_or(0);
            for index in 0..faces {
                if self.cmap(path, index).is_some_and(|m| m.has(c)) {
                    return Some((path.clone(), index));
                }
            }
        }
        None
    }

    #[cfg(unix)]
    fn find(&self, c: char) -> Option<(PathBuf, u32)> {
        let (path, index) = fc_match(&format!("sans-serif:charset={:x}", u32::from(c)))?;
        // fontconfig は、その文字が無くてもいちばん近いフォントを返すので、確かめる。
        self.cmap(&path, index)
            .is_some_and(|m| m.has(c))
            .then_some((path, index))
    }
}

impl FontProvider for OsFallback {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let c = request.cluster.chars().next()?;
        if c.is_control() || c.is_whitespace() {
            return None;
        }
        let (path, index) = self.find(c)?;
        let bytes = sc_core::platform::map_file_static(&path)?;
        let mut data = FontData::from_static(bytes);
        data.index = index;
        // 見つけたフォントは、両方の書体（プロポーショナルと等幅）の最後に加える。
        let families = [FontFamily::Proportional, FontFamily::Monospace]
            .into_iter()
            .map(|family| InsertFontFamily {
                family,
                priority: FontPriority::Lowest,
            })
            .collect();
        Some(FontInsert {
            name: format!("os:{}:{index}", path.display()),
            data,
            families,
        })
    }
}

/// フォントファイルの中のフォントの数（TTC なら複数、TTF・OTF なら 1）。
#[cfg_attr(unix, allow(dead_code))]
fn face_count(path: &PathBuf) -> std::io::Result<u32> {
    let mut f = File::open(path)?;
    let head = read_at(&mut f, 0, 12)?;
    Ok(if &head[..4] == b"ttcf" {
        be32(&head, 8)
    } else {
        1
    })
}

/// フォントの文字の一覧（OpenType の cmap の表のうち、Unicode の部分）。
struct Cmap {
    format: u16,
    data: Vec<u8>,
}

impl Cmap {
    /// 文字 `c` の字形があるか。
    fn has(&self, c: char) -> bool {
        let c = u32::from(c);
        match self.format {
            4 => c <= 0xFFFF && format4_glyph(&self.data, c as u16) != 0,
            12 => format12_glyph(&self.data, c) != 0,
            _ => false,
        }
    }
}

/// フォントファイルから cmap を読む。ファイル全体は読まず、必要なところだけを読む。
fn read_cmap(path: &PathBuf, index: u32) -> std::io::Result<Cmap> {
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "cmap を読めない");
    let mut f = File::open(path)?;
    let head = read_at(&mut f, 0, 12)?;
    let offset = if &head[..4] == b"ttcf" {
        if index >= be32(&head, 8) {
            return Err(bad());
        }
        be32(&read_at(&mut f, 12 + 4 * u64::from(index), 4)?, 0)
    } else {
        0
    };
    let tables = be16(&read_at(&mut f, u64::from(offset), 12)?, 4);
    let dir = read_at(&mut f, u64::from(offset) + 12, 16 * usize::from(tables))?;
    let (cmap_offset, cmap_len) = dir
        .as_chunks::<16>()
        .0
        .iter()
        .find(|r| &r[..4] == b"cmap")
        .map(|r| (be32(r, 8), be32(r, 12)))
        .ok_or_else(bad)?;
    let cmap = read_at(&mut f, u64::from(cmap_offset), cmap_len as usize)?;

    // Unicode の表を選ぶ（全部の文字を含む format 12 を優先し、無ければ BMP だけの format 4）。
    let mut best: Option<(u16, usize)> = None;
    for i in 0..usize::from(be16(&cmap, 2)) {
        let rec = 4 + 8 * i;
        let (platform, encoding) = (be16(&cmap, rec), be16(&cmap, rec + 2));
        let unicode = platform == 0 || (platform == 3 && matches!(encoding, 1 | 10));
        let sub = be32(&cmap, rec + 4) as usize;
        if !unicode || sub + 2 > cmap.len() {
            continue;
        }
        let format = be16(&cmap, sub);
        if format == 12 || (format == 4 && best.is_none()) {
            best = Some((format, sub));
        }
    }
    let (format, sub) = best.ok_or_else(bad)?;
    let len = if format == 12 {
        be32(&cmap, sub + 4) as usize
    } else {
        usize::from(be16(&cmap, sub + 2))
    };
    let data = cmap.get(sub..sub + len).ok_or_else(bad)?.to_vec();
    Ok(Cmap { format, data })
}

/// format 4（BMP の文字）で、文字の字形の番号を引く（0 は無い）。
fn format4_glyph(t: &[u8], c: u16) -> u16 {
    if t.len() < 14 {
        return 0;
    }
    let segs = usize::from(be16(t, 6) / 2);
    let ends = 14;
    let starts = ends + 2 * segs + 2;
    let deltas = starts + 2 * segs;
    let range_offsets = deltas + 2 * segs;
    if t.len() < range_offsets + 2 * segs {
        return 0;
    }
    // 終わりの文字の昇順に並んでいるので、二分探索する。
    let (mut lo, mut hi) = (0, segs);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if be16(t, ends + 2 * mid) < c {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == segs || be16(t, starts + 2 * lo) > c {
        return 0;
    }
    let delta = be16(t, deltas + 2 * lo);
    let ro = usize::from(be16(t, range_offsets + 2 * lo));
    if ro == 0 {
        return c.wrapping_add(delta);
    }
    let at = range_offsets + 2 * lo + ro + 2 * usize::from(c - be16(t, starts + 2 * lo));
    match t.get(at..at + 2) {
        Some(_) => match be16(t, at) {
            0 => 0,
            g => g.wrapping_add(delta),
        },
        None => 0,
    }
}

/// format 12（すべての文字）で、文字の字形の番号を引く（0 は無い）。
fn format12_glyph(t: &[u8], c: u32) -> u32 {
    if t.len() < 16 {
        return 0;
    }
    let groups = (be32(t, 12) as usize).min((t.len() - 16) / 12);
    let (mut lo, mut hi) = (0, groups);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let g = 16 + 12 * mid;
        if be32(t, g + 4) < c {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let g = 16 + 12 * lo;
    if lo == groups || be32(t, g) > c {
        return 0;
    }
    be32(t, g + 8) + (c - be32(t, g))
}

fn read_at(f: &mut File, pos: u64, len: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(pos))?;
    let mut buf = vec![0; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn be16(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2)
        .map_or(0, |s| u16::from_be_bytes([s[0], s[1]]))
}

fn be32(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4)
        .map_or(0, |s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// format 4 の表を作る（区間は (始め, 終わり, delta)）。最後に 0xFFFF の区間が要る。
    fn format4(segs: &[(u16, u16, u16)]) -> Vec<u8> {
        let n = segs.len() as u16;
        let mut t = vec![0, 4, 0, 0, 0, 0];
        t.extend((n * 2).to_be_bytes());
        t.extend([0; 6]);
        segs.iter().for_each(|s| t.extend(s.1.to_be_bytes()));
        t.extend([0, 0]);
        segs.iter().for_each(|s| t.extend(s.0.to_be_bytes()));
        segs.iter().for_each(|s| t.extend(s.2.to_be_bytes()));
        segs.iter().for_each(|_| t.extend([0, 0]));
        t
    }

    #[test]
    fn format4_lookup() {
        let t = format4(&[
            (0x41, 0x5A, 1),
            (0x3042, 0x3093, 0x1000),
            (0xFFFF, 0xFFFF, 1),
        ]);
        let m = Cmap { format: 4, data: t };
        assert!(m.has('A') && m.has('Z') && m.has('あ'));
        assert!(!m.has('a') && !m.has('한') && !m.has('😀'));
    }

    #[test]
    fn format12_lookup() {
        let mut t = vec![0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
        for (s, e, g) in [(0x41u32, 0x5Au32, 1u32), (0x1F600, 0x1F64F, 100)] {
            t.extend(s.to_be_bytes());
            t.extend(e.to_be_bytes());
            t.extend(g.to_be_bytes());
        }
        let m = Cmap {
            format: 12,
            data: t,
        };
        assert!(m.has('A') && m.has('😀'));
        assert!(!m.has('a') && !m.has('あ'));
    }

    /// 実際の OS のフォントで確かめる（フォントが無い環境では確かめない）。
    #[cfg(windows)]
    #[test]
    fn windows_fonts() {
        let dir = windows_fonts_dir();
        let yu = dir.join("YuGothM.ttc");
        if let Ok(m) = read_cmap(&yu, 1) {
            assert!(m.has('あ') && m.has('漢') && !m.has('한'));
        }
        let malgun = dir.join("malgun.ttf");
        if let Ok(m) = read_cmap(&malgun, 0) {
            assert!(m.has('한'));
        }
        // 游ゴシックに無いハングルは、OS の一覧から韓国語のフォントが見つかる。
        if yu.exists() && malgun.exists() {
            let fb = OsFallback::new("Yu Gothic UI");
            let (path, index) = fb.find('한').expect("ハングルのフォントが見つからない");
            assert!(read_cmap(&path, index).is_ok_and(|m| m.has('한')));
            // どのフォントにも無い文字（割り当てのない私用領域の文字）は見つからない。
            assert!(fb.find('\u{10FFFD}').is_none());
        }
    }
}
