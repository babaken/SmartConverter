//! SmartConverter の GUI（`SmartConverter`）。
//!
//! 画面の仕様は `docs/screen-design.md`。

// リリースビルドではコンソールウィンドウを出さない（Windows）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod diag;
mod fonts;
mod log_window;
mod notify;
mod options;
mod os;
mod preset_editor;
mod queue;
mod tray;
mod worker;

use std::path::PathBuf;

use clap::Parser;
use eframe::egui;

/// ウィンドウ・タスクバー用のアイコン（`cargo xtask icons` で生成した PNG）。
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon/png/smartconverter-256.png");

/// 起動引数（本家互換：`SmartConverter -S "タイトル" ファイル…`）。
#[derive(Debug, Parser)]
#[command(name = "SmartConverter", version, about = "SmartConverter")]
struct Args {
    /// [ItemN] の N でプリセットを指定
    #[arg(
        short = 'p',
        long = "preset",
        value_name = "N",
        conflicts_with = "preset_title"
    )]
    preset: Option<usize>,
    /// タイトルでプリセットを指定
    #[arg(short = 'S', long = "preset-title", value_name = "TITLE")]
    preset_title: Option<String>,
    /// 出力先フォルダ
    #[arg(short = 'o', long = "output-dir", value_name = "DIR")]
    output_dir: Option<PathBuf>,
    /// 変換するファイル（渡すとすぐ変換を始める）
    files: Vec<PathBuf>,
}

fn main() -> eframe::Result {
    let args = Args::parse();
    if let Some(app_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
    {
        diag::init(&sc_core::settings::default_path(&app_dir));
    }
    diag::op(format!(
        "起動 {} {}（{}）",
        sc_core::APP_NAME,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    ));
    let startup = app::StartupArgs {
        preset_index: args.preset,
        preset_title: args.preset_title,
        output_dir: args.output_dir,
        files: args.files,
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(sc_core::APP_NAME)
        .with_app_id("smartconverter")
        .with_inner_size([820.0, 560.0])
        .with_min_inner_size([640.0, 420.0])
        .with_drag_and_drop(true);
    if let Some((pos, size)) = saved_window() {
        viewport = viewport.with_inner_size(size);
        // Wayland ではアプリが位置を決められないので、大きさだけを復元する（基本設計 §9.1）。
        if !is_wayland() {
            viewport = viewport.with_position(pos);
        }
    }
    // アイコンが読めなくても起動は続ける。
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON_PNG) {
        viewport = viewport.with_icon(icon);
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    // Linux：Wayland のセッションでもネイティブの Wayland で開くのが既定。
    // ただし winit 0.30（egui 0.37 が使う版）は Wayland でファイルのドロップを受け取れないので、
    // SMARTCONVERTER_X11=1 のときは X11（XWayland）で開いてドロップを使えるようにする。
    // winit 0.31 で Wayland のドロップに対応する予定（egui が上げたら、この切り替えは要らなくなる）。
    #[cfg(target_os = "linux")]
    if !use_wayland() {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }));
    }
    let result = eframe::run_native(
        sc_core::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, startup, ICON_PNG)))),
    );
    match &result {
        Ok(()) => diag::op("終了"),
        Err(e) => diag::op(format!("終了（エラー：{e}）")),
    }
    result
}

/// 前回のウィンドウの位置と大きさ（設定の `MainWindow`）。
fn saved_window() -> Option<(egui::Pos2, egui::Vec2)> {
    let app_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let settings =
        sc_core::settings::Settings::load(&sc_core::settings::default_path(&app_dir)).ok()?;
    let v: Vec<f32> = settings
        .main_window
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    match v.as_slice() {
        [x, y, w, h] if *w >= 640.0 && *h >= 420.0 => {
            Some((egui::pos2(*x, *y), egui::vec2(*w, *h)))
        }
        _ => None,
    }
}

/// Wayland のウィンドウで動いているか（X11 / XWayland なら false）。
pub(crate) fn is_wayland() -> bool {
    cfg!(unix) && use_wayland()
}

/// Linux で Wayland のウィンドウを使うか。Wayland のセッションでは既定で Wayland。
/// `SMARTCONVERTER_X11=1` で、X11 のディスプレイ（XWayland）があれば X11 にする。
fn use_wayland() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()) {
        return false;
    }
    let want_x11 = std::env::var("SMARTCONVERTER_X11").is_ok_and(|v| v == "1");
    let has_x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    !(want_x11 && has_x11)
}
