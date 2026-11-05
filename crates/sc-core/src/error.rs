//! sc-core のエラー型。

use std::path::PathBuf;

/// sc-core が返すエラー。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path} を読み込めません: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} に書き込めません: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: プリセットがありません（[Item0] が見つかりません）")]
    NoPresets { path: PathBuf },

    #[error("コマンドを解釈できません: {0}")]
    CommandLine(String),
}

/// sc-core の Result。
pub type Result<T> = std::result::Result<T, Error>;
