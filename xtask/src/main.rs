//! 開発用の補助タスク。`cargo xtask <タスク>` で実行する。
//!
//! - `icons`：`assets/icon/*.svg` から PNG と ICO を作る
//! - `dist` ：配布物（Windows は zip、Linux は tar.gz）を `dist/` に作る。`--skip-build` でビルドを省く
//! - `public`：公開リポジトリ用のファイル一式を作る（`public/` の設定に従う）

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

mod dist;
mod public;

/// 16〜32px は簡略版、それ以上は通常版の SVG を使う。
const SMALL_MAX: u32 = 32;
const PNG_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512];
const ICO_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("icons") => icons(),
        Some("dist") => dist::run(&args[1..]),
        Some("public") => public::run(&args[1..]),
        _ => {
            eprintln!("使い方: cargo xtask <icons|dist [--skip-build]|public <出力先> [--ref <リビジョン>]>");
            std::process::exit(2);
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask はワークスペース直下にある")
        .to_path_buf()
}

fn icons() -> Result<()> {
    let dir = workspace_root().join("assets").join("icon");
    let full = load_svg(&dir.join("smartconverter.svg"))?;
    let small = load_svg(&dir.join("smartconverter-small.svg"))?;
    let tree_for = |size: u32| if size <= SMALL_MAX { &small } else { &full };

    let png_dir = dir.join("png");
    fs::create_dir_all(&png_dir)?;
    for &size in PNG_SIZES {
        let pixmap = render(tree_for(size), size)?;
        let path = png_dir.join(format!("smartconverter-{size}.png"));
        pixmap
            .save_png(&path)
            .with_context(|| format!("{} を書き出せません", path.display()))?;
        println!("書き出し: {}", path.display());
    }

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in ICO_SIZES {
        let pixmap = render(tree_for(size), size)?;
        // tiny-skia は乗算済みアルファなので、ICO 用に元に戻す。
        let rgba: Vec<u8> = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    let ico_path = dir.join("smartconverter.ico");
    let file = fs::File::create(&ico_path)
        .with_context(|| format!("{} を作れません", ico_path.display()))?;
    icon_dir.write(file)?;
    println!("書き出し: {}", ico_path.display());
    Ok(())
}

fn load_svg(path: &Path) -> Result<usvg::Tree> {
    let data = fs::read(path).with_context(|| format!("{} を読めません", path.display()))?;
    usvg::Tree::from_data(&data, &usvg::Options::default())
        .with_context(|| format!("{} を解析できません", path.display()))
}

fn render(tree: &usvg::Tree, size: u32) -> Result<tiny_skia::Pixmap> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("画像領域を確保できません")?;
    let svg = tree.size();
    let transform =
        tiny_skia::Transform::from_scale(size as f32 / svg.width(), size as f32 / svg.height());
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}
