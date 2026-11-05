//! 1 ジョブ分のコマンド列を実行する（基本設計 §4.1 の 5〜7、要件 §4.4）。
//!
//! 呼び出し側（CLI・GUI）には、イベントをコールバックで渡す。GUI はこれを別スレッドで動かし、
//! チャネルで画面スレッドへ送る。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::cmdline;
use crate::naming;
use crate::plan::JobPlan;
use crate::platform::{self, ProcessTree};
use crate::progress::{self, ProgressUpdate};
use crate::text;

/// 実行の設定。
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// 子プロセスの優先度（0〜3）
    pub priority: u8,
    pub tools: BTreeMap<String, String>,
    pub app_dir: PathBuf,
    pub path_env: Option<OsString>,
    /// 中断したとき、このジョブで新しくできた出力（途中までのファイル）を消す
    pub delete_partial: bool,
}

/// 出力の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// 実行中のイベント。
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerEvent {
    /// コマンドを開始した（`display` は展開後のコマンド全文）
    CommandStarted {
        index: usize,
        count: usize,
        tool: String,
        display: String,
    },
    /// ツールの出力 1 行。`replace_last` が true なら直前の行の上書き（`\r` で終わった行）
    Output {
        stream: Stream,
        line: String,
        replace_last: bool,
    },
    /// アプリからのお知らせ
    Info(String),
    /// 必ず伝えるお知らせ（CLI でも `-v` なしで出す。途中までの出力を消したときなど）
    Notice(String),
    /// アプリからの警告
    Warning(String),
    /// 進捗（残り時間・予想サイズは、ツールが出さなければアプリが計算して埋める）
    Progress {
        index: usize,
        count: usize,
        tool: String,
        update: ProgressUpdate,
        elapsed: Duration,
    },
    /// コマンドが終わった
    CommandFinished {
        index: usize,
        exit_code: Option<i32>,
        elapsed: Duration,
    },
}

/// ジョブの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// すべてのコマンドが成功した（`outputs` は出力先にできたファイル）
    Succeeded { outputs: Vec<PathBuf> },
    /// 失敗した（理由）
    Failed(String),
    /// 中断した
    Aborted,
    /// 同名ファイルがあるためスキップした
    Skipped,
}

/// 中断を求められてから、強制終了するまでの待ち時間（Linux の SIGTERM → SIGKILL）。
const KILL_GRACE: Duration = Duration::from_secs(3);
/// 子プロセスの終了後、出力を読み切るのを待つ時間（孫プロセスがパイプを持ち続ける場合の対策）。
const DRAIN_WAIT: Duration = Duration::from_secs(1);
/// 進捗イベントの最短間隔。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// 予想サイズを自分で計算するときの間隔（フォルダを読むため控えめにする）。
const SIZE_SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// ジョブを実行する。`abort` が true になったら中断する。
pub fn run_job(
    plan: &JobPlan,
    opts: &RunOptions,
    abort: &AtomicBool,
    on_event: &mut dyn FnMut(RunnerEvent),
) -> JobOutcome {
    let Some(output) = &plan.output else {
        on_event(RunnerEvent::Info(
            "同名のファイルがあるためスキップしました".into(),
        ));
        return JobOutcome::Skipped;
    };
    // 出力ファイルの一覧・予想サイズに、前からあった同じ名前のファイル（別の拡張子）を入れないよう、
    // 始める前にあったものを覚えておく（変わっていないものは、このジョブの出力ではない）。
    let before = Existing::scan(&output.base);

    // 一時ファイルを置くジョブ専用のフォルダ（naming::temporary_base）。
    let temp_dir = match naming::create_temporary_dir(&plan.temporary_base) {
        Ok(d) => d,
        Err(e) => {
            let dir = plan.temporary_base.parent().unwrap_or(Path::new(""));
            return JobOutcome::Failed(format!(
                "一時フォルダを作れません：{}（{e}）",
                dir.display()
            ));
        }
    };

    let outcome = run_commands(plan, opts, abort, &before, on_event);

    // 成功・失敗・中断のどれでも一時ファイルを消す（要件 F-EX-07）。
    let removed = remove_temporary_files(&plan.temporary_base);
    if removed > 0 {
        on_event(RunnerEvent::Info(format!(
            "一時ファイルを {removed} 件削除しました"
        )));
    }
    // ツールが別の名前で作ったものも、フォルダごと消す（自分で新しく作ったフォルダなので安全）。
    if let Err(e) = std::fs::remove_dir_all(&temp_dir) {
        on_event(RunnerEvent::Warning(format!(
            "一時フォルダを削除できません：{}（{e}）",
            temp_dir.display()
        )));
    }

    if opts.delete_partial && matches!(outcome, Err(JobOutcome::Aborted)) {
        remove_partial_outputs(&output.base, &before, on_event);
    }

    match outcome {
        Ok(()) => JobOutcome::Succeeded {
            outputs: files_with_stem(&output.base, &before),
        },
        Err(o) => o,
    }
}

fn run_commands(
    plan: &JobPlan,
    opts: &RunOptions,
    abort: &AtomicBool,
    before: &Existing,
    on_event: &mut dyn FnMut(RunnerEvent),
) -> Result<(), JobOutcome> {
    for w in &plan.warnings {
        on_event(RunnerEvent::Warning(w.clone()));
    }
    if let Some(script) = &plan.script {
        write_script(script, on_event)?;
    }
    let count = plan.commands.len();
    for (index, planned) in plan.commands.iter().enumerate() {
        if abort.load(Ordering::SeqCst) {
            return Err(JobOutcome::Aborted);
        }
        for u in &planned.unknown_vars {
            on_event(RunnerEvent::Warning(format!(
                "未定義の変数 {u} は置き換えずに実行します"
            )));
        }

        let parsed =
            cmdline::parse(&planned.expanded).map_err(|e| JobOutcome::Failed(e.to_string()))?;
        let tool = cmdline::tool_name(&parsed.program);
        let program = cmdline::resolve_program(
            &parsed.program,
            &opts.tools,
            &opts.app_dir,
            opts.path_env.as_deref(),
        )
        .ok_or_else(|| JobOutcome::Failed(format!("ツールが見つかりません: {}", parsed.program)))?;

        on_event(RunnerEvent::CommandStarted {
            index,
            count,
            tool: tool.clone(),
            display: planned.expanded.clone(),
        });

        let mut cmd = Command::new(&program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if !parsed.raw_args.is_empty() {
                cmd.raw_arg(&parsed.raw_args);
            }
        }
        #[cfg(not(windows))]
        cmd.args(&parsed.args);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(w) = platform::configure(&mut cmd, opts.priority) {
            on_event(RunnerEvent::Warning(w));
        }

        let started = Instant::now();
        let mut child = cmd.spawn().map_err(|e| {
            JobOutcome::Failed(format!(
                "{tool} を起動できません（{}）: {e}",
                program.display()
            ))
        })?;
        let tree = ProcessTree::attach(&mut child).map_err(|e| {
            JobOutcome::Failed(format!(
                "{tool} を起動できません（{}）: {e}",
                program.display()
            ))
        })?;

        let (tx, rx) = mpsc::channel::<Msg>();
        let readers = [
            child
                .stdout
                .take()
                .map(|p| spawn_reader(p, Stream::Stdout, tx.clone())),
            child
                .stderr
                .take()
                .map(|p| spawn_reader(p, Stream::Stderr, tx.clone())),
        ];
        drop(tx);

        let writes_output = writes_output_file(&planned.template);
        let mut parser = progress::parser_for(&tool, &planned.expanded);
        let mut last_progress = Instant::now() - PROGRESS_INTERVAL;
        let mut last_scan: Option<Instant> = None;
        let mut scanned_size: u64 = 0;
        let mut abort_at: Option<Instant> = None;
        let mut exited_at: Option<Instant> = None;
        let mut status = None;
        let mut open_streams = readers.iter().filter(|r| r.is_some()).count();

        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Msg::Line(stream, bytes, replace_last)) => {
                    let line = text::strip_ansi(&text::decode_line(&bytes));
                    let line = line.trim_end();
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Some(mut update) = parser.feed(line) {
                        let elapsed = started.elapsed();
                        fill_estimates(&mut update, elapsed);
                        // 予想サイズは、出力ファイル（<%OutputFile%>）を書くコマンドのときだけ出す。
                        // 途中のコマンド（WAV への書き出しなど）の大きさは、できあがるファイルの大きさと関係がないため。
                        if !writes_output {
                            update.estimated_size = None;
                        } else if update.estimated_size.is_none()
                            && update.ratio.is_some_and(|r| r > 0.05)
                        {
                            if last_scan.is_none_or(|t| t.elapsed() >= SIZE_SCAN_INTERVAL) {
                                scanned_size = output_size(plan, before);
                                last_scan = Some(Instant::now());
                            }
                            if scanned_size > 0 {
                                update.estimated_size = update
                                    .ratio
                                    .map(|r| (scanned_size as f64 / r as f64) as u64);
                            }
                        }
                        if last_progress.elapsed() >= PROGRESS_INTERVAL {
                            last_progress = Instant::now();
                            on_event(RunnerEvent::Progress {
                                index,
                                count,
                                tool: tool.clone(),
                                update,
                                elapsed,
                            });
                        }
                    }
                    on_event(RunnerEvent::Output {
                        stream,
                        line: line.to_owned(),
                        replace_last,
                    });
                }
                Ok(Msg::Eof) => open_streams = open_streams.saturating_sub(1),
                Err(mpsc::RecvTimeoutError::Timeout)
                | Err(mpsc::RecvTimeoutError::Disconnected) => {}
            }

            if abort.load(Ordering::SeqCst) && abort_at.is_none() {
                on_event(RunnerEvent::Info(format!("{tool} を中断しています…")));
                tree.terminate();
                abort_at = Some(Instant::now());
            }
            if let Some(t) = abort_at {
                if t.elapsed() >= KILL_GRACE {
                    tree.kill();
                }
            }

            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(s)) => {
                        status = Some(s);
                        exited_at = Some(Instant::now());
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tree.kill();
                        return Err(JobOutcome::Failed(format!(
                            "{tool} の状態を取得できません: {e}"
                        )));
                    }
                }
            }
            if let Some(t) = exited_at {
                if open_streams == 0 || t.elapsed() >= DRAIN_WAIT {
                    break;
                }
            }
        }
        // 孫プロセスが残っていれば、ここで止める（Windows は tree がスコープを抜けて
        // Job Object が閉じられたときにも止まる）。
        tree.kill();

        let elapsed = started.elapsed();
        let exit_code = status.and_then(|s| s.code());
        on_event(RunnerEvent::CommandFinished {
            index,
            exit_code,
            elapsed,
        });

        if abort_at.is_some() {
            return Err(JobOutcome::Aborted);
        }
        if !status.is_some_and(|s| s.success()) {
            let code = exit_code.map_or("なし".to_owned(), |c| c.to_string());
            return Err(JobOutcome::Failed(format!(
                "{tool} が終了コード {code} で終了しました（コマンド {}/{count}）",
                index + 1
            )));
        }
        on_event(RunnerEvent::Progress {
            index,
            count,
            tool: tool.clone(),
            update: ProgressUpdate {
                ratio: Some(1.0),
                eta: Some(Duration::ZERO),
                ..Default::default()
            },
            elapsed,
        });
    }
    Ok(())
}

enum Msg {
    Line(Stream, Vec<u8>, bool),
    Eof,
}

/// パイプを読み、`\r` / `\n` で区切って行ごとに送るスレッド。
fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    stream: Stream,
    tx: mpsc::Sender<Msg>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut line: Vec<u8> = Vec::new();
        let mut pending_cr = false;
        loop {
            let n = match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                if pending_cr {
                    pending_cr = false;
                    if b == b'\n' {
                        // \r\n は通常の改行
                        let _ = tx.send(Msg::Line(stream, std::mem::take(&mut line), false));
                        continue;
                    }
                    let _ = tx.send(Msg::Line(stream, std::mem::take(&mut line), true));
                }
                match b {
                    b'\r' => pending_cr = true,
                    b'\n' => {
                        let _ = tx.send(Msg::Line(stream, std::mem::take(&mut line), false));
                    }
                    _ => line.push(b),
                }
            }
        }
        if pending_cr || !line.is_empty() {
            let _ = tx.send(Msg::Line(stream, line, pending_cr));
        }
        let _ = tx.send(Msg::Eof);
    })
}

/// ツールが出さなかった残り時間を、経過時間と進捗率から計算する。
fn fill_estimates(update: &mut ProgressUpdate, elapsed: Duration) {
    if update.eta.is_none() {
        if let Some(r) = update.ratio.filter(|r| *r > 0.01) {
            let secs = elapsed.as_secs_f64() * (1.0 - r as f64) / r as f64;
            update.eta = Some(Duration::from_secs_f64(secs.max(0.0)));
        }
    }
}

/// 出力ファイル（このジョブで作られた・書き換えられた `出力名.*`）の今の合計サイズ（予想サイズの計算用）。
fn output_size(plan: &JobPlan, before: &Existing) -> u64 {
    plan.output
        .as_ref()
        .map(|o| {
            files_with_stem(&o.base, before)
                .iter()
                .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// コマンドが出力ファイル（`<%OutputFile%>`）を書くか（予想サイズを出すかどうかの判定）。
fn writes_output_file(template: &str) -> bool {
    template.to_ascii_lowercase().contains("<%outputfile%>")
}

/// `base` のファイル名で始まるファイル（`base.hevc`、`base.wav` など）。
fn files_with_prefix(base: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (base.parent(), base.file_name()) else {
        return Vec::new();
    };
    let prefix = name.to_string_lossy().into_owned();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .collect()
}

/// ジョブを始める前にあった `出力名.*` と、その更新日時・大きさ。
///
/// 前は「開始の 2 秒前より後に更新されたもの」を出力としていたが、直前のジョブが同じ名前で
/// 別の拡張子のファイルを作っていると、それも数えてしまった。時刻の余裕に頼らず、
/// 始める前の状態と比べる（上書きされたものは、更新日時か大きさが変わる）。
#[derive(Debug, Default)]
struct Existing(std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, u64)>);

impl Existing {
    fn scan(base: &Path) -> Self {
        Self(
            stem_files(base)
                .into_iter()
                .map(|p| {
                    let state = file_state(&p);
                    (p, state)
                })
                .collect(),
        )
    }

    /// 始める前からあったか。
    fn contains(&self, path: &Path) -> bool {
        self.0.contains_key(path)
    }

    /// 始める前と同じまま（このジョブで作られても書き換えられてもいない）か。
    fn unchanged(&self, path: &Path) -> bool {
        self.0.get(path).is_some_and(|s| *s == file_state(path))
    }
}

fn file_state(path: &Path) -> (Option<std::time::SystemTime>, u64) {
    std::fs::metadata(path).map_or((None, 0), |m| (m.modified().ok(), m.len()))
}

/// `base.拡張子` のファイル。
fn stem_files(base: &Path) -> Vec<PathBuf> {
    let name = base
        .file_name()
        .map(|n| format!("{}.", n.to_string_lossy()))
        .unwrap_or_default();
    files_with_prefix(base)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&name))
        })
        .collect()
}

/// `base.拡張子` のファイルのうち、このジョブで作られたか書き換えられたもの（出力ファイルの一覧用）。
fn files_with_stem(base: &Path, before: &Existing) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = stem_files(base)
        .into_iter()
        .filter(|p| !before.unchanged(p))
        .collect();
    list.sort();
    list
}

/// 中断したジョブの途中までの出力を消す（設定 `DeletePartial`）。
///
/// 消すのは、このジョブで新しくできた `出力名.*` だけ。前からあって書き換えられたもの
/// （上書きの設定のとき）は元に戻せないので、消さずに警告する。
fn remove_partial_outputs(base: &Path, before: &Existing, on_event: &mut dyn FnMut(RunnerEvent)) {
    for path in files_with_stem(base, before) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if before.contains(&path) {
            on_event(RunnerEvent::Warning(format!(
                "途中まで上書きされたファイルは残しました：{name}"
            )));
            continue;
        }
        match remove_file_retrying(&path) {
            Ok(()) => on_event(RunnerEvent::Notice(format!(
                "途中までの出力を削除しました：{name}"
            ))),
            Err(e) => on_event(RunnerEvent::Warning(format!(
                "途中までの出力を削除できません：{name}（{e}）"
            ))),
        }
    }
}

/// ファイルを消す。中断したツールの子プロセスが終わりきらずに開いたままのことがある
/// （Windows では消せない）ので、約 2 秒までやり直す。
fn remove_file_retrying(path: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound && tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            r => return r,
        }
    }
}

/// スケルトンからスクリプトを作る（要件 F-AV-02）。
fn write_script(
    script: &crate::script::PlannedScript,
    on_event: &mut dyn FnMut(RunnerEvent),
) -> Result<(), JobOutcome> {
    let content = script
        .content
        .as_ref()
        .map_err(|e| JobOutcome::Failed(e.clone()))?;
    for u in &script.unknown_vars {
        on_event(RunnerEvent::Warning(format!(
            "テンプレートの未定義の変数 {u} は置き換えずに書き出します"
        )));
    }
    for w in &script.warnings {
        on_event(RunnerEvent::Warning(w.clone()));
    }
    std::fs::write(&script.path, content).map_err(|e| {
        JobOutcome::Failed(format!(
            "スクリプトを作れません：{}（{e}）",
            script.path.display()
        ))
    })?;
    on_event(RunnerEvent::Info(format!(
        "{} のスクリプトを作りました：{}（テンプレート：{}）",
        script.kind.name(),
        script.path.display(),
        script.template.display()
    )));
    Ok(())
}

/// 一時ファイル（`base` で始まるファイル）を消して、消した数を返す。
pub fn remove_temporary_files(base: &Path) -> usize {
    files_with_prefix(base)
        .into_iter()
        .filter(|p| std::fs::remove_file(p).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::{OnExists, TimestampOptions, TimestampPos};
    use crate::plan::{plan_job, PlanOptions};
    use crate::preset::Preset;

    struct Env {
        dir: PathBuf,
        templates: Vec<crate::settings::ScriptTemplate>,
        delete_partial: bool,
    }

    impl Env {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sc_runner_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("out")).unwrap();
            std::fs::create_dir_all(dir.join("tmp")).unwrap();
            std::fs::write(dir.join("input.ts"), b"x").unwrap();
            Self {
                dir,
                templates: Vec::new(),
                delete_partial: true,
            }
        }

        fn run(
            &self,
            commands: &[&str],
            abort_after: Option<Duration>,
        ) -> (JobOutcome, Vec<RunnerEvent>) {
            let preset = Preset {
                extra: Vec::new(),
                index: 0,
                title: "T".into(),
                commands: commands.iter().map(|s| s.to_string()).collect(),
            };
            let ts = TimestampOptions {
                enabled: false,
                pos: TimestampPos::HeadWithSpace,
                format: String::new(),
            };
            let tools = BTreeMap::new();
            let out = self.dir.join("out");
            let tmp = self.dir.join("tmp");
            let support = crate::script::ScriptSupport::all_for_test();
            let popts = PlanOptions {
                output_dir: &out,
                temporary_dir: &tmp,
                on_exists: OnExists::Overwrite,
                timestamp: &ts,
                tools: &tools,
                app_dir: &self.dir,
                path_env: std::env::var_os("PATH"),
                script: (!self.templates.is_empty()).then_some(crate::script::ScriptOptions {
                    templates: &self.templates,
                    support: &support,
                }),
            };
            let mtime = crate::plan::file_mtime(&self.dir.join("input.ts"));
            let plan = plan_job(&self.dir.join("input.ts"), &mtime, &preset, 1, &popts);
            let ropts = RunOptions {
                priority: 1,
                tools: BTreeMap::new(),
                app_dir: self.dir.clone(),
                path_env: std::env::var_os("PATH"),
                delete_partial: self.delete_partial,
            };
            let abort = std::sync::Arc::new(AtomicBool::new(false));
            if let Some(after) = abort_after {
                let a = abort.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(after);
                    a.store(true, Ordering::SeqCst);
                });
            }
            let mut events = Vec::new();
            let outcome = run_job(&plan, &ropts, &abort, &mut |e| events.push(e));
            (outcome, events)
        }

        fn tmp_files(&self) -> usize {
            std::fs::read_dir(self.dir.join("tmp")).unwrap().count()
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(windows)]
    const OK_CMDS: [&str; 2] = [
        r#"cmd /c "echo hello& echo x> "<%TemporaryFile%>.tmp"""#,
        r#"cmd /c "copy /y "<%TemporaryFile%>.tmp" "<%OutputFile%>.txt" >nul""#,
    ];
    #[cfg(unix)]
    const OK_CMDS: [&str; 2] = [
        r#"sh -c 'echo hello; echo x > "$0.tmp"' "<%TemporaryFile%>""#,
        r#"sh -c 'cp "$0.tmp" "$1.txt"' "<%TemporaryFile%>" "<%OutputFile%>""#,
    ];

    #[test]
    fn outputs_exclude_files_from_before_the_job() {
        // 直前のジョブが同じ名前で別の拡張子のファイルを作った直後でも、それは数えない（PR #20 の Linux 確認）。
        let dir = std::env::temp_dir().join(format!("sc_runner_stem_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("One");
        std::fs::write(dir.join("One.mp4"), b"previous job").unwrap();
        std::fs::write(dir.join("One.txt"), b"old").unwrap();
        std::fs::write(dir.join("Other.m4a"), b"other").unwrap();

        let before = Existing::scan(&base);
        assert!(files_with_stem(&base, &before).is_empty());

        // このジョブが作ったもの・書き換えたものだけが出力
        std::fs::write(dir.join("One.m4a"), b"this job").unwrap();
        std::fs::write(dir.join("One.txt"), b"overwritten").unwrap();
        assert_eq!(
            files_with_stem(&base, &before),
            vec![dir.join("One.m4a"), dir.join("One.txt")]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn estimated_size_only_for_output_commands() {
        assert!(writes_output_file(
            r#""MP4Box" -add "<%TemporaryFile%>.hevc" -new "<%OutputFile%>.mp4""#
        ));
        assert!(writes_output_file(r#"ffmpeg -i x "<%outputfile%>.mp4""#));
        assert!(!writes_output_file(
            r#""ffmpeg" -i "<%InputFile%>" -vn "<%TemporaryFile%>.wav""#
        ));
        assert!(!writes_output_file(
            r#""qaac64" "<%TemporaryFile%>.wav" -o "<%TemporaryFile%>.m4a""#
        ));
    }

    #[test]
    fn success_runs_all_and_cleans_up() {
        let env = Env::new("ok");
        // 前からある同じ名前のファイル（別の拡張子）は、出力の一覧に入らない
        let old = env.dir.join("out").join("input.m4a");
        let f = std::fs::File::create(&old).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        drop(f);
        let (outcome, events) = env.run(&OK_CMDS, None);
        match &outcome {
            JobOutcome::Succeeded { outputs } => {
                assert_eq!(outputs, &vec![env.dir.join("out").join("input.txt")]);
            }
            other => panic!("{other:?} {events:#?}"),
        }
        assert!(events
            .iter()
            .any(|e| matches!(e, RunnerEvent::Output { line, .. } if line == "hello")));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    e,
                    RunnerEvent::CommandFinished {
                        exit_code: Some(0),
                        ..
                    }
                ))
                .count(),
            2
        );
        assert_eq!(env.tmp_files(), 0, "一時ファイルが残っている");
    }

    #[test]
    fn failure_stops_remaining_commands() {
        let env = Env::new("fail");
        #[cfg(windows)]
        let cmds = [r#"cmd /c exit 3"#, r#"cmd /c echo never"#];
        #[cfg(unix)]
        let cmds = [r#"sh -c 'exit 3'"#, r#"sh -c 'echo never'"#];
        let (outcome, events) = env.run(&cmds, None);
        assert!(
            matches!(&outcome, JobOutcome::Failed(m) if m.contains("終了コード 3")),
            "{outcome:?}"
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e, RunnerEvent::CommandStarted { index: 1, .. })));
    }

    #[test]
    fn missing_tool_fails_before_start() {
        let env = Env::new("missing");
        let (outcome, events) = env.run(&["no_such_tool_xyz -i a"], None);
        assert!(matches!(&outcome, JobOutcome::Failed(m) if m.contains("ツールが見つかりません")));
        assert!(!events
            .iter()
            .any(|e| matches!(e, RunnerEvent::CommandStarted { .. })));
    }

    #[test]
    fn abort_stops_long_command_and_cleans_up() {
        let env = Env::new("abort");
        #[cfg(windows)]
        let cmds = [
            r#"cmd /c "echo x> "<%TemporaryFile%>.tmp"& ping -n 30 127.0.0.1 >nul""#,
            r#"cmd /c echo never"#,
        ];
        #[cfg(unix)]
        let cmds = [
            r#"sh -c 'echo x > "$0.tmp"; sleep 30' "<%TemporaryFile%>""#,
            r#"sh -c 'echo never'"#,
        ];
        let started = Instant::now();
        let (outcome, _events) = env.run(&cmds, Some(Duration::from_millis(800)));
        assert_eq!(outcome, JobOutcome::Aborted);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "中断に時間がかかりすぎ"
        );
        assert_eq!(env.tmp_files(), 0);
    }

    /// 中断すると、このジョブで新しくできた出力は消し、前からあって書き換えられたものは残す。
    #[test]
    fn abort_removes_new_partial_outputs() {
        for delete in [true, false] {
            let mut env = Env::new(if delete {
                "partial_del"
            } else {
                "partial_keep"
            });
            env.delete_partial = delete;
            let out = env.dir.join("out");
            std::fs::write(out.join("input.log"), b"old\n").unwrap();
            #[cfg(windows)]
            let cmds = [
                r#"cmd /c "echo x> "<%OutputFile%>.txt"& echo y>> "<%OutputFile%>.log"& ping -n 30 127.0.0.1 >nul""#,
            ];
            #[cfg(unix)]
            let cmds =
                [r#"sh -c 'echo x > "$0.txt"; echo y >> "$0.log"; sleep 30' "<%OutputFile%>""#];
            let (outcome, events) = env.run(&cmds, Some(Duration::from_millis(1500)));
            assert_eq!(outcome, JobOutcome::Aborted);
            assert_eq!(out.join("input.txt").exists(), !delete);
            assert!(out.join("input.log").exists());
            let warned = events
                .iter()
                .any(|e| matches!(e, RunnerEvent::Warning(w) if w.contains("input.log")));
            assert_eq!(warned, delete);
            let noticed = events
                .iter()
                .any(|e| matches!(e, RunnerEvent::Notice(n) if n.contains("input.txt")));
            assert_eq!(noticed, delete);
        }
    }

    #[test]
    fn unknown_variable_is_warned_and_kept() {
        let env = Env::new("unknown");
        #[cfg(windows)]
        let cmds = [r#"cmd /c echo "<%NoSuchVar%>""#];
        #[cfg(unix)]
        let cmds = [r#"sh -c 'echo "$0"' "<%NoSuchVar%>""#];
        let (outcome, events) = env.run(&cmds, None);
        assert!(matches!(outcome, JobOutcome::Succeeded { .. }));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunnerEvent::Warning(w) if w.contains("<%NoSuchVar%>"))));
        assert!(events.iter().any(
            |e| matches!(e, RunnerEvent::Output { line, .. } if line.contains("<%NoSuchVar%>"))
        ));
    }

    #[test]
    fn carriage_return_lines_are_marked() {
        let env = Env::new("cr");
        // printf で \r 区切りの出力を作る
        #[cfg(windows)]
        let cmds = [
            r#"powershell -NoProfile -Command "[Console]::Out.Write('a 10%' + [char]13 + 'a 20%' + [char]13 + [char]10)""#,
        ];
        #[cfg(unix)]
        let cmds = [r#"sh -c 'printf "a 10%%\ra 20%%\r\n"'"#];
        let (outcome, events) = env.run(&cmds, None);
        assert!(
            matches!(outcome, JobOutcome::Succeeded { .. }),
            "{outcome:?}"
        );
        let lines: Vec<(String, bool)> = events
            .iter()
            .filter_map(|e| match e {
                RunnerEvent::Output {
                    line, replace_last, ..
                } => Some((line.clone(), *replace_last)),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec![("a 10%".into(), true), ("a 20%".into(), false)]);
    }

    #[test]
    fn skeleton_script_is_written_used_and_removed() {
        let mut env = Env::new("skeleton");
        std::fs::write(
            env.dir.join("skel.avs"),
            "Source(\"<%InputFile%>\")\n# <%InputFileBase%> <%Nope%>\n",
        )
        .unwrap();
        env.templates = vec![crate::settings::ScriptTemplate {
            ext: "TS".into(),
            template: "skel.avs".into(),
        }];
        // 生成されたスクリプトを出力先へ写す
        #[cfg(windows)]
        let cmds = [r#"cmd /c "copy /y "<%InputFile%>" "<%OutputFile%>.avs" >nul""#];
        #[cfg(unix)]
        let cmds = [r#"cp "<%InputFile%>" "<%OutputFile%>.avs""#];
        let (outcome, events) = env.run(&cmds, None);
        assert!(
            matches!(outcome, JobOutcome::Succeeded { .. }),
            "{outcome:?}"
        );
        let copied = std::fs::read(env.dir.join("out").join("input.avs")).unwrap();
        let text = crate::text::decode(&copied).0;
        let input = env.dir.join("input.ts");
        assert!(
            text.contains(&format!("Source(\"{}\")", input.display())),
            "{text}"
        );
        assert!(text.contains("# input <%Nope%>"));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunnerEvent::Warning(w) if w.contains("<%Nope%>"))));
        assert!(events.iter().any(
            |e| matches!(e, RunnerEvent::CommandStarted { display, .. } if display.contains(".avs"))
        ));
        assert_eq!(env.tmp_files(), 0, "生成したスクリプトが残っている");
    }

    #[test]
    fn skeleton_missing_template_fails() {
        let mut env = Env::new("skeleton_missing");
        env.templates = vec![crate::settings::ScriptTemplate {
            ext: "ts".into(),
            template: "nothing.vpy".into(),
        }];
        let (outcome, _) = env.run(&["no_such_tool_xyz"], None);
        assert!(
            matches!(&outcome, JobOutcome::Failed(m) if m.contains("テンプレートを読み込めません")),
            "{outcome:?}"
        );
    }
}
