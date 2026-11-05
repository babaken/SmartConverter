//! CLI での変換の実行とプログレスバー（要件 F-CL-09〜12、基本設計 §2.4）。

use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use sc_core::plan::{self, PlanOptions};
use sc_core::preset::Preset;
use sc_core::progress::ProgressUpdate;
use sc_core::runner::{self, JobOutcome, RunOptions, RunnerEvent};
use sc_core::script::{ScriptOptions, ScriptSupport};
use sc_core::settings::Settings;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 終了コード（要件 F-CL-11）。
pub const EXIT_OK: u8 = 0;
pub const EXIT_FAILED: u8 = 1;
pub const EXIT_ABORTED: u8 = 130;

/// 端末でないときに進捗を出す間隔。
const PLAIN_INTERVAL: Duration = Duration::from_secs(10);
/// プログレスバーの目盛り（進捗率 × これ）。
const BAR_SCALE: u64 = 1000;
/// ファイル名の表示幅の上限（全角は 2 桁）。
const NAME_WIDTH: usize = 40;

pub struct Convert<'a> {
    pub inputs: &'a [PathBuf],
    pub preset: &'a Preset,
    pub output_dir: &'a Path,
    pub settings: &'a Settings,
    pub app_dir: &'a Path,
    pub quiet: bool,
    pub verbose: bool,
    pub log: Option<&'a Path>,
    pub abort: Arc<AtomicBool>,
}

impl Convert<'_> {
    pub fn run(&self) -> Result<u8> {
        std::fs::create_dir_all(self.output_dir).with_context(|| {
            format!("出力先フォルダを作れません: {}", self.output_dir.display())
        })?;
        // 前に強制終了されたときに残った一時フォルダを片付ける
        let stale = self.settings.remove_stale_temporary_dirs();
        if stale > 0 && !self.quiet {
            eprintln!("前回残った一時フォルダを {stale} 件削除しました");
        }
        let temp_dir = self.settings.temporary_dir();
        std::fs::create_dir_all(&temp_dir)
            .with_context(|| format!("一時フォルダを作れません: {}", temp_dir.display()))?;

        let mut log = match self.log {
            Some(p) => Some(
                File::options()
                    .create(true)
                    .append(true)
                    .open(p)
                    .with_context(|| format!("ログファイルを開けません: {}", p.display()))?,
            ),
            None => None,
        };

        let path_env = std::env::var_os("PATH");
        let support =
            ScriptSupport::detect(&self.settings.tools, self.app_dir, path_env.as_deref());
        let popts = PlanOptions {
            output_dir: self.output_dir,
            temporary_dir: &temp_dir,
            on_exists: self.settings.on_exists,
            timestamp: &self.settings.timestamp,
            tools: &self.settings.tools,
            app_dir: self.app_dir,
            path_env: path_env.clone(),
            script: script_options(self.settings, &support),
        };
        let ropts = RunOptions {
            priority: self.settings.priority,
            tools: self.settings.tools.clone(),
            app_dir: self.app_dir.to_owned(),
            path_env,
            delete_partial: self.settings.delete_partial,
        };

        let fancy = !self.quiet && std::io::stderr().is_terminal();
        let mut ui = Ui::new(fancy, self.quiet, self.verbose, self.inputs.len());
        let total = self.inputs.len();
        let mut failed = 0;
        let mut aborted = false;

        for (i, input) in self.inputs.iter().enumerate() {
            if self.abort.load(Ordering::SeqCst) {
                aborted = true;
                break;
            }
            let name = display_name(input);
            let mtime = plan::file_mtime(input);
            let job = plan::plan_job(input, &mtime, self.preset, i as u64 + 1, &popts);
            ui.start_job(i, &name);
            write_log(
                &mut log,
                &format!(
                    "━━ [{}/{total}] {} [{}]",
                    i + 1,
                    input.display(),
                    self.preset.title
                ),
            );

            let outcome = runner::run_job(&job, &ropts, &self.abort, &mut |event| {
                log_event(&mut log, &event);
                ui.event(i, &name, &event);
            });
            ui.finish_job();

            let line = match &outcome {
                JobOutcome::Succeeded { outputs } => {
                    let list = if outputs.is_empty() {
                        "（出力ファイルが見つかりません）".to_owned()
                    } else {
                        outputs
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    format!("OK    {} -> {list}", input.display())
                }
                JobOutcome::Failed(reason) => {
                    failed += 1;
                    format!("NG    {} ({reason})", input.display())
                }
                JobOutcome::Skipped => {
                    format!("SKIP  {} (同名のファイルがあります)", input.display())
                }
                JobOutcome::Aborted => {
                    aborted = true;
                    format!("ABORT {}", input.display())
                }
            };
            write_log(&mut log, &line);
            ui.result(&line);
            if aborted {
                break;
            }
        }
        ui.done();

        Ok(if aborted {
            EXIT_ABORTED
        } else if failed > 0 {
            EXIT_FAILED
        } else {
            EXIT_OK
        })
    }
}

/// 進捗の表示（端末ならプログレスバー、そうでなければ 10 秒ごとの 1 行）。
struct Ui {
    multi: Option<MultiProgress>,
    overall: Option<ProgressBar>,
    bar: Option<ProgressBar>,
    quiet: bool,
    verbose: bool,
    total: usize,
    last_plain: Instant,
    cmd: (usize, usize, String),
    determinate: bool,
}

impl Ui {
    fn new(fancy: bool, quiet: bool, verbose: bool, total: usize) -> Self {
        let multi = fancy.then(|| MultiProgress::with_draw_target(ProgressDrawTarget::stderr()));
        let overall = match (&multi, total > 1) {
            (Some(m), true) => {
                let pb = m.add(ProgressBar::new(total as u64));
                pb.set_style(
                    ProgressStyle::with_template(
                        "全体 [{pos}/{len}] {bar:24.cyan/blue} {percent:>3}%  経過 {elapsed_precise}",
                    )
                    .unwrap()
                    .progress_chars("█▉▊▋▌▍▎▏ "),
                );
                Some(pb)
            }
            _ => None,
        };
        Self {
            multi,
            overall,
            bar: None,
            quiet,
            verbose,
            total,
            last_plain: Instant::now(),
            cmd: (0, 0, String::new()),
            determinate: false,
        }
    }

    fn start_job(&mut self, _index: usize, name: &str) {
        self.cmd = (0, 0, String::new());
        self.determinate = false;
        self.last_plain = Instant::now();
        if let Some(m) = &self.multi {
            let pb = m.add(ProgressBar::new(BAR_SCALE));
            pb.set_prefix(name.to_owned());
            self.set_spinner(&pb);
            self.bar = Some(pb);
        }
    }

    fn set_spinner(&self, pb: &ProgressBar) {
        pb.set_style(
            ProgressStyle::with_template("{prefix}\n     {spinner:.cyan} {wide_msg}").unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(120));
    }

    fn set_determinate(&mut self) {
        if let Some(pb) = &self.bar {
            pb.disable_steady_tick();
            pb.set_style(
                ProgressStyle::with_template(
                    "{prefix}\n     [{bar:30.cyan/blue}] {percent:>3}%  {wide_msg}",
                )
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏ "),
            );
        }
        self.determinate = true;
    }

    fn event(&mut self, index: usize, name: &str, event: &RunnerEvent) {
        match event {
            RunnerEvent::CommandStarted {
                index: i,
                count,
                tool,
                ..
            } => {
                self.cmd = (*i + 1, *count, tool.clone());
                if let Some(pb) = &self.bar {
                    pb.set_prefix(format!("{name}  コマンド {}/{count} {tool}", i + 1));
                    pb.set_position(0);
                    pb.set_message(String::new());
                }
                if self.determinate {
                    // 次のコマンドは進捗が分かるとは限らない
                    if let Some(pb) = self.bar.clone() {
                        self.set_spinner(&pb);
                    }
                    self.determinate = false;
                }
            }
            RunnerEvent::Progress {
                update, elapsed, ..
            } => {
                let msg = progress_message(update, *elapsed);
                if let Some(r) = update.ratio {
                    if !self.determinate {
                        self.set_determinate();
                    }
                    if let Some(pb) = &self.bar {
                        pb.set_position((r as f64 * BAR_SCALE as f64) as u64);
                    }
                }
                if let Some(pb) = &self.bar {
                    pb.set_message(msg.clone());
                }
                if self.multi.is_none()
                    && !self.quiet
                    && self.last_plain.elapsed() >= PLAIN_INTERVAL
                {
                    self.last_plain = Instant::now();
                    let percent = update
                        .ratio
                        .map(|r| format!("{:.0}%", r * 100.0))
                        .unwrap_or_else(|| "--%".into());
                    eprintln!(
                        "[{}/{}] {name} コマンド {}/{} {} {percent} {msg}",
                        index + 1,
                        self.total,
                        self.cmd.0,
                        self.cmd.1,
                        self.cmd.2
                    );
                }
            }
            RunnerEvent::Output { line, .. } if self.verbose => self.print_err(line),
            RunnerEvent::Warning(w) => self.print_err(&format!("⚠ {w}")),
            RunnerEvent::Info(s) if self.verbose => self.print_err(&format!("ⓘ {s}")),
            RunnerEvent::Notice(s) => self.print_err(&format!("ⓘ {s}")),
            _ => {}
        }
    }

    fn print_err(&self, line: &str) {
        match &self.multi {
            Some(m) => {
                let _ = m.println(line);
            }
            None => eprintln!("{line}"),
        }
    }

    fn finish_job(&mut self) {
        if let Some(pb) = self.bar.take() {
            pb.finish_and_clear();
            if let Some(m) = &self.multi {
                m.remove(&pb);
            }
        }
        if let Some(o) = &self.overall {
            o.inc(1);
        }
    }

    fn result(&self, line: &str) {
        match &self.multi {
            Some(m) => m.suspend(|| println!("{line}")),
            None => println!("{line}"),
        }
    }

    fn done(&self) {
        if let Some(o) = &self.overall {
            o.finish_and_clear();
        }
    }
}

/// スケルトン機能の設定（無効にしているか、テンプレートが無ければ `None`）。
pub fn script_options<'a>(
    settings: &'a Settings,
    support: &'a ScriptSupport,
) -> Option<ScriptOptions<'a>> {
    (!settings.script_disabled && !settings.script_templates.is_empty()).then_some(ScriptOptions {
        templates: &settings.script_templates,
        support,
    })
}

fn progress_message(u: &ProgressUpdate, elapsed: Duration) -> String {
    let mut parts = Vec::new();
    match (u.frames, u.total_frames) {
        (Some(f), Some(t)) => parts.push(format!("{f}/{t} frames")),
        (Some(f), None) => parts.push(format!("{f} frames")),
        _ => {}
    }
    if let Some(fps) = u.fps.filter(|f| *f > 0.0) {
        parts.push(format!("{fps:.0} fps"));
    }
    parts.push(format!("経過 {}", fmt_duration(elapsed)));
    if let Some(eta) = u.eta {
        parts.push(format!("残り {}", fmt_duration(eta)));
    }
    if let Some(size) = u.estimated_size {
        parts.push(format!("予想 {:.0} MB", size as f64 / 1024.0 / 1024.0));
    }
    parts.join("  ")
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// ファイル名を表示幅で切り詰める（途中を「…」にする）。
fn display_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    truncate_middle(&name, NAME_WIDTH)
}

fn truncate_middle(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_owned();
    }
    let half = (max - 1) / 2;
    let mut head = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > half {
            break;
        }
        head.push(c);
        w += cw;
    }
    let mut tail: Vec<char> = Vec::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max - 1 - half {
            break;
        }
        tail.push(c);
        w += cw;
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}

fn write_log(log: &mut Option<File>, line: &str) {
    if let Some(f) = log {
        let now = chrono_now();
        let _ = writeln!(f, "{now} {line}");
    }
}

fn log_event(log: &mut Option<File>, event: &RunnerEvent) {
    if log.is_none() {
        return;
    }
    let line = match event {
        RunnerEvent::CommandStarted {
            index,
            count,
            display,
            ..
        } => {
            format!("▶ [{}/{count}] {display}", index + 1)
        }
        // `\r` の上書き行はファイルには書かない（最後の値だけで十分）
        RunnerEvent::Output {
            replace_last: true, ..
        } => return,
        RunnerEvent::Output { line, .. } => format!("  {line}"),
        RunnerEvent::Info(s) | RunnerEvent::Notice(s) => format!("ⓘ {s}"),
        RunnerEvent::Warning(s) => format!("⚠ {s}"),
        RunnerEvent::CommandFinished {
            index,
            exit_code,
            elapsed,
        } => format!(
            "{} [{}] 終了コード {} （{}）",
            if *exit_code == Some(0) { "✓" } else { "✗" },
            index + 1,
            exit_code.map_or("なし".to_owned(), |c| c.to_string()),
            fmt_duration(*elapsed)
        ),
        RunnerEvent::Progress { .. } => return,
    };
    write_log(log, &line);
}

/// ログ用の現在時刻（ローカル時刻）。
fn chrono_now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate_middle("short.ts", 40), "short.ts");
        let long = "あいうえおかきくけこさしすせそたちつてと.ts";
        let t = truncate_middle(long, 20);
        assert!(t.width() <= 20, "{t} {}", t.width());
        assert!(t.contains('…'));
        assert!(t.ends_with(".ts"));
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(Duration::from_secs(192)), "03:12");
        assert_eq!(fmt_duration(Duration::from_secs(3725)), "1:02:05");
    }
}
