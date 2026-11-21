//! 端末のプログレスバー（標準エラー出力の下の方に進捗の行を描き、ほかの出力はその上に流す）。
//!
//! 行は「全体の進捗」（ファイルが 2 つ以上のとき）と「今のジョブ」（2 行）。
//! 進捗が分からないあいだは、回るマーク（スピナー）を描く。描き直しは 1 秒に 20 回まで。

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;

/// スピナーの絵。
const SPINNER: [char; 8] = ['⠁', '⠂', '⠄', '⡀', '⢀', '⠠', '⠐', '⠈'];
/// スピナーを回す間隔（描き直しの間隔でもある）。
const TICK: Duration = Duration::from_millis(120);
/// 進捗が来たときに描き直す最短の間隔。
const MIN_REDRAW: Duration = Duration::from_millis(50);
/// バーの目盛りの絵（満・8 分の 7 … 8 分の 1）。
const BAR_PARTS: [char; 8] = ['█', '▉', '▊', '▋', '▌', '▍', '▎', '▏'];

const CYAN: &str = "\x1b[36m";
const BLUE: &str = "\x1b[34m";
const RESET: &str = "\x1b[0m";

/// プログレスバー。端末のときだけ作る。
pub struct Bars {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct State {
    /// 全体の進捗（済んだ数, 全体の数, 始めた時刻）。
    overall: Option<(usize, usize, Instant)>,
    job: Option<Job>,
    frame: usize,
    /// 今、画面に描いてある行の数。
    drawn: usize,
    last_draw: Option<Instant>,
}

struct Job {
    prefix: String,
    message: String,
    /// 進捗率（0〜1）。分からなければ `None`（スピナーを描く）。
    ratio: Option<f32>,
}

impl Bars {
    /// `total` はファイルの数（2 つ以上なら全体の進捗も描く）。
    pub fn new(total: usize) -> Self {
        sc_core::platform::enable_ansi_stderr();
        let state = Arc::new(Mutex::new(State {
            overall: (total > 1).then(|| (0, total, Instant::now())),
            ..State::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::Builder::new()
                .name("progress".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(TICK);
                        let mut s = lock(&state);
                        s.frame = s.frame.wrapping_add(1);
                        s.redraw();
                    }
                })
                .ok()
        };
        Self {
            state,
            stop,
            ticker,
        }
    }

    /// 新しいジョブを始める（進捗は分からない状態から）。
    pub fn start_job(&self, prefix: &str) {
        let mut s = lock(&self.state);
        s.job = Some(Job {
            prefix: prefix.to_owned(),
            message: String::new(),
            ratio: None,
        });
        s.redraw();
    }

    /// ジョブの 1 行目（ファイル名とコマンド）を変え、進捗を分からない状態に戻す。
    pub fn set_prefix(&self, prefix: String) {
        let mut s = lock(&self.state);
        if let Some(job) = &mut s.job {
            job.prefix = prefix;
            job.message.clear();
            job.ratio = None;
        }
        s.redraw();
    }

    /// 進捗率（分かるとき）と、2 行目の説明を変える。
    pub fn set_progress(&self, ratio: Option<f32>, message: String) {
        let mut s = lock(&self.state);
        if let Some(job) = &mut s.job {
            if ratio.is_some() {
                job.ratio = ratio;
            }
            job.message = message;
        }
        if s.last_draw.is_none_or(|t| t.elapsed() >= MIN_REDRAW) {
            s.redraw();
        }
    }

    /// ジョブを終える（ジョブの行を消し、全体の進捗を 1 つ進める）。
    pub fn finish_job(&self) {
        let mut s = lock(&self.state);
        s.job = None;
        if let Some((done, ..)) = &mut s.overall {
            *done += 1;
        }
        s.redraw();
    }

    /// バーの上に 1 行出す（`stdout` が true なら標準出力に）。
    pub fn println(&self, line: &str, stdout: bool) {
        let mut s = lock(&self.state);
        s.clear();
        if stdout {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        } else {
            eprintln!("{line}");
        }
        s.redraw();
    }

    /// すべての行を消して終える。
    pub fn finish(mut self) {
        self.stop_ticker();
        let mut s = lock(&self.state);
        s.overall = None;
        s.job = None;
        s.clear();
    }

    fn stop_ticker(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.ticker.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Bars {
    fn drop(&mut self) {
        self.stop_ticker();
        lock(&self.state).clear();
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

impl State {
    /// 描いてある行を消す（カーソルを上げて、そこから下を消す）。
    fn clear(&mut self) {
        if self.drawn > 0 {
            eprint!("\r\x1b[{}A\x1b[J", self.drawn);
            self.drawn = 0;
        }
    }

    fn redraw(&mut self) {
        let width = sc_core::platform::stderr_terminal_width().unwrap_or(80);
        let lines = self.lines(width);
        let mut buf = String::new();
        if self.drawn > 0 {
            buf.push_str(&format!("\r\x1b[{}A\x1b[J", self.drawn));
        }
        for line in &lines {
            buf.push_str(line);
            buf.push('\n');
        }
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(buf.as_bytes());
        let _ = err.flush();
        self.drawn = lines.len();
        self.last_draw = Some(Instant::now());
    }

    /// 描く行（端末の幅で切り詰める。折り返すと消すときの行数がずれるため）。
    fn lines(&self, width: usize) -> Vec<String> {
        let width = width.saturating_sub(1).max(20);
        let mut lines = Vec::new();
        if let Some((done, total, start)) = self.overall {
            let ratio = done as f32 / total.max(1) as f32;
            let head = format!("全体 [{done}/{total}] ");
            let tail = format!(
                " {:>3}%  経過 {}",
                (ratio * 100.0) as u32,
                hms(start.elapsed())
            );
            lines.push(fit(&format!("{head}{}{tail}", bar(ratio, 24)), width));
        }
        if let Some(job) = &self.job {
            lines.push(fit(&job.prefix, width));
            let head = match job.ratio {
                Some(r) => format!("     [{}] {:>3}%  ", bar(r, 30), (r * 100.0) as u32),
                None => format!("     {CYAN}{}{RESET} ", SPINNER[self.frame % SPINNER.len()]),
            };
            lines.push(fit(&format!("{head}{}", job.message), width));
        }
        lines
    }
}

/// 色付きのバー（済んだところは水色、残りは青）。
fn bar(ratio: f32, width: usize) -> String {
    let eighths = (ratio.clamp(0.0, 1.0) * (width * 8) as f32) as usize;
    let (full, part) = (eighths / 8, eighths % 8);
    let mut s = String::from(CYAN);
    s.extend(std::iter::repeat_n(BAR_PARTS[0], full));
    let mut used = full;
    if part > 0 && used < width {
        s.push(BAR_PARTS[8 - part]);
        used += 1;
    }
    s.push_str(BLUE);
    s.extend(std::iter::repeat_n(' ', width - used));
    s.push_str(RESET);
    s
}

/// 表示幅が `width` を超えないように末尾を切る（色の制御文字は幅に数えない）。
fn fit(line: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ESC [ … m は幅 0 としてそのまま写す
            out.push(c);
            for x in chars.by_ref() {
                out.push(x);
                if x.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        let cw = c.width().unwrap_or(0);
        if w + cw > width {
            out.push_str(RESET);
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// 時:分:秒（`00:01:23`）。
fn hms(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// 色の制御文字を除いた表示幅。
    fn visible_width(s: &str) -> usize {
        sc_core::text::strip_ansi(s).width()
    }

    #[test]
    fn bar_has_fixed_width() {
        for r in [0.0, 0.01, 0.37, 0.5, 0.999, 1.0, 1.5] {
            assert_eq!(visible_width(&bar(r, 24)), 24, "{r}");
        }
        assert!(bar(0.5, 4).starts_with(&format!("{CYAN}██{BLUE}")));
        // 8 分の 1 単位で描く
        assert!(bar(1.0 / 32.0, 4).contains('▏'));
        assert!(bar(7.0 / 32.0, 4).contains('▉'));
    }

    #[test]
    fn fit_cuts_by_display_width() {
        assert_eq!(fit("abc", 10), "abc");
        assert_eq!(visible_width(&fit("あいうえお", 5)), 4);
        let colored = format!("{CYAN}abcdef{RESET}");
        assert_eq!(visible_width(&fit(&colored, 3)), 3);
    }

    #[test]
    fn lines_for_job_and_overall() {
        let mut s = State {
            overall: Some((1, 3, Instant::now())),
            ..State::default()
        };
        s.job = Some(Job {
            prefix: "動画.ts  コマンド 1/2 ffmpeg".into(),
            message: "経過 00:05".into(),
            ratio: None,
        });
        let lines = s.lines(80);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("全体 [1/3] "));
        assert!(lines[0].contains(" 33%  経過 00:00:0"));
        assert_eq!(lines[1], "動画.ts  コマンド 1/2 ffmpeg");
        assert!(sc_core::text::strip_ansi(&lines[2]).starts_with("     ⠁ 経過"));

        s.job.as_mut().unwrap().ratio = Some(0.42);
        let lines = s.lines(80);
        assert!(sc_core::text::strip_ansi(&lines[2]).contains("]  42%  経過 00:05"));
        // 狭い端末では切り詰める
        for line in s.lines(30) {
            assert!(visible_width(&line) <= 29, "{line}");
        }
    }
}
