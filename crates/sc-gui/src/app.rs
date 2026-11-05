//! メイン画面（画面設計 S-01）と、サブ画面・トレイ・通知のとりまとめ。

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, IconData, RichText, Theme, ViewportCommand, ViewportId};
use egui_extras::{Column, TableBuilder};
use sc_core::paths;
use sc_core::plan::{self, PlanOptions};
use sc_core::preset::PresetSet;
use sc_core::runner::{JobOutcome, RunOptions, RunnerEvent};
use sc_core::script::{ScriptKind, ScriptOptions, ScriptSupport};
use sc_core::settings::{self, Settings};

use crate::log_window::{Log, LogEntry, LogKind, LogView};
use crate::options::{OptionsAction, OptionsEnv, OptionsView, ToolStatus};
use crate::preset_editor::{CheckContext, EditorAction, PresetEditor};
use crate::queue::{JobId, JobState, Move, Queue};
use crate::tray::{Tray, TrayAction};
use crate::worker::{Worker, WorkerMsg};
use crate::{notify, os};

/// Transcoding.ini の変更を確かめる間隔。
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
/// トーストの表示時間。
const TOAST_TIME: Duration = Duration::from_secs(5);

/// アクセントカラー（画面設計 §3）。
const ACCENT_LIGHT: Color32 = Color32::from_rgb(0x02, 0x84, 0xc7);
const ACCENT_DARK: Color32 = Color32::from_rgb(0x38, 0xbd, 0xf8);

/// 起動時の指定（コマンドライン引数）。
#[derive(Debug, Default)]
pub struct StartupArgs {
    pub preset_index: Option<usize>,
    pub preset_title: Option<String>,
    pub output_dir: Option<PathBuf>,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
enum Dialog {
    ConfirmClose,
    ConfirmAbort,
    CreateOutputDir(PathBuf),
    ImportOriginal,
    /// Transcoding.ini の読み込みエラー（D-02）
    PresetError(String),
    Error(String),
    About,
}

pub struct App {
    settings: Settings,
    settings_path: PathBuf,
    app_dir: PathBuf,
    font_info: Option<String>,
    icon_png: &'static [u8],
    icon: Option<Arc<IconData>>,

    preset_path: PathBuf,
    presets: Option<PresetSet>,
    /// プリセットファイルを読めなかった理由（ステータスバーに出す）
    preset_error: Option<String>,
    preset_mtime: Option<SystemTime>,
    last_watch: Instant,

    /// ① で選んでいるプリセット（タイトル）
    current_preset: String,
    /// ② の出力先（入力中の文字列）
    output_dir_text: String,
    /// プリセット・出力先を画面で変えたか（起動引数で指定しただけなら設定に保存しない）
    preset_edited: bool,
    output_edited: bool,

    queue: Queue,
    worker: Option<Worker>,
    /// 「処理中を中断」（AllAbort）で自動の続行を止めている
    stopped: bool,
    /// 変換を 1 件でも始めたか（ExitIfFinished・完了通知用）
    ran_any: bool,

    log: Log,
    log_view: LogView,
    log_file: Option<(String, File)>,
    options: Option<OptionsView>,
    editor: Option<PresetEditor>,
    tool_status: Vec<ToolStatus>,
    /// AviSynth+ / VapourSynth の検出結果
    script_support: ScriptSupport,
    /// プレビュー中の見た目（テーマ・倍率）
    appearance: (settings::Theme, u16),

    tray: Option<Tray>,
    /// トレイに格納してウィンドウを隠している
    hidden: bool,
    last_tooltip: Instant,

    toast: Option<(String, Instant)>,
    dialog: Option<Dialog>,
    allow_close: bool,
    /// 閉じる要求を処理して設定を保存したか（on_exit で二重に保存しないため）
    saved_on_close: bool,
    /// on_exit で設定を保存するための Context（logic で受け取る）
    exit_ctx: egui::Context,
    /// Shift+クリックの起点
    anchor: Option<JobId>,
    settings_dirty: bool,
    /// SIGTERM などで終了を求められた（Linux。シグナルのスレッドが立てる）
    terminate: Arc<AtomicBool>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        args: StartupArgs,
        icon_png: &'static [u8],
    ) -> Self {
        let ctx = &cc.egui_ctx;
        let font_info = crate::fonts::install(ctx);
        let app_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let settings_path = settings::default_path(&app_dir);
        let first_run = !settings_path.exists();
        let (settings, settings_error) = match Settings::load(&settings_path) {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e.to_string())),
        };
        apply_appearance(ctx, settings.theme, settings.ui_scale);
        crate::diag::op(format!(
            "設定 {}（テーマ {:?}、拡大率 {}%）{}",
            settings_path.display(),
            settings.theme,
            settings.ui_scale,
            if settings_error.is_some() {
                "：読み込みエラー"
            } else {
                ""
            }
        ));

        let dialog = match settings_error {
            Some(e) => Some(Dialog::Error(format!(
                "設定ファイルを読み込めません。\n{e}"
            ))),
            None if first_run => Some(Dialog::ImportOriginal),
            None => None,
        };
        let tray = if crate::tray::SUPPORTED {
            Tray::new(ctx, icon_png)
        } else {
            None
        };
        if settings.start_minimized {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }

        let preset_path =
            settings::resolve_preset_path(&settings_path, &app_dir, &settings.preset_file);
        let mut app = Self {
            current_preset: settings.default_preset.clone(),
            output_dir_text: settings.output_dir.clone(),
            preset_edited: false,
            output_edited: false,
            appearance: (settings.theme, settings.ui_scale),
            settings,
            settings_path,
            app_dir,
            font_info,
            icon_png,
            icon: eframe::icon_data::from_png_bytes(icon_png)
                .ok()
                .map(Arc::new),
            preset_path,
            presets: None,
            preset_error: None,
            preset_mtime: None,
            last_watch: Instant::now(),
            queue: Queue::default(),
            worker: None,
            stopped: false,
            ran_any: false,
            log: Log::default(),
            log_view: LogView::default(),
            log_file: None,
            options: None,
            editor: None,
            tool_status: Vec::new(),
            script_support: ScriptSupport::default(),
            tray,
            hidden: false,
            last_tooltip: Instant::now(),
            toast: None,
            dialog,
            allow_close: false,
            saved_on_close: false,
            exit_ctx: cc.egui_ctx.clone(),
            anchor: None,
            settings_dirty: false,
            terminate: Arc::new(AtomicBool::new(false)),
        };
        app.reload_presets(false);
        // 前に強制終了されたときに残った一時フォルダを片付ける
        let stale = app.settings.remove_stale_temporary_dirs();
        if stale > 0 {
            app.show_toast(format!("前回残った一時フォルダを {stale} 件削除しました"));
        }
        // Linux：SIGTERM・SIGHUP・SIGINT（ログアウト、kill など）で終わるときも、変換中のツールを止めてから終わる。
        // 子プロセスは別のプロセスグループで動いているので、何もしないと残って動き続ける。
        #[cfg(target_os = "linux")]
        {
            let flag = app.terminate.clone();
            let ctx = ctx.clone();
            let _ = ctrlc::set_handler(move || {
                flag.store(true, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        app.detect_scripts(None);
        app.apply_startup_args(args, ctx);
        // 動作確認用（デバッグビルドのみ）：SC_DEBUG_OPEN=log,options,about でサブ画面を開いて起動する。
        #[cfg(debug_assertions)]
        if let Ok(v) = std::env::var("SC_DEBUG_OPEN") {
            if v.contains("log") {
                app.log_view.open_for(None);
            }
            if v.contains("options") {
                app.open_options();
            }
            if v.contains("about") {
                app.dialog = Some(Dialog::About);
            }
            if v.contains("presets") {
                app.open_editor();
            }
        }
        app
    }

    // -----------------------------------------------------------------------
    // プリセット

    fn reload_presets(&mut self, notify: bool) {
        self.preset_mtime = file_mtime(&self.preset_path);
        match PresetSet::load(&self.preset_path) {
            Ok(set) => {
                if set.by_title(&self.current_preset).is_none() {
                    self.current_preset = set
                        .presets
                        .first()
                        .map(|p| p.title.clone())
                        .unwrap_or_default();
                }
                let missing = self
                    .queue
                    .mark_missing_presets(|t| set.by_title(t).is_some());
                if notify {
                    let mut msg =
                        format!("プリセットを再読み込みしました（{} 件）", set.presets.len());
                    if missing > 0 {
                        msg.push_str(&format!(
                            "。{missing} 件のジョブのプリセットが見つかりません"
                        ));
                    }
                    self.show_toast(msg);
                }
                self.presets = Some(set);
                self.preset_error = None;
            }
            Err(e) => {
                self.presets = None;
                self.preset_error = Some(e.to_string());
                if self.dialog.is_none() {
                    self.dialog = Some(Dialog::PresetError(format!(
                        "Transcoding.ini を読み込めませんでした。\n{e}"
                    )));
                }
            }
        }
    }

    fn watch_presets(&mut self) {
        if self.last_watch.elapsed() < WATCH_INTERVAL {
            return;
        }
        self.last_watch = Instant::now();
        let mtime = file_mtime(&self.preset_path);
        if mtime.is_some() && mtime != self.preset_mtime {
            self.reload_presets(true);
        }
    }

    fn preset_titles(&self) -> Vec<String> {
        self.presets
            .as_ref()
            .map(|s| s.presets.iter().map(|p| p.title.clone()).collect())
            .unwrap_or_default()
    }

    /// プリセットで使われているツールを調べる（オプション画面の［ツール］）。
    fn refresh_tool_status(&mut self) {
        self.detect_scripts(None);
        let mut names: Vec<String> = Vec::new();
        if let Some(set) = &self.presets {
            for p in &set.presets {
                for c in &p.commands {
                    if let Ok(parsed) = sc_core::cmdline::parse(c) {
                        if !names
                            .iter()
                            .any(|n| n.eq_ignore_ascii_case(&parsed.program))
                        {
                            names.push(parsed.program);
                        }
                    }
                }
            }
        }
        let path_env = std::env::var_os("PATH");
        self.tool_status = names
            .into_iter()
            .map(|name| ToolStatus {
                path: sc_core::cmdline::resolve_program(
                    &name,
                    &self.settings.tools,
                    &self.app_dir,
                    path_env.as_deref(),
                ),
                name,
            })
            .collect();
    }

    // -----------------------------------------------------------------------
    // ジョブの追加と実行

    fn apply_startup_args(&mut self, args: StartupArgs, ctx: &egui::Context) {
        if let Some(set) = &self.presets {
            let chosen = args
                .preset_index
                .and_then(|i| set.by_index(i))
                .or_else(|| args.preset_title.as_deref().and_then(|t| set.by_title(t)));
            if let Some(p) = chosen {
                self.current_preset = p.title.clone();
            } else if args.preset_index.is_some() || args.preset_title.is_some() {
                self.show_toast("指定されたプリセットが見つかりません".into());
            }
        }
        if let Some(dir) = args.output_dir {
            self.output_dir_text = dir.display().to_string();
        }
        if !args.files.is_empty() {
            // コマンドラインで渡されたときは、設定にかかわらずすぐ始める（本家互換）。
            if self.dialog == Some(Dialog::ImportOriginal) {
                self.dialog = None;
            }
            self.add_files(args.files, ctx, true);
        }
    }

    fn add_files(&mut self, files: Vec<PathBuf>, ctx: &egui::Context, force_start: bool) {
        let mut folders = 0;
        let mut added = 0;
        for f in files {
            if f.is_dir() {
                folders += 1;
                continue;
            }
            self.queue.add(paths::absolute(&f), &self.current_preset);
            added += 1;
        }
        crate::diag::op(format!("ファイル追加 {added} 件（フォルダ {folders} 件）"));
        if folders > 0 {
            self.show_toast(format!("フォルダは登録できません（{folders} 件）"));
        }
        if added > 0 && (force_start || self.settings.auto_start_on_add) {
            self.stopped = false;
            self.try_start_next(ctx);
        }
    }

    /// 出力先の入力欄の状態。
    fn output_dir_problem(&self) -> Option<&'static str> {
        let t = self.output_dir_text.trim();
        if t.is_empty() {
            Some("出力先を指定してください")
        } else if paths::is_root(Path::new(t)) {
            Some("ドライブのルートには出力できません")
        } else {
            None
        }
    }

    fn try_start_next(&mut self, ctx: &egui::Context) {
        if self.worker.is_some() || self.stopped {
            return;
        }
        let Some(presets) = &self.presets else {
            return;
        };
        if let Some(problem) = self.output_dir_problem() {
            if self.queue.next_pending().is_some() {
                self.show_toast(problem.into());
            }
            return;
        }
        let output_dir = paths::absolute(Path::new(self.output_dir_text.trim()));
        if !output_dir.is_dir() {
            if self.queue.next_pending().is_some() {
                self.show_window(ctx);
                self.dialog = Some(Dialog::CreateOutputDir(output_dir));
            }
            return;
        }

        while let Some(id) = self.queue.next_pending() {
            let Some(job) = self.queue.get(id) else {
                return;
            };
            let Some(preset) = presets.by_title(&job.preset_title) else {
                let job = self.queue.get_mut(id).expect("存在するジョブ");
                job.state = JobState::PresetMissing;
                job.message = Some(format!("プリセット「{}」がありません", job.preset_title));
                continue;
            };

            let temp_dir = self.settings.temporary_dir();
            let _ = std::fs::create_dir_all(&temp_dir);
            let path_env = std::env::var_os("PATH");
            let popts = PlanOptions {
                output_dir: &output_dir,
                temporary_dir: &temp_dir,
                on_exists: self.settings.on_exists,
                timestamp: &self.settings.timestamp,
                tools: &self.settings.tools,
                app_dir: &self.app_dir,
                path_env: path_env.clone(),
                script: (!self.settings.script_disabled
                    && !self.settings.script_templates.is_empty())
                .then_some(ScriptOptions {
                    templates: &self.settings.script_templates,
                    support: &self.script_support,
                }),
            };
            let mtime = plan::file_mtime(&job.input);
            let job_plan = plan::plan_job(&job.input, &mtime, preset, id, &popts);
            let ropts = RunOptions {
                priority: self.settings.priority,
                tools: self.settings.tools.clone(),
                app_dir: self.app_dir.clone(),
                path_env,
                delete_partial: self.settings.delete_partial,
            };

            let input = job.input.clone();
            let preset_title = preset.title.clone();
            let job = self.queue.get_mut(id).expect("存在するジョブ");
            job.state = JobState::Running;
            job.started_at = Some(Instant::now());
            job.output_base = job_plan.output.as_ref().map(|o| o.base.clone());
            self.push_log(
                id,
                LogKind::JobStart,
                format!("#{id} {} [{preset_title}]", input.display()),
            );
            self.ran_any = true;
            crate::diag::op(format!("ジョブ #{id} 開始 [{preset_title}]"));
            self.worker = Some(Worker::start(id, job_plan, ropts, ctx.clone()));
            return;
        }
        self.on_queue_idle(ctx);
    }

    fn abort_running(&mut self) {
        crate::diag::op("中断");
        if let Some(w) = &self.worker {
            w.abort();
            if self.settings.all_abort {
                self.stopped = true;
            }
        }
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.worker else { return };
        let msgs = worker.drain();
        let mut finished = false;
        for msg in msgs {
            match msg {
                WorkerMsg::Event(id, event) => self.on_event(id, event),
                WorkerMsg::Finished(id, outcome) => {
                    self.on_finished(id, outcome, ctx);
                    finished = true;
                }
            }
        }
        if finished {
            self.worker = None;
            self.try_start_next(ctx);
        }
    }

    fn on_event(&mut self, id: JobId, event: RunnerEvent) {
        match event {
            RunnerEvent::CommandStarted {
                index,
                count,
                tool,
                display,
            } => {
                if let Some(j) = self.queue.get_mut(id) {
                    j.progress.command = index + 1;
                    j.progress.command_count = count;
                    j.progress.tool = tool;
                    j.progress.update = Default::default();
                }
                self.push_log(
                    id,
                    LogKind::Command,
                    format!("[{}/{count}] {display}", index + 1),
                );
            }
            RunnerEvent::Progress { update, .. } => {
                if let Some(j) = self.queue.get_mut(id) {
                    j.progress.update = update;
                }
            }
            RunnerEvent::Output {
                line, replace_last, ..
            } => {
                let entry = LogEntry {
                    job: id,
                    time: SystemTime::now(),
                    kind: LogKind::Output,
                    text: line,
                };
                if replace_last {
                    // \r で上書きされる進捗行はファイルには書かない
                    self.log.replace_last_output(entry);
                } else {
                    self.write_log_file(&entry);
                    self.log.push(entry);
                }
            }
            RunnerEvent::Info(s) | RunnerEvent::Notice(s) => self.push_log(id, LogKind::Info, s),
            RunnerEvent::Warning(s) => self.push_log(id, LogKind::Warning, s),
            RunnerEvent::CommandFinished {
                index, exit_code, ..
            } => {
                let kind = if exit_code == Some(0) {
                    LogKind::Success
                } else {
                    LogKind::Failure
                };
                let code = exit_code.map_or("なし".to_owned(), |c| c.to_string());
                self.push_log(id, kind, format!("[{}] 終了コード {code}", index + 1));
            }
        }
    }

    fn on_finished(&mut self, id: JobId, outcome: JobOutcome, ctx: &egui::Context) {
        let Some(j) = self.queue.get_mut(id) else {
            return;
        };
        j.elapsed = j.started_at.map(|t| t.elapsed());
        let name = j.file_name();
        let (state, msg, kind) = match outcome {
            JobOutcome::Succeeded { outputs } => {
                j.outputs = outputs;
                (JobState::Succeeded, None, LogKind::Success)
            }
            JobOutcome::Failed(m) => (JobState::Failed, Some(m), LogKind::Failure),
            JobOutcome::Aborted => (
                JobState::Aborted,
                Some("中断しました".to_owned()),
                LogKind::Failure,
            ),
            JobOutcome::Skipped => (
                JobState::Skipped,
                Some("同名のファイルがあるためスキップしました".to_owned()),
                LogKind::Info,
            ),
        };
        j.state = state;
        j.message = msg.clone();
        crate::diag::op(format!("ジョブ #{id} 終了：{}", state.label()));
        let text = format!(
            "#{id} {}{}",
            state.label(),
            msg.as_deref().map(|m| format!("：{m}")).unwrap_or_default()
        );
        self.push_log(id, kind, text);

        // 最小化中なら OS の通知を出す（N-03 / N-04）
        if self.settings.notify && self.is_background(ctx) {
            match state {
                JobState::Succeeded => notify::show("変換完了", &name),
                JobState::Failed => {
                    notify::show("変換失敗", &format!("{name}\n{}", msg.unwrap_or_default()))
                }
                _ => {}
            }
        }
    }

    /// 待機中のジョブがなくなったとき。
    fn on_queue_idle(&mut self, ctx: &egui::Context) {
        if !self.ran_any || self.worker.is_some() || self.queue.next_pending().is_some() {
            return;
        }
        if self.settings.notify && self.is_background(ctx) {
            notify::show("すべての変換が終わりました", &self.queue.finished_text());
        }
        if self.settings.exit_if_finished {
            self.allow_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn push_log(&mut self, id: JobId, kind: LogKind, text: String) {
        let entry = LogEntry {
            job: id,
            time: SystemTime::now(),
            kind,
            text,
        };
        self.write_log_file(&entry);
        self.log.push(entry);
    }

    /// 設定フォルダの logs/日付.log に追記する（LogToFile）。
    fn write_log_file(&mut self, entry: &LogEntry) {
        if !self.settings.log_to_file {
            return;
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if self.log_file.as_ref().is_none_or(|(d, _)| *d != today) {
            let dir = self
                .settings_path
                .parent()
                .unwrap_or(Path::new("."))
                .join("logs");
            let _ = std::fs::create_dir_all(&dir);
            self.log_file = File::options()
                .create(true)
                .append(true)
                .open(dir.join(format!("{today}.log")))
                .ok()
                .map(|f| (today, f));
        }
        if let Some((_, f)) = &mut self.log_file {
            let _ = writeln!(f, "#{} {}", entry.job, entry.format());
        }
    }

    fn show_toast(&mut self, msg: String) {
        self.toast = Some((msg, Instant::now()));
    }

    // -----------------------------------------------------------------------
    // ウィンドウ・トレイ

    /// 最小化中またはトレイに格納中か。
    fn is_background(&self, ctx: &egui::Context) -> bool {
        self.hidden || ctx.input(|i| i.viewport().minimized == Some(true))
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        if self.hidden {
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            self.hidden = false;
        }
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        let actions = tray.poll();
        for a in actions {
            match a {
                TrayAction::ShowWindow => self.show_window(ctx),
                TrayAction::ShowLog => self.log_view.open_for(None),
                TrayAction::Abort => {
                    self.show_window(ctx);
                    if self.worker.is_some() {
                        self.dialog = Some(Dialog::ConfirmAbort);
                    }
                }
                TrayAction::Options => self.open_options(),
                TrayAction::Quit => {
                    self.show_window(ctx);
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        // 最小化されたら、トレイに格納してウィンドウを隠す。
        // Wayland ではアプリがウィンドウを隠せない（winit が対応していない）ので、タスクバーに残したままにする。
        let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
        if minimized && !self.hidden && self.settings.minimize_to_tray && !crate::is_wayland() {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
            self.hidden = true;
        }

        if self.last_tooltip.elapsed() >= Duration::from_secs(1) {
            self.last_tooltip = Instant::now();
            let tip = match self.queue.running() {
                Some(j) => {
                    let percent = j
                        .progress
                        .update
                        .ratio
                        .map(|r| format!(" {:.0}%", r * 100.0))
                        .unwrap_or_default();
                    format!(
                        "SmartConverter - {}{percent}（残り {} 件）",
                        j.file_name(),
                        self.queue.count(JobState::Pending)
                    )
                }
                None => "SmartConverter - 待機中".to_owned(),
            };
            if let Some(tray) = &self.tray {
                tray.set_tooltip(&tip);
                tray.set_abort_enabled(self.worker.is_some());
            }
        }
    }

    /// プリセット編集画面を開く。確認欄のサンプルには、選んでいるジョブ（無ければ先頭）の入力を使う。
    fn open_editor(&mut self) {
        crate::diag::op("プリセット編集を開く");
        if self.editor.is_some() {
            return;
        }
        let sample = self
            .queue
            .selected
            .iter()
            .next()
            .and_then(|id| self.queue.get(*id))
            .or_else(|| self.queue.jobs.first())
            .map(|j| j.input.clone());
        match PresetEditor::open(&self.preset_path, sample.as_deref()) {
            Ok(e) => self.editor = Some(e),
            Err(e) => self.dialog = Some(Dialog::Error(e)),
        }
    }

    /// プリセット編集画面で保存されたとき。
    fn on_presets_saved(&mut self, renames: &[(String, String)]) {
        for (old, new) in renames {
            self.queue.rename_preset(old, new);
            if self.current_preset == *old {
                self.current_preset = new.clone();
                self.preset_edited = true;
                self.settings_dirty = true;
            }
        }
        self.reload_presets(false);
        self.show_toast("プリセットを保存しました（元のファイルは .bak に残しています）".into());
    }

    fn open_options(&mut self) {
        if self.options.is_none() {
            crate::diag::op(format!(
                "オプション画面を開く（変換中 {}）",
                self.worker.is_some()
            ));
            self.refresh_tool_status();
            self.options = Some(OptionsView::new(&self.settings));
        }
    }

    // -----------------------------------------------------------------------
    // 設定

    fn save_settings(&mut self, ctx: &egui::Context) {
        if self.preset_edited {
            self.settings.default_preset = self.current_preset.clone();
        }
        if self.output_edited {
            self.settings.output_dir = self.output_dir_text.trim().to_owned();
        }
        // ウィンドウの大きさ（と、扱える OS では位置）を保存する。隠しているとき・最小化中は保存しない
        // （最小化中の位置は画面の外になる。Windows では -32000）。
        if !self.hidden && ctx.input(|i| i.viewport().minimized != Some(true)) {
            if let Some(rect) = ctx.input(|i| i.viewport().outer_rect) {
                let size = ctx
                    .input(|i| i.viewport().inner_rect)
                    .map_or(rect.size(), |r| r.size());
                if size.x >= 640.0 && size.y >= 420.0 {
                    self.settings.main_window = format!(
                        "{},{},{},{}",
                        rect.min.x.round(),
                        rect.min.y.round(),
                        size.x.round(),
                        size.y.round()
                    );
                }
            }
        }
        crate::diag::op("設定を保存");
        if let Err(e) = self.settings.save(&self.settings_path) {
            crate::diag::op(format!("設定を保存できません：{e}"));
            self.show_toast(format!("設定を保存できません: {e}"));
        }
        self.settings_dirty = false;
    }

    /// オプション画面で［OK］［適用］されたとき。
    fn apply_options(&mut self, mut new: Settings, ctx: &egui::Context) {
        // メイン画面で持っている値は、オプション画面を開いてから変わっているかもしれないので引き継ぐ。
        new.default_preset = self.settings.default_preset.clone();
        new.output_dir = self.settings.output_dir.clone();
        new.main_window = self.settings.main_window.clone();
        new.log_window = self.settings.log_window.clone();
        let preset_changed = new.preset_file != self.settings.preset_file;
        crate::diag::op(format!(
            "オプションを適用（テーマ {:?}、拡大率 {}%）",
            new.theme, new.ui_scale
        ));
        self.settings = new;
        self.appearance = (self.settings.theme, self.settings.ui_scale);
        apply_appearance(ctx, self.settings.theme, self.settings.ui_scale);
        if preset_changed {
            self.preset_path = settings::resolve_preset_path(
                &self.settings_path,
                &self.app_dir,
                &self.settings.preset_file,
            );
            self.reload_presets(true);
        }
        self.refresh_tool_status();
        self.save_settings(ctx);
    }

    fn import_original(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("携帯動画変換君の設定（3GP_Converter.ini）を選択")
            .add_filter("3GP_Converter.ini", &["ini"])
            .pick_file()
        else {
            return;
        };
        let before_preset_file = self.settings.preset_file.clone();
        match self.settings.import_original(&path) {
            Ok(()) => {
                self.current_preset = self.settings.default_preset.clone();
                self.output_dir_text = self.settings.output_dir.clone();
                apply_appearance(ctx, self.settings.theme, self.settings.ui_scale);
                if self.settings.preset_file != before_preset_file {
                    self.preset_path = settings::resolve_preset_path(
                        &self.settings_path,
                        &self.app_dir,
                        &self.settings.preset_file,
                    );
                    self.reload_presets(false);
                }
                // 取り込んだ設定を、開いているオプション画面にも反映する
                if self.options.is_some() {
                    self.options = Some(OptionsView::new(&self.settings));
                }
                self.save_settings(ctx);
                self.show_toast(format!("{} を取り込みました", path.display()));
            }
            Err(e) => {
                self.dialog = Some(Dialog::Error(format!("設定を取り込めません。\n{e}")));
            }
        }
    }

    // -----------------------------------------------------------------------
    // 画面

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let sc = |m, k| KeyboardShortcut::new(m, k);
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::O))) {
            self.pick_files(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Q))) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::NONE, Key::F5))) {
            self.reload_presets(true);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Enter))) {
            self.stopped = false;
            self.try_start_next(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Period)))
            && self.worker.is_some()
        {
            self.dialog = Some(Dialog::ConfirmAbort);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::L))) {
            self.log_view.open_for(None);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Comma))) {
            self.open_options();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::P))) {
            self.open_editor();
        }
        // テキスト入力中は一覧の操作をしない
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::A))) {
            self.queue.selected = self.queue.jobs.iter().map(|j| j.id).collect();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::NONE, Key::Delete))) {
            self.queue.remove_selected();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::ALT, Key::ArrowUp))) {
            self.queue.move_selected(Move::Up);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::ALT, Key::ArrowDown))) {
            self.queue.move_selected(Move::Down);
        }
    }

    fn pick_files(&mut self, ctx: &egui::Context) {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("変換するファイルを追加")
            .pick_files()
        {
            self.add_files(files, ctx, false);
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("ファイル(F)", |ui| {
                if ui.button("ファイルを追加…    Ctrl+O").clicked() {
                    self.pick_files(&ctx);
                }
                if ui.button("本家の設定を取り込む…").clicked() {
                    self.import_original(&ctx);
                }
                ui.separator();
                if ui.button("終了    Ctrl+Q").clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            });
            ui.menu_button("表示(V)", |ui| {
                if ui.button("ログ    Ctrl+L").clicked() {
                    self.log_view.open_for(None);
                }
                ui.menu_button("テーマ", |ui| {
                    use sc_core::settings::Theme as T;
                    let mut changed = false;
                    for (t, label) in [
                        (T::System, "OS に合わせる"),
                        (T::Light, "ライト"),
                        (T::Dark, "ダーク"),
                    ] {
                        if ui.radio(self.settings.theme == t, label).clicked() {
                            self.settings.theme = t;
                            changed = true;
                        }
                    }
                    if changed {
                        crate::diag::op(format!(
                            "テーマを {:?} に（変換中 {}）",
                            self.settings.theme,
                            self.worker.is_some()
                        ));
                        self.appearance = (self.settings.theme, self.settings.ui_scale);
                        apply_appearance(&ctx, self.settings.theme, self.settings.ui_scale);
                        self.settings_dirty = true;
                    }
                });
            });
            ui.menu_button("ツール(T)", |ui| {
                if ui.button("プリセット編集    Ctrl+P").clicked() {
                    self.open_editor();
                }
                if ui.button("プリセットを再読み込み    F5").clicked() {
                    self.reload_presets(true);
                }
                if ui.button("オプション    Ctrl+,").clicked() {
                    self.open_options();
                }
            });
            ui.menu_button("ヘルプ(H)", |ui| {
                if ui.button("置換変数の一覧").clicked() {
                    self.dialog = Some(Dialog::About);
                }
                if ui.button("バージョン情報").clicked() {
                    self.dialog = Some(Dialog::About);
                }
            });
        });
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let titles = self.preset_titles();
        egui::Grid::new("controls")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                let name_label = ui.label("プリセット");
                if titles.is_empty() {
                    ui.add_enabled(false, egui::Label::new("プリセットがありません"));
                } else {
                    let before = self.current_preset.clone();
                    let combo = egui::ComboBox::from_id_salt("preset")
                        .width(ui.available_width())
                        .selected_text(&self.current_preset)
                        .show_ui(ui, |ui| {
                            for t in &titles {
                                ui.selectable_value(&mut self.current_preset, t.clone(), t);
                            }
                        });
                    combo.response.labelled_by(name_label.id);
                    if before != self.current_preset {
                        self.preset_edited = true;
                        self.settings_dirty = true;
                    }
                }
                ui.end_row();

                ui.label("出力先");
                ui.horizontal(|ui| {
                    let problem = self.output_dir_problem();
                    let missing =
                        problem.is_none() && !Path::new(self.output_dir_text.trim()).is_dir();
                    let width = ui.available_width() - 110.0;
                    let mut edit = egui::TextEdit::singleline(&mut self.output_dir_text)
                        .desired_width(width.max(100.0))
                        .hint_text("出力先フォルダ");
                    if problem.is_some() || missing {
                        edit = edit.text_color(error_color(ui.ctx()));
                    }
                    let resp = ui.add(edit);
                    if let Some(p) = problem {
                        resp.clone().on_hover_text(p);
                    } else if missing {
                        resp.clone().on_hover_text(
                            "フォルダがありません（変換開始時に作成するか確認します）",
                        );
                    }
                    if resp.changed() {
                        self.output_edited = true;
                        self.settings_dirty = true;
                    }
                    if ui.button("選択").clicked() {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("出力先フォルダを選択")
                            .pick_folder()
                        {
                            self.output_dir_text = dir.display().to_string();
                            self.output_edited = true;
                            self.settings_dirty = true;
                        }
                    }
                    if ui.button("開く").clicked() {
                        let dir = PathBuf::from(self.output_dir_text.trim());
                        if dir.is_dir() {
                            os::open_folder(&dir, None);
                        }
                    }
                });
                ui.end_row();
                if let Some(p) = self.output_dir_problem() {
                    ui.label("");
                    ui.colored_label(error_color(ui.ctx()), p);
                    ui.end_row();
                }
            });
    }

    fn queue_table(&mut self, ui: &mut egui::Ui) {
        if self.queue.jobs.is_empty() {
            // Wayland のウィンドウではドロップを受け取れない（winit 0.30 の制限。main.rs の説明）
            let hint = if crate::is_wayland() {
                "［ファイルを追加］（Ctrl+O）で動画ファイルを追加\n\
                 （Wayland ではドラッグ＆ドロップできません。\
                 使うときは SMARTCONVERTER_X11=1 を付けて起動してください）"
            } else {
                "ここに動画ファイルをドラッグ＆ドロップ"
            };
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(hint).size(18.0).weak());
            });
            return;
        }

        let titles = self.preset_titles();
        let mut clicked: Option<(JobId, egui::Modifiers)> = None;
        let mut action: Option<RowAction> = None;
        let jobs: Vec<_> = self.queue.jobs.clone();
        let selected = self.queue.selected.clone();
        let accent = accent(ui.ctx());

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(28.0))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::initial(170.0).at_least(80.0).clip(true))
            .column(Column::initial(80.0).at_least(60.0))
            .column(Column::initial(120.0).at_least(80.0))
            .column(Column::initial(64.0).at_least(50.0))
            .header(22.0, |mut h| {
                for t in ["#", "ファイル", "プリセット", "状態", "進捗", "経過"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|mut body| {
                for (n, job) in jobs.iter().enumerate() {
                    body.row(24.0, |mut row| {
                        row.set_selected(selected.contains(&job.id));
                        row.col(|ui| {
                            ui.label((n + 1).to_string());
                        });
                        row.col(|ui| {
                            // 省略したときに egui が自動で出す吹き出しは止め、フルパスの吹き出しだけにする
                            ui.add(
                                egui::Label::new(job.file_name()).show_tooltip_when_elided(false),
                            )
                            .on_hover_text(job.input.display().to_string());
                        });
                        row.col(|ui| {
                            ui.label(&job.preset_title);
                        });
                        row.col(|ui| {
                            let (text, color) = state_style(job.state, ui.ctx(), accent);
                            let resp = ui.colored_label(color, text);
                            if let Some(m) = &job.message {
                                resp.on_hover_text(m);
                            }
                        });
                        let is_selected = selected.contains(&job.id);
                        row.col(|ui| progress_cell(ui, job, is_selected));
                        row.col(|ui| {
                            if let Some(e) = job.elapsed_now() {
                                ui.label(fmt_duration(e));
                            }
                        });
                        let resp = row.response();
                        if resp.clicked() {
                            clicked = Some((job.id, resp.ctx.input(|i| i.modifiers)));
                        }
                        if resp.double_clicked() {
                            action = Some(RowAction::ShowLog(job.id));
                        }
                        if resp.secondary_clicked() && !selected.contains(&job.id) {
                            clicked = Some((job.id, egui::Modifiers::NONE));
                        }
                        resp.context_menu(|ui| {
                            if let Some(a) = row_menu(ui, job, &titles) {
                                action = Some(a);
                            }
                        });
                    });
                }
            });

        if let Some((id, mods)) = clicked {
            self.select(id, mods);
        }
        if let Some(a) = action {
            self.do_row_action(a);
        }
    }

    fn select(&mut self, id: JobId, mods: egui::Modifiers) {
        if mods.shift {
            if let Some(anchor) = self.anchor {
                let pos = |x| self.queue.jobs.iter().position(|j| j.id == x);
                if let (Some(a), Some(b)) = (pos(anchor), pos(id)) {
                    let (s, e) = if a <= b { (a, b) } else { (b, a) };
                    self.queue.selected = self.queue.jobs[s..=e].iter().map(|j| j.id).collect();
                    return;
                }
            }
        }
        if mods.command {
            if !self.queue.selected.remove(&id) {
                self.queue.selected.insert(id);
            }
        } else {
            self.queue.selected = [id].into();
        }
        self.anchor = Some(id);
    }

    fn do_row_action(&mut self, action: RowAction) {
        crate::diag::op(format!("行の操作：{}", action.name()));
        match action {
            RowAction::SetPreset(t) => self.queue.set_preset_for_selected(&t),
            RowAction::Retry => self.queue.retry_selected(),
            RowAction::Move(m) => self.queue.move_selected(m),
            RowAction::Remove => {
                self.queue.remove_selected();
            }
            RowAction::ShowLog(id) => self.log_view.open_for(Some(id)),
            RowAction::OpenOutput(id) => {
                if let Some(j) = self.queue.get(id) {
                    let file = j.outputs.first().cloned();
                    let dir = j
                        .output_base
                        .as_ref()
                        .and_then(|b| b.parent().map(Path::to_path_buf));
                    if let Some(dir) = dir {
                        os::open_folder(&dir, file.as_deref());
                    }
                }
            }
            RowAction::OpenInput(id) => {
                if let Some(j) = self.queue.get(id) {
                    if let Some(dir) = j.input.parent() {
                        os::open_folder(dir, Some(&j.input));
                    }
                }
            }
        }
    }

    fn detail(&self, ui: &mut egui::Ui) {
        ui.set_min_height(64.0);
        let Some(job) = self.queue.running() else {
            ui.add_space(4.0);
            ui.weak("待機中");
            return;
        };
        let p = &job.progress;
        ui.horizontal(|ui| {
            ui.strong(job.file_name());
            if p.command_count > 0 {
                ui.label(format!(
                    "コマンド {}/{}  {}",
                    p.command, p.command_count, p.tool
                ));
            }
        });
        let bar = match p.update.ratio {
            Some(r) => egui::ProgressBar::new(r).show_percentage(),
            None => egui::ProgressBar::new(0.0).animate(true),
        };
        add_progress(ui, bar.fill(progress_fill(ui.ctx(), false)));
        ui.horizontal_wrapped(|ui| {
            let elapsed = job.elapsed_now().unwrap_or_default();
            ui.label(format!("経過 {}", fmt_duration(elapsed)));
            if let Some(eta) = p.update.eta {
                ui.label(format!(
                    "予想終了 {}（あと {}）",
                    clock_after(eta),
                    fmt_duration(eta)
                ));
            }
            if let Some(size) = p.update.estimated_size {
                ui.label(format!(
                    "予想サイズ {:.0} MB",
                    size as f64 / 1024.0 / 1024.0
                ));
            }
            if let (Some(f), Some(fps)) = (p.update.frames, p.update.fps) {
                match p.update.total_frames {
                    Some(t) => ui.weak(format!("{f}/{t} frames  {fps:.0} fps")),
                    None => ui.weak(format!("{f} frames  {fps:.0} fps")),
                };
            }
        });
    }

    fn buttons(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            let can_start = self.worker.is_none() && self.queue.next_pending().is_some();
            if ui
                .add_enabled(can_start, egui::Button::new("▶ 変換開始"))
                .on_hover_text("Ctrl+Enter")
                .clicked()
            {
                crate::diag::op("［変換開始］");
                self.stopped = false;
                self.try_start_next(&ctx);
            }
            if ui
                .add_enabled(self.worker.is_some(), egui::Button::new("■ 処理中を中断"))
                .on_hover_text("Ctrl+.")
                .clicked()
            {
                self.dialog = Some(Dialog::ConfirmAbort);
            }
            if ui
                .add_enabled(
                    self.queue.count(JobState::Succeeded) > 0,
                    egui::Button::new("完了分を消去"),
                )
                .clicked()
            {
                crate::diag::op("［完了分を消去］");
                self.queue.clear_finished();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("設定").on_hover_text("Ctrl+,").clicked() {
                    self.open_options();
                }
                if ui.button("ログ").on_hover_text("Ctrl+L").clicked() {
                    self.log_view.open_for(None);
                }
            });
        });
    }

    /// AviSynth+ / VapourSynth を調べる（`tools` を省くと今の設定の［ツール］を使う）。
    fn detect_scripts(&mut self, tools: Option<&BTreeMap<String, String>>) {
        let path_env = std::env::var_os("PATH");
        self.script_support = ScriptSupport::detect(
            tools.unwrap_or(&self.settings.tools),
            &self.app_dir,
            path_env.as_deref(),
        );
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(self.queue.status_text());
            // 右から：VapourSynth、AviSynth+、プリセットファイル（長いパスは … で省略）
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for kind in [ScriptKind::VapourSynth, ScriptKind::AviSynth] {
                    match self.script_support.get(kind) {
                        Some(p) => ui
                            .label(format!("{} ✓", kind.name()))
                            .on_hover_text(p.display().to_string()),
                        None => ui
                            .weak(format!("{} ✗", kind.name()))
                            .on_hover_text("見つかりません（この種類のスケルトンは使いません）"),
                    };
                    ui.add_space(6.0);
                }
                if let Some(set) = &self.presets {
                    let text = format!(
                        "{}（{}）",
                        self.preset_path.display(),
                        set.encoding.display_name()
                    );
                    let font = match &self.font_info {
                        Some(f) => format!("日本語フォント: {f}"),
                        None => "日本語フォントが見つかりません".into(),
                    };
                    ui.add(egui::Label::new(egui::RichText::new(&text).weak()).truncate())
                        .on_hover_text(format!("{text}\n{font}"));
                } else {
                    // 読み込めないときは、探したファイルを出す（ようこそ画面でエラーが隠れても分かるように）
                    let text =
                        format!("プリセットを読み込めません：{}", self.preset_path.display());
                    let err = self.preset_error.clone().unwrap_or_default();
                    ui.add(
                        egui::Label::new(egui::RichText::new(&text).color(error_color(ui.ctx())))
                            .truncate(),
                    )
                    .on_hover_text(format!("{text}\n{err}"));
                }
            });
        });
    }

    fn drop_overlay(&mut self, ctx: &egui::Context) {
        let hovered = ctx.input(|i| i.raw.hovered_files.len());
        if hovered > 0 {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop_overlay"),
            ));
            let rect = ctx.content_rect().shrink(8.0);
            let color = accent(ctx);
            painter.rect_filled(rect, 8.0, color.gamma_multiply(0.12));
            painter.rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(2.0, color),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("ドロップして追加（{hovered} 件）"),
                egui::FontId::proportional(22.0),
                color,
            );
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_files(dropped, ctx, false);
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.toast else { return };
        if at.elapsed() > TOAST_TIME {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -90.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(msg.as_str());
                });
            });
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            match &dialog {
                Dialog::ConfirmClose => {
                    ui.heading("確認");
                    ui.label("変換中のファイルがあります。中断して終了しますか？");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("中断して終了").clicked() {
                            self.abort_running();
                            self.allow_close = true;
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                            close = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::ConfirmAbort => {
                    let name = self.queue.running().map(|j| j.file_name()).unwrap_or_default();
                    ui.heading("確認");
                    ui.label(format!("「{name}」の変換を中断しますか？"));
                    if self.settings.all_abort {
                        ui.label("（残りのファイルも開始しません）");
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("中断").clicked() {
                            self.abort_running();
                            close = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::CreateOutputDir(dir) => {
                    ui.heading("確認");
                    ui.label(format!("出力先フォルダ {} がありません。作成しますか？", dir.display()));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("作成").clicked() {
                            close = true;
                            match std::fs::create_dir_all(dir) {
                                Ok(()) => {
                                    self.dialog = None;
                                    self.try_start_next(ctx);
                                }
                                Err(e) => {
                                    self.dialog = Some(Dialog::Error(format!(
                                        "フォルダを作成できません。\n{e}"
                                    )));
                                    close = false;
                                }
                            }
                        }
                        if ui.button("キャンセル").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::ImportOriginal => {
                    ui.heading("ようこそ");
                    ui.label("携帯動画変換君の設定（3GP_Converter.ini）を取り込みますか？");
                    ui.weak("出力先・既定のプリセット・優先度・タイムスタンプなどを引き継ぎます。同じフォルダの Transcoding.ini も使います。");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("ファイルを選択…").clicked() {
                            close = true;
                            self.dialog = None;
                            self.import_original(ctx);
                        }
                        if ui.button("取り込まない").clicked() {
                            close = true;
                            self.save_settings(ctx);
                        }
                    });
                }
                Dialog::PresetError(msg) => {
                    ui.heading("エラー");
                    ui.label(msg);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("プリセット編集を開く").clicked() {
                            close = true;
                            self.dialog = None;
                            self.open_editor();
                        }
                        if ui.button("閉じる").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Error(msg) => {
                    ui.heading("エラー");
                    ui.label(msg);
                    ui.add_space(8.0);
                    if ui.button("閉じる").clicked() {
                        close = true;
                    }
                }
                Dialog::About => {
                    ui.horizontal(|ui| {
                        if let Some(tex) = about_icon(ui.ctx(), self.icon_png) {
                            ui.image((tex.id(), egui::vec2(64.0, 64.0)));
                        }
                        ui.vertical(|ui| {
                            ui.heading(sc_core::APP_NAME);
                            ui.label(format!("バージョン {}", sc_core::VERSION));
                        });
                    });
                    ui.label("携帯動画変換君の操作感を受け継いだ動画変換フロントエンド");
                    ui.label("ライセンス: MIT OR Apache-2.0");
                    ui.weak("サードパーティーライセンスの一覧は v0.9 の配布物に同梱します。");
                    ui.add_space(6.0);
                    ui.strong("置換変数");
                    for n in sc_core::vars::NAMES {
                        ui.monospace(format!("<%{n}%>"));
                    }
                    ui.add_space(8.0);
                    if ui.button("閉じる").clicked() {
                        close = true;
                    }
                }
            }
        });
        if close && self.dialog.as_ref() == Some(&dialog) {
            self.dialog = None;
        }
    }

    /// ログ画面とオプション画面を、別ウィンドウ（OS のウィンドウ）として出す。
    fn sub_windows(&mut self, ctx: &egui::Context) {
        if self.log_view.open {
            let mut builder = egui::ViewportBuilder::default()
                .with_title(format!("ログ - {}", sc_core::APP_NAME))
                .with_inner_size(fit_to_monitor(
                    ctx,
                    saved_size(&self.settings.log_window).unwrap_or(egui::vec2(760.0, 480.0)),
                ))
                .with_min_inner_size([480.0, 240.0]);
            if let Some(icon) = &self.icon {
                builder = builder.with_icon(icon.clone());
            }
            let running = self
                .worker
                .as_ref()
                .and_then(|_| self.queue.running().map(|j| j.id));
            let (log_view, log, queue) = (&mut self.log_view, &mut self.log, &self.queue);
            let mut size = None;
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("log_window"),
                builder,
                |ui, _| {
                    log_view.ui(ui, log, queue, running);
                    if ui.input(|i| i.viewport().close_requested()) {
                        log_view.open = false;
                    }
                    size = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
                },
            );
            if !self.log_view.open {
                if let Some(s) = size {
                    self.settings.log_window = format!("{},{}", s.x.round(), s.y.round());
                    self.settings_dirty = true;
                }
            }
        }

        if self.options.is_some() {
            let mut builder = egui::ViewportBuilder::default()
                .with_title(format!("オプション - {}", sc_core::APP_NAME))
                .with_inner_size(fit_to_monitor(ctx, egui::vec2(720.0, 480.0)))
                .with_min_inner_size([560.0, 360.0]);
            if let Some(icon) = &self.icon {
                builder = builder.with_icon(icon.clone());
            }
            let settings_path = self.settings_path.clone();
            let app_dir = self.app_dir.clone();
            let scripts = self.script_support.clone();
            let tools = std::mem::take(&mut self.tool_status);
            let options = self.options.as_mut().expect("is_some を確認済み");
            let mut action = None;
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("options_window"),
                builder,
                |ui, _| {
                    let env = OptionsEnv {
                        config_path: &settings_path,
                        app_dir: &app_dir,
                        tools: &tools,
                        scripts: &scripts,
                    };
                    action = options.ui(ui, &env);
                    if ui.input(|i| i.viewport().close_requested()) {
                        action = Some(OptionsAction::Cancel);
                    }
                },
            );
            self.tool_status = tools;

            // テーマと倍率はプレビューとしてすぐ反映する
            let draft = options.draft();
            let preview = (draft.theme, draft.ui_scale);
            if preview != self.appearance {
                self.appearance = preview;
                apply_appearance(ctx, preview.0, preview.1);
            }

            match action {
                Some(OptionsAction::Apply { settings, close }) => {
                    self.apply_options(*settings, ctx);
                    if close {
                        self.options = None;
                    }
                }
                Some(OptionsAction::Cancel) => {
                    self.options = None;
                    self.appearance = (self.settings.theme, self.settings.ui_scale);
                    apply_appearance(ctx, self.settings.theme, self.settings.ui_scale);
                }
                Some(OptionsAction::ImportOriginal) => self.import_original(ctx),
                Some(OptionsAction::RedetectScripts) => {
                    let tools = options.draft().tools.clone();
                    self.detect_scripts(Some(&tools));
                }
                Some(OptionsAction::OpenConfigDir) => {
                    if let Some(dir) = self.settings_path.parent() {
                        os::open_folder(dir, Some(&self.settings_path));
                    }
                }
                None => {}
            }
        }
    }

    /// プリセット編集画面を別ウィンドウで出す。
    fn editor_window(&mut self, ctx: &egui::Context) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let mut builder = egui::ViewportBuilder::default()
            .with_title(editor.window_title())
            .with_inner_size(fit_to_monitor(
                ctx,
                saved_size(&self.settings.preset_window).unwrap_or(egui::vec2(980.0, 620.0)),
            ))
            .with_min_inner_size([760.0, 480.0]);
        if let Some(icon) = &self.icon {
            builder = builder.with_icon(icon.clone());
        }
        let mut pending: HashMap<String, usize> = HashMap::new();
        for j in self
            .queue
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Pending)
        {
            *pending.entry(j.preset_title.clone()).or_default() += 1;
        }
        let output_dir = {
            let t = self.output_dir_text.trim();
            if t.is_empty() {
                std::env::temp_dir()
            } else {
                PathBuf::from(t)
            }
        };
        let check = CheckContext {
            tools: &self.settings.tools,
            app_dir: &self.app_dir,
            output_dir: &output_dir,
            pending: &pending,
        };
        let mut action = None;
        let mut size = None;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("preset_editor"),
            builder,
            |ui, _| {
                action = editor.ui(ui, &check);
                if ui.input(|i| i.viewport().close_requested()) {
                    // 未保存なら閉じずに確認する
                    if let Some(a) = editor.request_close() {
                        action = Some(a);
                    } else {
                        ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                    }
                }
                size = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
            },
        );

        let mut close = false;
        match action {
            Some(EditorAction::Saved { renames, close: c }) => {
                self.on_presets_saved(&renames);
                close = c;
            }
            Some(EditorAction::Error(e)) => {
                self.dialog = Some(Dialog::Error(format!("プリセットを保存できません。\n{e}")));
            }
            Some(EditorAction::Close) => close = true,
            None => {}
        }
        if close {
            self.editor = None;
            if let Some(s) = size {
                self.settings.preset_window = format!("{},{}", s.x.round(), s.y.round());
                self.settings_dirty = true;
            }
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) {
            crate::diag::op(format!(
                "閉じる要求（変換中 {}、確認なし {}）",
                self.worker.is_some(),
                self.allow_close
            ));
            if self.worker.is_some() && !self.allow_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.show_window(ctx);
                self.dialog = Some(Dialog::ConfirmClose);
            } else {
                self.save_settings(ctx);
                self.saved_on_close = true;
            }
        }
    }
}

impl eframe::App for App {
    /// 画面を描く前に毎回呼ばれ、最小化中・トレイに格納中も呼ばれる。
    /// 変換の進み具合・INI の監視・トレイの操作は、ここで処理する。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.terminate.swap(false, Ordering::SeqCst) {
            crate::diag::op("終了の要求（シグナル）");
            // 変換中なら中断する（Worker を捨てるときに、ツールが止まるのを待つ）。確認ダイアログは出さずに閉じる。
            if let Some(w) = &self.worker {
                w.abort();
            }
            self.stopped = true;
            self.allow_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        self.poll_worker(ctx);
        self.watch_presets();
        self.handle_tray(ctx);
        // 閉じる要求は、最小化中・トレイに格納中も処理する（ui は呼ばれないため。
        // Windows のタスクマネージャーの「タスクの終了」や、ログアウト時の終了の要求）。
        self.handle_close(ctx);

        // 実行中は経過時間を更新し、待機中も INI の変更やトレイの操作を確かめるために時々動かす。
        let wait = if self.worker.is_some() {
            Duration::from_millis(250)
        } else if self.is_background(ctx) {
            Duration::from_millis(500)
        } else {
            WATCH_INTERVAL
        };
        ctx.request_repaint_after(wait);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("controls").show(ui, |ui| {
            ui.add_space(6.0);
            self.controls(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("buttons").show(ui, |ui| {
            ui.add_space(4.0);
            self.buttons(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("detail").show(ui, |ui| {
            ui.add_space(4.0);
            self.detail(ui);
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ui, |ui| self.queue_table(ui));

        self.drop_overlay(&ctx);
        self.toast_ui(&ctx);
        self.sub_windows(&ctx);
        self.editor_window(&ctx);
        self.dialogs(&ctx);

        if self.settings_dirty && self.worker.is_none() {
            self.save_settings(&ctx);
        }
    }

    /// 終わるときに 1 回呼ばれる。画面が一度も描かれないまま終わる（Wayland で画面のロック中に
    /// 閉じられたなど、logic も呼ばれない）と、閉じる要求を処理しないので、ここで保存する。
    fn on_exit(&mut self) {
        if self.saved_on_close && !self.settings_dirty {
            return;
        }
        crate::diag::op(
            "終了時に設定を保存（閉じる要求を処理しないまま終わった、またはそのあとに変わった）",
        );
        let ctx = self.exit_ctx.clone();
        self.save_settings(&ctx);
    }
}

// ---------------------------------------------------------------------------
// 部品

enum RowAction {
    SetPreset(String),
    Retry,
    Move(Move),
    Remove,
    ShowLog(JobId),
    OpenOutput(JobId),
    OpenInput(JobId),
}

impl RowAction {
    /// 操作ログに書く名前（プリセット名とジョブ番号だけで、ファイル名は書かない）。
    fn name(&self) -> String {
        match self {
            Self::SetPreset(t) => format!("プリセットを「{t}」に"),
            Self::Retry => "やり直す".into(),
            Self::Move(_) => "並べ替え".into(),
            Self::Remove => "リストから削除".into(),
            Self::ShowLog(id) => format!("#{id} のログ"),
            Self::OpenOutput(id) => format!("#{id} の出力先を開く"),
            Self::OpenInput(id) => format!("#{id} の入力元を開く"),
        }
    }
}

fn row_menu(ui: &mut egui::Ui, job: &crate::queue::Job, titles: &[String]) -> Option<RowAction> {
    let mut action = None;
    let running = job.state == JobState::Running;
    ui.add_enabled_ui(!running, |ui| {
        ui.menu_button("プリセットを変更", |ui| {
            for t in titles {
                if ui.button(t).clicked() {
                    action = Some(RowAction::SetPreset(t.clone()));
                }
            }
        });
    });
    if ui
        .add_enabled(job.state.is_finished(), egui::Button::new("再実行"))
        .clicked()
    {
        action = Some(RowAction::Retry);
    }
    ui.separator();
    let pending = job.state == JobState::Pending;
    for (label, m) in [
        ("上へ    Alt+↑", Move::Up),
        ("下へ    Alt+↓", Move::Down),
        ("先頭へ", Move::Top),
        ("末尾へ", Move::Bottom),
    ] {
        if ui.add_enabled(pending, egui::Button::new(label)).clicked() {
            action = Some(RowAction::Move(m));
        }
    }
    ui.separator();
    if ui.button("ログを表示").clicked() {
        action = Some(RowAction::ShowLog(job.id));
    }
    if ui
        .add_enabled(
            job.output_base.is_some(),
            egui::Button::new("出力フォルダを開く"),
        )
        .clicked()
    {
        action = Some(RowAction::OpenOutput(job.id));
    }
    if ui.button("入力ファイルの場所を開く").clicked() {
        action = Some(RowAction::OpenInput(job.id));
    }
    ui.separator();
    if ui
        .add_enabled(!running, egui::Button::new("一覧から削除    Delete"))
        .clicked()
    {
        action = Some(RowAction::Remove);
    }
    if action.is_some() {
        ui.close();
    }
    action
}

fn progress_cell(ui: &mut egui::Ui, job: &crate::queue::Job, selected: bool) {
    let p = &job.progress;
    match job.state {
        JobState::Running => {
            let text = if p.command_count > 0 {
                format!("{}/{}", p.command, p.command_count)
            } else {
                String::new()
            };
            let bar = match p.update.ratio {
                Some(r) => egui::ProgressBar::new(r),
                None => egui::ProgressBar::new(0.0).animate(true),
            };
            add_progress(ui, bar.text(text).fill(progress_fill(ui.ctx(), selected)));
        }
        JobState::Succeeded => {
            add_progress(ui, egui::ProgressBar::new(1.0).show_percentage());
        }
        JobState::Failed | JobState::Aborted if p.command_count > 0 => {
            ui.label(format!("▲ {}/{}", p.command, p.command_count));
        }
        _ => {}
    }
}

fn state_style(state: JobState, ctx: &egui::Context, accent: Color32) -> (&'static str, Color32) {
    let dark = ctx.theme() == Theme::Dark;
    let pick = |light: [u8; 3], darkc: [u8; 3]| {
        let c = if dark { darkc } else { light };
        Color32::from_rgb(c[0], c[1], c[2])
    };
    let color = match state {
        JobState::Pending => ctx.style_of(ctx.theme()).visuals.text_color(),
        JobState::Running => accent,
        JobState::Succeeded => pick([0x16, 0xa3, 0x4a], [0x4a, 0xde, 0x80]),
        JobState::Failed => pick([0xdc, 0x26, 0x26], [0xf8, 0x71, 0x71]),
        JobState::Aborted | JobState::PresetMissing | JobState::Skipped => {
            pick([0xd9, 0x77, 0x06], [0xfb, 0xbf, 0x24])
        }
    };
    (state.label(), color)
}

/// 実行中の進捗バーの塗り。バーの文字はアクセント色（`selection.stroke`）で描かれるので、
/// 塗りは同じ色を暗く（半透明に）して、文字が塗りの上でも外でも読めるようにする。
/// 選択中の行は、行の選択色と見分けられるように少し濃くする。
fn progress_fill(ctx: &egui::Context, selected: bool) -> Color32 {
    accent(ctx).gamma_multiply(if selected { 0.7 } else { 0.45 })
}

/// 進捗バーを置く。ライトテーマではバーの地（`extreme_bg_color`、既定は白）を薄い灰色にして、
/// 背景と区別できるようにする（入力欄の地は変えないよう、バーの中だけで変える）。
fn add_progress(ui: &mut egui::Ui, bar: egui::ProgressBar) {
    ui.scope(|ui| {
        if !ui.visuals().dark_mode {
            ui.visuals_mut().extreme_bg_color = Color32::from_gray(218);
        }
        ui.add(bar.corner_radius(2.0));
    });
}

fn accent(ctx: &egui::Context) -> Color32 {
    if ctx.theme() == Theme::Dark {
        ACCENT_DARK
    } else {
        ACCENT_LIGHT
    }
}

fn error_color(ctx: &egui::Context) -> Color32 {
    if ctx.theme() == Theme::Dark {
        Color32::from_rgb(0xf8, 0x71, 0x71)
    } else {
        Color32::from_rgb(0xdc, 0x26, 0x26)
    }
}

/// テーマ・アクセントカラー・UI の倍率を設定する（画面設計 §3）。
pub fn apply_appearance(ctx: &egui::Context, theme: settings::Theme, scale: u16) {
    use sc_core::settings::Theme as T;
    ctx.set_theme(match theme {
        T::System => egui::ThemePreference::System,
        T::Light => egui::ThemePreference::Light,
        T::Dark => egui::ThemePreference::Dark,
    });
    for (t, color) in [(Theme::Light, ACCENT_LIGHT), (Theme::Dark, ACCENT_DARK)] {
        ctx.style_mut_of(t, |s| {
            s.visuals.selection.bg_fill = color.gamma_multiply(0.35);
            s.visuals.selection.stroke.color = color;
            s.visuals.hyperlink_color = color;
        });
    }
    ctx.set_zoom_factor(scale.clamp(80, 200) as f32 / 100.0);
}

fn about_icon(ctx: &egui::Context, png: &[u8]) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("about_icon_texture");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let data = eframe::icon_data::from_png_bytes(png).ok()?;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [data.width as usize, data.height as usize],
        &data.rgba,
    );
    let tex = ctx.load_texture("about_icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// サブ画面を開くときの大きさを、画面（モニター）の 85% までに収める。
/// 拡大率が大きいと、既定の大きさでも画面の高さを超え、下のボタンがタスクバーに隠れるため。
fn fit_to_monitor(ctx: &egui::Context, size: egui::Vec2) -> egui::Vec2 {
    match ctx.input(|i| i.viewport().monitor_size) {
        Some(m) if m.x > 0.0 && m.y > 0.0 => size.min(m * 0.85),
        _ => size,
    }
}

fn saved_size(s: &str) -> Option<egui::Vec2> {
    let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    match v.as_slice() {
        [w, h] if *w >= 300.0 && *h >= 200.0 => Some(egui::vec2(*w, *h)),
        _ => None,
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// 今から `after` 後の時刻（HH:MM）。
fn clock_after(after: Duration) -> String {
    let t: chrono::DateTime<chrono::Local> = (SystemTime::now() + after).into();
    t.format("%H:%M").to_string()
}
