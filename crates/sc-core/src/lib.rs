//! SmartConverter の中核ライブラリ。
//!
//! GUI に依存しない処理（INI の読み書き、置換変数、コマンド実行、進捗解析など）をまとめる。
//! モジュール構成は `docs/basic-design.md` §2.3 を参照。

pub mod cmdline;
pub mod error;
pub mod ini;
pub mod naming;
pub mod paths;
pub mod plan;
pub mod platform;
pub mod preset;
pub mod progress;
pub mod runner;
pub mod script;
pub mod settings;
pub mod text;
pub mod vars;

pub use error::{Error, Result};

/// アプリケーション名。
pub const APP_NAME: &str = "SmartConverter";

/// ワークスペースのバージョン（`Cargo.toml` の `workspace.package.version`）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 画面やログに出す「名前 バージョン」の文字列を返す。
pub fn app_title() -> String {
    format!("{APP_NAME} {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_title_contains_name_and_version() {
        let title = app_title();
        assert!(title.starts_with(APP_NAME));
        assert!(title.ends_with(VERSION));
    }
}
