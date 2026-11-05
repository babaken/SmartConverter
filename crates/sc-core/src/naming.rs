//! タイトル・出力ファイル名・一時ファイル名の決定（要件 §4.7、F-QU-10、F-EX-07〜08）。
//!
//! タイムスタンプの書式は本家（Delphi の FormatDateTime）互換。`docs/original-behavior.md` §2 を参照。

use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDateTime, Timelike};

/// タイムスタンプを付ける位置（`AddTimePos`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampPos {
    /// 0：先頭（1 字空け）
    HeadWithSpace,
    /// 2：先頭
    Head,
    /// 1・3：末尾。本家では付かない（バグ）ため、SmartConverter でも付けない
    TailIgnored,
}

impl TimestampPos {
    pub fn from_ini(value: u8) -> Self {
        match value {
            0 => Self::HeadWithSpace,
            2 => Self::Head,
            _ => Self::TailIgnored,
        }
    }

    pub fn to_ini(self) -> u8 {
        match self {
            Self::HeadWithSpace => 0,
            Self::Head => 2,
            Self::TailIgnored => 1,
        }
    }
}

/// タイムスタンプの設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampOptions {
    pub enabled: bool,
    pub pos: TimestampPos,
    pub format: String,
}

/// 画面に出す書式の候補（本家と同じ 6 つ）。
pub const FORMAT_CANDIDATES: [&str; 6] = [
    "[mm-dd]",
    "[mm-dd hh-mm]",
    "[yy-mm-dd]",
    "[yy-mm-dd hh-mm]",
    "[yy-mm-dd hh-mm-ss]",
    "[yyyymmdd]",
];

/// タイトル（`<%Title%>`）を作る：入力ファイル名（拡張子なし）にタイムスタンプを付け、
/// ファイル名に使えない文字を置き換える。
pub fn build_title(input_base: &str, ts: &TimestampOptions, mtime: &NaiveDateTime) -> String {
    let title = if ts.enabled {
        let stamp = format_datetime(&ts.format, mtime);
        match ts.pos {
            TimestampPos::HeadWithSpace => format!("{stamp} {input_base}"),
            TimestampPos::Head => format!("{stamp}{input_base}"),
            TimestampPos::TailIgnored => input_base.to_owned(),
        }
    } else {
        input_base.to_owned()
    };
    sanitize_file_name(&title)
}

/// `<%TimeStamp%>` の値（書式は固定）。
pub fn timestamp_value(mtime: &NaiveDateTime) -> String {
    mtime.format("%Y-%m-%d %H:%M:%S").to_string()
}

const SHORT_DAY: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];
const LONG_DAY: [&str; 7] = [
    "日曜日",
    "月曜日",
    "火曜日",
    "水曜日",
    "木曜日",
    "金曜日",
    "土曜日",
];

/// Delphi の FormatDateTime 互換の書式で日時を文字列にする（日本語ロケール）。
///
/// 対応：`y yy yyy yyyy`、`m mm mmm mmmm`（`h`/`hh` の直後の `m`/`mm` は分）、`d dd ddd dddd ddddd dddddd`、
/// `h hh`、`n nn`、`s ss`、`z zzz`、`t tt`、`c`、`am/pm` `a/p` `ampm`、`/`、`:`、`"…"` `'…'`。
/// 書式の文字は大文字小文字を区別しない。
pub fn format_datetime(format: &str, dt: &NaiveDateTime) -> String {
    let chars: Vec<char> = format.chars().collect();
    let twelve_hour = {
        let upper = format.to_uppercase();
        upper.contains("AM/PM") || upper.contains("A/P") || upper.contains("AMPM")
    };
    let hour = |dt: &NaiveDateTime| {
        if twelve_hour {
            match dt.hour() % 12 {
                0 => 12,
                h => h,
            }
        } else {
            dt.hour()
        }
    };

    let mut out = String::new();
    let mut last_was_hour = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let upper = c.to_ascii_uppercase();
        let run = chars[i..]
            .iter()
            .take_while(|x| x.to_ascii_uppercase() == upper)
            .count();
        match upper {
            '"' | '\'' => {
                let end = chars[i + 1..]
                    .iter()
                    .position(|&x| x == c)
                    .map_or(chars.len(), |p| i + 1 + p);
                out.extend(&chars[i + 1..end]);
                i = (end + 1).min(chars.len());
                continue;
            }
            'Y' => {
                if run <= 2 {
                    out.push_str(&format!("{:02}", dt.year() % 100));
                } else {
                    out.push_str(&format!("{:04}", dt.year()));
                }
                last_was_hour = false;
            }
            'M' => {
                if last_was_hour && run <= 2 {
                    push_num(&mut out, dt.minute(), run);
                } else {
                    match run {
                        1 | 2 => push_num(&mut out, dt.month(), run),
                        _ => out.push_str(&format!("{}月", dt.month())),
                    }
                }
                last_was_hour = false;
            }
            'D' => {
                let wd = dt.weekday().num_days_from_sunday() as usize;
                match run {
                    1 | 2 => push_num(&mut out, dt.day(), run),
                    3 => out.push_str(SHORT_DAY[wd]),
                    4 => out.push_str(LONG_DAY[wd]),
                    5 => out.push_str(&dt.format("%Y/%m/%d").to_string()),
                    _ => out.push_str(&format!("{}年{}月{}日", dt.year(), dt.month(), dt.day())),
                }
                last_was_hour = false;
            }
            'H' => {
                push_num(&mut out, hour(dt), run.min(2));
                last_was_hour = true;
            }
            'N' => {
                push_num(&mut out, dt.minute(), run.min(2));
                last_was_hour = false;
            }
            'S' => {
                push_num(&mut out, dt.second(), run.min(2));
                last_was_hour = false;
            }
            'Z' => {
                let ms = dt.nanosecond() / 1_000_000 % 1000;
                if run >= 3 {
                    out.push_str(&format!("{ms:03}"));
                } else {
                    out.push_str(&ms.to_string());
                }
                last_was_hour = false;
            }
            'T' => {
                if run >= 2 {
                    out.push_str(&format!(
                        "{}:{:02}:{:02}",
                        dt.hour(),
                        dt.minute(),
                        dt.second()
                    ));
                } else {
                    out.push_str(&format!("{}:{:02}", dt.hour(), dt.minute()));
                }
                last_was_hour = false;
            }
            'C' => {
                out.push_str(&dt.format("%Y/%m/%d").to_string());
                if dt.time() != chrono::NaiveTime::MIN {
                    out.push_str(&format!(
                        " {}:{:02}:{:02}",
                        dt.hour(),
                        dt.minute(),
                        dt.second()
                    ));
                }
                last_was_hour = false;
                i += run;
                continue;
            }
            'A' => {
                let rest: String = chars[i..].iter().collect::<String>().to_uppercase();
                let pm = dt.hour() >= 12;
                let (text, len) = if rest.starts_with("AM/PM") {
                    (if pm { "PM" } else { "AM" }, 5)
                } else if rest.starts_with("A/P") {
                    (if pm { "P" } else { "A" }, 3)
                } else if rest.starts_with("AMPM") {
                    (if pm { "午後" } else { "午前" }, 4)
                } else {
                    out.push(c);
                    i += 1;
                    continue;
                };
                out.push_str(text);
                i += len;
                continue;
            }
            _ => {
                // `/` と `:` は日本語ロケールの区切り文字（同じ文字）、それ以外はそのまま。
                out.push(c);
                i += 1;
                continue;
            }
        }
        i += run;
    }
    out
}

fn push_num(out: &mut String, value: u32, width: usize) {
    if width >= 2 {
        out.push_str(&format!("{value:02}"));
    } else {
        out.push_str(&value.to_string());
    }
}

/// ファイル名に使えない文字を `_` に置き換える（要件 F-FN-04）。
pub fn sanitize_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            let bad = if cfg!(windows) {
                c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            } else {
                c == '/' || c == '\0'
            };
            if bad {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cfg!(windows) {
        // Windows では末尾のピリオドと空白がファイル名から落ちるので、先に除いておく。
        replaced.trim_end_matches(['.', ' ']).to_owned()
    } else {
        replaced
    }
}

/// 同名のファイルがあるときの扱い（`OnExists`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnExists {
    /// 連番を付ける（既定）
    Rename,
    /// 上書きする
    Overwrite,
    /// スキップする
    Skip,
}

impl OnExists {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "rename" => Some(Self::Rename),
            "overwrite" => Some(Self::Overwrite),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::Overwrite => "Overwrite",
            Self::Skip => "Skip",
        }
    }
}

/// 出力名の決定結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputDecision {
    /// このタイトルで出力する（連番を付けたときは変わる）
    Use { title: String, renamed: bool },
    /// 同名のファイルがあるのでスキップする
    Skip,
}

/// 出力先に「タイトル.拡張子」のファイルがすでにあるかを調べ、出力に使うタイトルを決める。
///
/// 出力の拡張子はプリセットのコマンドで決まるため、拡張子を問わず同じ名前（拡張子なし）のファイルを探す。
pub fn decide_output(dir: &Path, title: &str, on_exists: OnExists) -> OutputDecision {
    let exists = |t: &str| stem_exists(dir, t);
    if on_exists == OnExists::Overwrite || !exists(title) {
        return OutputDecision::Use {
            title: title.to_owned(),
            renamed: false,
        };
    }
    match on_exists {
        OnExists::Skip => OutputDecision::Skip,
        _ => {
            let mut n = 1;
            loop {
                let candidate = format!("{title} ({n})");
                if !exists(&candidate) {
                    return OutputDecision::Use {
                        title: candidate,
                        renamed: true,
                    };
                }
                n += 1;
            }
        }
    }
}

fn stem_exists(dir: &Path, title: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let prefix = format!("{title}.");
    entries.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if cfg!(windows) {
            name.to_lowercase().starts_with(&prefix.to_lowercase())
        } else {
            name.starts_with(&prefix)
        }
    })
}

/// 一時ファイル名（拡張子なし）を作る：`<dir>/<名前>/<名前>`、名前は `sc_<pid>_<jobid>_<乱数12桁>`。
///
/// 一時ファイルはジョブ専用のフォルダ（実行時に、本人だけが使えるフォルダとして新しく作る）に置く。
/// 共有の一時フォルダ（Linux の /tmp）で、ほかのユーザーが同じ名前のファイルやシンボリックリンクを
/// 先に置いて、書き込み先をすり替えるのを防ぐため。
pub fn temporary_base(dir: &Path, job_id: u64) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    // RandomState はプロセスごとに OS の乱数で初期化される。
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(job_id);
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    let rand = h.finish() & 0xFFFF_FFFF_FFFF;
    let name = format!("sc_{}_{}_{rand:012x}", std::process::id(), job_id);
    dir.join(&name).join(name)
}

/// 一時ファイルを置くジョブ専用のフォルダを作る（`temporary_base` の親）。
///
/// すでにあれば失敗する（だれかが先に作ったフォルダは使わない）。Linux では本人だけが使える（0700）。
pub fn create_temporary_dir(temporary_base: &Path) -> std::io::Result<PathBuf> {
    let dir = temporary_base
        .parent()
        .ok_or_else(|| std::io::Error::other("一時フォルダの場所が正しくありません"))?;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    Ok(dir.to_owned())
}

/// 前に強制終了されたときに残った、ジョブ専用の一時フォルダを片付ける。消した数を返す。
///
/// 対象は `dir` の直下にある `sc_<pid>_<jobid>_<乱数12桁>` の形のフォルダのうち、
/// そのプロセスがもう動いていないもの（ほかの SmartConverter が使っているものは消さない）。
/// シンボリックリンクは対象にしない。Linux では自分が持ち主のフォルダだけにする（共有の /tmp 対策）。
/// `min_age` を渡すと、フォルダの更新からその時間以上たったものだけを消す（共有フォルダ対策）。
pub fn remove_stale_temporary_dirs(dir: &Path, min_age: Option<std::time::Duration>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(temporary_dir_pid) else {
            continue;
        };
        // シンボリックリンクはたどらずに調べる
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        if !meta.is_dir() || !is_mine(&meta) {
            continue;
        }
        if let Some(age) = min_age {
            let old_enough = meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|e| e >= age);
            if !old_enough {
                continue;
            }
        }
        if pid == std::process::id() || crate::platform::process_exists(pid) {
            continue;
        }
        if std::fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `sc_<pid>_<jobid>_<乱数12桁>` ならプロセス ID を返す。
fn temporary_dir_pid(name: &str) -> Option<u32> {
    let mut parts = name.strip_prefix("sc_")?.split('_');
    let pid = parts.next()?.parse().ok()?;
    parts.next()?.parse::<u64>().ok()?;
    let rand = parts.next()?;
    let ok =
        parts.next().is_none() && rand.len() == 12 && rand.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then_some(pid)
}

#[cfg(unix)]
fn is_mine(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid は常に成功する。
    meta.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn is_mine(_meta: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(h: u32, m: u32, s: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2024, 3, 5)
            .unwrap()
            .and_hms_milli_opt(h, m, s, 45)
            .unwrap()
    }

    #[test]
    fn candidates_match_original() {
        // docs/original-behavior.md §2.1（秒は丸めない）
        let t = dt(7, 8, 9);
        let expect = [
            "[03-05]",
            "[03-05 07-08]",
            "[24-03-05]",
            "[24-03-05 07-08]",
            "[24-03-05 07-08-09]",
            "[20240305]",
        ];
        for (fmt, want) in FORMAT_CANDIDATES.iter().zip(expect) {
            assert_eq!(format_datetime(fmt, &t), want, "format {fmt}");
        }
    }

    #[test]
    fn custom_format_with_weekday() {
        assert_eq!(
            format_datetime("yyyy_mm_dd(ddd) hh_nn_ss", &dt(7, 8, 9)),
            "2024_03_05(火) 07_08_09"
        );
        assert_eq!(format_datetime("dddd", &dt(7, 8, 9)), "火曜日");
        assert_eq!(format_datetime("m/d h:n:s", &dt(7, 8, 9)), "3/5 7:8:9");
        assert_eq!(format_datetime("\"yy\"yy 'mm'", &dt(7, 8, 9)), "yy24 mm");
        assert_eq!(format_datetime("hh:nn zzz", &dt(7, 8, 9)), "07:08 045");
        assert_eq!(format_datetime("h am/pm", &dt(19, 0, 0)), "7 PM");
        assert_eq!(format_datetime("YYYY-MM-DD", &dt(7, 8, 9)), "2024-03-05");
    }

    #[test]
    fn no_rounding_at_end_of_day() {
        assert_eq!(
            format_datetime("[yy-mm-dd hh-mm-ss]", &dt(23, 59, 59)),
            "[24-03-05 23-59-59]"
        );
        assert_eq!(timestamp_value(&dt(23, 59, 59)), "2024-03-05 23:59:59");
    }

    #[test]
    fn title_positions() {
        let t = dt(7, 8, 9);
        let opts = |enabled, pos| TimestampOptions {
            enabled,
            pos,
            format: "[mm-dd]".into(),
        };
        assert_eq!(
            build_title("テスト 動画", &opts(true, TimestampPos::HeadWithSpace), &t),
            "[03-05] テスト 動画"
        );
        assert_eq!(
            build_title("テスト 動画", &opts(true, TimestampPos::Head), &t),
            "[03-05]テスト 動画"
        );
        assert_eq!(
            build_title("テスト 動画", &opts(true, TimestampPos::TailIgnored), &t),
            "テスト 動画"
        );
        assert_eq!(
            build_title("テスト 動画", &opts(false, TimestampPos::Head), &t),
            "テスト 動画"
        );
        assert_eq!(TimestampPos::from_ini(3), TimestampPos::TailIgnored);
    }

    #[cfg(windows)]
    #[test]
    fn sanitize_windows() {
        assert_eq!(sanitize_file_name("a:b/c*d?."), "a_b_c_d_");
    }

    #[test]
    fn decide_output_variants() {
        let dir = std::env::temp_dir().join(format!("sc_naming_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("movie.mp4"), b"").unwrap();
        std::fs::write(dir.join("movie (1).mp4"), b"").unwrap();

        assert_eq!(
            decide_output(&dir, "other", OnExists::Rename),
            OutputDecision::Use {
                title: "other".into(),
                renamed: false
            }
        );
        assert_eq!(
            decide_output(&dir, "movie", OnExists::Rename),
            OutputDecision::Use {
                title: "movie (2)".into(),
                renamed: true
            }
        );
        assert_eq!(
            decide_output(&dir, "movie", OnExists::Skip),
            OutputDecision::Skip
        );
        assert_eq!(
            decide_output(&dir, "movie", OnExists::Overwrite),
            OutputDecision::Use {
                title: "movie".into(),
                renamed: false
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn temporary_names_are_unique() {
        let dir = Path::new("tmp");
        let a = temporary_base(dir, 1);
        let b = temporary_base(dir, 2);
        assert_ne!(a, b);
        assert!(a
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(&format!("sc_{}_1_", std::process::id())));
        // ジョブ専用のフォルダの中
        assert_eq!(a.parent().unwrap().parent().unwrap(), dir);
        assert_eq!(a.parent().unwrap().file_name(), a.file_name());
    }

    #[test]
    fn stale_temporary_dirs_are_removed() {
        let root = std::env::temp_dir().join(format!("sc_stale_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // 動いていないプロセス（PID は大きな値）の一時フォルダ：中身ごと消える
        let dead = root.join("sc_4000000000_1_0123456789ab");
        std::fs::create_dir_all(&dead).unwrap();
        std::fs::write(dead.join("sc_4000000000_1_0123456789ab.wav"), b"x").unwrap();
        // 自分（動いている）の一時フォルダ：残る
        let live = root.join(format!("sc_{}_1_0123456789ab", std::process::id()));
        std::fs::create_dir_all(&live).unwrap();
        // 形の違うもの：残る
        let other = root.join("sc_4000000000_1_xyz");
        std::fs::create_dir_all(&other).unwrap();
        let file = root.join("sc_4000000000_2_0123456789ab");
        std::fs::write(&file, b"x").unwrap();

        // 共有フォルダ用の条件（1 日）では、作ったばかりなので消えない
        assert_eq!(
            remove_stale_temporary_dirs(&root, Some(std::time::Duration::from_secs(86_400))),
            0
        );
        assert!(dead.exists());
        assert_eq!(remove_stale_temporary_dirs(&root, None), 1);
        assert!(!dead.exists());
        assert!(live.exists());
        assert!(other.exists());
        assert!(file.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn temporary_dir_names() {
        assert_eq!(temporary_dir_pid("sc_123_4_0123456789ab"), Some(123));
        assert_eq!(temporary_dir_pid("sc_123_4_0123456789"), None);
        assert_eq!(temporary_dir_pid("sc_123_4_0123456789ab_x"), None);
        assert_eq!(temporary_dir_pid("sc_cmdline_test_1"), None);
        let base = temporary_base(Path::new("t"), 7);
        let name = base.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(temporary_dir_pid(&name), Some(std::process::id()));
    }

    #[test]
    fn temporary_dir_is_new_and_private() {
        let root = std::env::temp_dir().join(format!("sc_tmpdir_test_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let base = temporary_base(&root, 1);
        let dir = create_temporary_dir(&base).unwrap();
        assert!(dir.is_dir());
        // 2 回目（だれかが先に作った場合）は失敗する
        assert!(create_temporary_dir(&base).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
