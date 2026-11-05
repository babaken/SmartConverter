//! プリセット編集画面（画面設計 S-04、要件 F-PE-01〜05）。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText};
use sc_core::naming::{OnExists, TimestampOptions, TimestampPos};
use sc_core::plan::{self, PlanOptions};
use sc_core::preset::{self, Preset, PresetInfo, PresetSet};
use sc_core::text::TextEncoding;

/// 外部での変更を確かめる間隔。
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// 呼び出し側に返す操作。
pub enum EditorAction {
    /// 保存した（タイトルを変えたプリセットの (前, 後)）。`close` なら保存後に画面を閉じる
    Saved {
        renames: Vec<(String, String)>,
        close: bool,
    },
    /// 保存に失敗した
    Error(String),
    /// 画面を閉じる
    Close,
}

/// 確認欄（dry-run）に必要な設定。
pub struct CheckContext<'a> {
    pub tools: &'a BTreeMap<String, String>,
    pub app_dir: &'a Path,
    pub output_dir: &'a Path,
    /// プリセットのタイトルごとの、待機中のジョブの数（削除の確認に使う）
    pub pending: &'a HashMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct Item {
    preset: Preset,
    /// 読み込んだときのタイトル（新しく追加したものは None）
    original_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Dialog {
    Delete(usize),
    Revert,
    CloseUnsaved,
}

/// 確認欄の 1 行。
#[derive(Debug, Clone)]
struct CheckLine {
    text: String,
    notes: Vec<(Severity, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
    Info,
}

pub struct PresetEditor {
    path: PathBuf,
    base: PresetSet,
    mtime: Option<SystemTime>,
    info: PresetInfo,
    items: Vec<Item>,
    saved: (PresetInfo, Vec<Item>),
    selected: usize,
    /// 最後に入力していたコマンドの行と、カーソル位置（文字数）
    cursor: Option<(usize, usize)>,
    sample: String,
    external_changed: bool,
    last_watch: Instant,
    dialog: Option<Dialog>,
    check_cache: Option<(Vec<String>, String, Vec<CheckLine>)>,
}

impl PresetEditor {
    /// ファイルを開く。無ければ空のプリセット集として開く（保存すると作られる）。
    pub fn open(path: &Path, sample: Option<&Path>) -> Result<Self, String> {
        let base = match std::fs::read(path) {
            Ok(bytes) => {
                let (text, enc) = sc_core::text::decode(&bytes);
                let mut set = PresetSet::parse(&text, enc);
                set.path = Some(path.to_owned());
                set
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                PresetSet::parse("", TextEncoding::Utf8)
            }
            Err(e) => return Err(format!("{} を読み込めません: {e}", path.display())),
        };
        Ok(Self::from_set(path, base, sample))
    }

    fn from_set(path: &Path, base: PresetSet, sample: Option<&Path>) -> Self {
        let items: Vec<Item> = base
            .presets
            .iter()
            .map(|p| Item {
                preset: p.clone(),
                original_title: Some(p.title.clone()),
            })
            .collect();
        let sample = sample
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "sample.ts".to_owned());
        Self {
            path: path.to_owned(),
            mtime: file_mtime(path),
            info: base.info.clone(),
            saved: (base.info.clone(), items.clone()),
            items,
            base,
            selected: 0,
            cursor: None,
            sample,
            external_changed: false,
            last_watch: Instant::now(),
            dialog: None,
            check_cache: None,
        }
    }

    /// 窓のタイトル（読み込み時の文字コードと、保存すると UTF-8 になることを示す）。
    pub fn window_title(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let enc = self.base.encoding;
        if enc == TextEncoding::Utf8 {
            format!("プリセット編集 - {name}")
        } else {
            format!(
                "プリセット編集 - {name}（{} → 保存時 UTF-8）",
                enc.display_name()
            )
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.info != self.saved.0 || self.items != self.saved.1
    }

    fn presets(&self) -> Vec<Preset> {
        self.items.iter().map(|i| i.preset.clone()).collect()
    }

    /// 閉じるよう求められたとき（窓の × など）。未保存なら確認する。
    pub fn request_close(&mut self) -> Option<EditorAction> {
        if self.is_dirty() {
            self.dialog = Some(Dialog::CloseUnsaved);
            None
        } else {
            Some(EditorAction::Close)
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, check: &CheckContext<'_>) -> Option<EditorAction> {
        let mut action = None;
        self.watch_file();

        // 上部：プリセット集の名前・説明、外部での変更の帯
        egui::Panel::top("pe_top").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let l = ui.label("プリセット集の名前");
                // 名前は幅の約 4 割（最低 200）、説明は残り
                let w = (ui.available_width() * 0.4).max(200.0);
                ui.add(egui::TextEdit::singleline(&mut self.info.title).desired_width(w))
                    .labelled_by(l.id);
                let l = ui.label("説明");
                ui.add(
                    egui::TextEdit::singleline(&mut self.info.description)
                        .desired_width(ui.available_width()),
                )
                .labelled_by(l.id);
            });
            if self.external_changed {
                ui.horizontal(|ui| {
                    ui.colored_label(warn_color(ui), "ファイルが外部で変更されました");
                    if ui.button("再読み込み").clicked() {
                        self.reload();
                    }
                    if ui.button("このまま編集").clicked() {
                        self.external_changed = false;
                        self.mtime = file_mtime(&self.path);
                    }
                });
            }
            ui.add_space(4.0);
        });

        // 下部：未保存の表示・保存できない理由・ボタン
        let problems = preset::validate(&self.presets());
        egui::Panel::bottom("pe_bottom").show(ui, |ui| {
            ui.add_space(4.0);
            for p in &problems {
                ui.colored_label(error_color(ui), format!("✗ {p}"));
            }
            ui.horizontal(|ui| {
                if self.is_dirty() {
                    ui.label("● 未保存の変更があります");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("閉じる").clicked() {
                        action = self.request_close();
                    }
                    if ui
                        .add_enabled(self.is_dirty(), egui::Button::new("元に戻す"))
                        .clicked()
                    {
                        self.dialog = Some(Dialog::Revert);
                    }
                    let can_save = problems.is_empty() && self.is_dirty();
                    if ui
                        .add_enabled(can_save, egui::Button::new("保存"))
                        .on_hover_text("Ctrl+S")
                        .clicked()
                    {
                        action = Some(self.save());
                    }
                });
            });
            ui.add_space(4.0);
        });

        // 左：プリセット一覧
        egui::Panel::left("pe_list")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| self.list_ui(ui));

        // 本体：タイトル・コマンド・確認欄
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    if self.items.is_empty() {
                        ui.weak("［追加］でプリセットを作ってください");
                        return;
                    }
                    self.selected = self.selected.min(self.items.len() - 1);
                    self.edit_ui(ui);
                    ui.separator();
                    self.check_ui(ui, check);
                });
        });

        if let Some(a) = self.dialogs(ui.ctx(), check) {
            action = Some(a);
        }

        let save_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
        if ui.input_mut(|i| i.consume_shortcut(&save_key)) && problems.is_empty() && self.is_dirty()
        {
            action = Some(self.save());
        }
        action
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.strong("プリセット");
        let list_height = (ui.available_height() - 110.0).max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (i, item) in self.items.iter().enumerate() {
                    let title = if item.preset.title.trim().is_empty() {
                        "（タイトルなし）".to_owned()
                    } else {
                        item.preset.title.clone()
                    };
                    if ui.selectable_label(self.selected == i, title).clicked() {
                        self.selected = i;
                        self.cursor = None;
                    }
                }
            });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("追加").clicked() {
                self.items.push(Item {
                    preset: Preset {
                        title: self.unique_title("新しいプリセット"),
                        commands: vec![String::new()],
                        ..Default::default()
                    },
                    original_title: None,
                });
                self.selected = self.items.len() - 1;
            }
            let has = !self.items.is_empty();
            if ui.add_enabled(has, egui::Button::new("複製")).clicked() {
                let mut copy = self.items[self.selected].clone();
                copy.preset.title = self.unique_title(&format!("{} のコピー", copy.preset.title));
                copy.original_title = None;
                self.items.insert(self.selected + 1, copy);
                self.selected += 1;
            }
            if ui.add_enabled(has, egui::Button::new("削除")).clicked() {
                self.dialog = Some(Dialog::Delete(self.selected));
            }
            if ui
                .add_enabled(has && self.selected > 0, egui::Button::new("↑"))
                .on_hover_text("上へ")
                .clicked()
            {
                self.items.swap(self.selected, self.selected - 1);
                self.selected -= 1;
            }
            if ui
                .add_enabled(
                    has && self.selected + 1 < self.items.len(),
                    egui::Button::new("↓"),
                )
                .on_hover_text("下へ")
                .clicked()
            {
                self.items.swap(self.selected, self.selected + 1);
                self.selected += 1;
            }
        });
        ui.add_space(6.0);
        let ignored = &self.base.ignored_sections;
        egui::CollapsingHeader::new(format!("保持されるセクション（{}）", ignored.len()))
            .default_open(false)
            .show(ui, |ui| {
                ui.weak("プリセットとして読まれないセクションです。保存してもそのまま残ります。");
                for s in ignored {
                    ui.monospace(format!("[{s}]"));
                }
            });
    }

    fn edit_ui(&mut self, ui: &mut egui::Ui) {
        let sel = self.selected;
        let titles: Vec<String> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != sel)
            .map(|(_, it)| it.preset.title.trim().to_owned())
            .collect();
        let item = &mut self.items[sel];

        ui.horizontal(|ui| {
            let l = ui.label("タイトル");
            let t = item.preset.title.trim();
            let bad = t.is_empty() || titles.iter().any(|x| x == t);
            let mut edit = egui::TextEdit::singleline(&mut item.preset.title)
                .desired_width(ui.available_width());
            if bad {
                edit = edit.text_color(error_color(ui));
            }
            ui.add(edit).labelled_by(l.id);
        });
        ui.add_space(6.0);
        let commands_label = ui.strong("コマンド");

        let mut action: Option<(usize, CmdAction)> = None;
        let count = item.preset.commands.len();
        // 番号・入力欄・ボタンを 1 行ずつ横に並べる。入力欄は残りの幅いっぱいにする
        // （表（Grid）に入れると、入力欄の幅がセルの幅に切り詰められるため）
        for (i, cmd) in item.preset.commands.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.monospace(format!("{i:>2}"));
                let edit_width = (ui.available_width() - 110.0).max(240.0);
                let id = cmd_id(sel, i);
                let resp = ui.add_sized(
                    [edit_width, ui.spacing().interact_size.y],
                    egui::TextEdit::singleline(cmd)
                        .id(id)
                        .font(egui::TextStyle::Monospace),
                );
                let resp = resp.labelled_by(commands_label.id);
                if resp.has_focus() {
                    let pos = egui::TextEdit::load_state(ui.ctx(), id)
                        .and_then(|s| s.cursor.char_range())
                        .map_or(cmd.chars().count(), |r| r.primary.index.into());
                    self.cursor = Some((i, pos));
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("↑").small())
                    .on_hover_text("上へ")
                    .clicked()
                {
                    action = Some((i, CmdAction::Up));
                }
                if ui
                    .add_enabled(i + 1 < count, egui::Button::new("↓").small())
                    .on_hover_text("下へ")
                    .clicked()
                {
                    action = Some((i, CmdAction::Down));
                }
                if ui.button("✕").on_hover_text("この行を削除").clicked() {
                    action = Some((i, CmdAction::Remove));
                }
            });
        }
        if let Some((i, a)) = action {
            let cmds = &mut item.preset.commands;
            match a {
                CmdAction::Up => cmds.swap(i, i - 1),
                CmdAction::Down => cmds.swap(i, i + 1),
                CmdAction::Remove => {
                    cmds.remove(i);
                }
            }
            self.cursor = None;
        }

        ui.horizontal(|ui| {
            if ui.button("＋ コマンドを追加").clicked() {
                item.preset.commands.push(String::new());
            }
            let l = ui.label("変数を挿入");
            let mut chosen: Option<&str> = None;
            let combo = egui::ComboBox::from_id_salt("pe_insert_var")
                .selected_text("選択…")
                .show_ui(ui, |ui| {
                    for n in sc_core::vars::NAMES {
                        if ui.selectable_label(false, format!("<%{n}%>")).clicked() {
                            chosen = Some(n);
                        }
                    }
                });
            combo.response.labelled_by(l.id);
            if let Some(name) = chosen {
                let text = format!("<%{name}%>");
                let cmds = &mut item.preset.commands;
                if cmds.is_empty() {
                    cmds.push(String::new());
                }
                let (row, pos) = self
                    .cursor
                    .filter(|(r, _)| *r < cmds.len())
                    .unwrap_or((cmds.len() - 1, cmds[cmds.len() - 1].chars().count()));
                let s = &mut cmds[row];
                let byte = s.char_indices().nth(pos).map_or(s.len(), |(b, _)| b);
                s.insert_str(byte, &text);
                let new_pos = pos + text.chars().count();
                self.cursor = Some((row, new_pos));
                // カーソルを挿入した変数の後ろに置いて、入力欄に戻る
                let id = cmd_id(sel, row);
                let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text_selection::CCursorRange::one(
                        egui::text::CCursor::new(new_pos),
                    )));
                egui::TextEdit::store_state(ui.ctx(), id, state);
                ui.memory_mut(|m| m.request_focus(id));
            }
        });
    }

    fn check_ui(&mut self, ui: &mut egui::Ui, check: &CheckContext<'_>) {
        ui.strong("確認（変数を展開した結果）");
        ui.horizontal(|ui| {
            let l = ui.label("サンプル入力");
            ui.add(egui::TextEdit::singleline(&mut self.sample).desired_width(360.0))
                .labelled_by(l.id);
            if ui.button("選択").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_file() {
                    self.sample = p.display().to_string();
                }
            }
        });

        let commands = self.items[self.selected].preset.commands.clone();
        let fresh =
            matches!(&self.check_cache, Some((c, s, _)) if *c == commands && *s == self.sample);
        if !fresh {
            let lines = build_check(&commands, &self.sample, check);
            self.check_cache = Some((commands, self.sample.clone(), lines));
        }
        let lines = &self.check_cache.as_ref().expect("直前に作った").2;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (i, line) in lines.iter().enumerate() {
                ui.label(RichText::new(format!("{i}: {}", line.text)).monospace());
                for (sev, note) in &line.notes {
                    let (mark, color) = match sev {
                        Severity::Error => ("✗", error_color(ui)),
                        Severity::Warning => ("⚠", warn_color(ui)),
                        Severity::Info => ("ⓘ", ui.visuals().weak_text_color()),
                    };
                    ui.colored_label(color, format!("    {mark} {note}"));
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, check: &CheckContext<'_>) -> Option<EditorAction> {
        let dialog = self.dialog.clone()?;
        let mut action = None;
        let mut close = false;
        egui::Modal::new(egui::Id::unique("pe_dialog")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading("確認");
            match &dialog {
                Dialog::Delete(i) => {
                    let title = self
                        .items
                        .get(*i)
                        .map(|it| it.preset.title.clone())
                        .unwrap_or_default();
                    ui.label(format!("プリセット「{title}」を削除しますか？"));
                    let orig = self.items.get(*i).and_then(|it| it.original_title.clone());
                    if let Some(n) = orig
                        .and_then(|t| check.pending.get(&t).copied())
                        .filter(|n| *n > 0)
                    {
                        ui.colored_label(
                            warn_color(ui),
                            format!("待機中のジョブ {n} 件がこのプリセットを使っています"),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("キャンセル").clicked() {
                            close = true;
                        }
                        if ui.button("削除").clicked() {
                            if *i < self.items.len() {
                                self.items.remove(*i);
                                self.selected =
                                    self.selected.min(self.items.len().saturating_sub(1));
                                self.cursor = None;
                            }
                            close = true;
                        }
                    });
                }
                Dialog::Revert => {
                    ui.label("保存していない変更を破棄しますか？");
                    ui.horizontal(|ui| {
                        if ui.button("キャンセル").clicked() {
                            close = true;
                        }
                        if ui.button("破棄").clicked() {
                            self.info = self.saved.0.clone();
                            self.items = self.saved.1.clone();
                            self.selected = self.selected.min(self.items.len().saturating_sub(1));
                            self.cursor = None;
                            close = true;
                        }
                    });
                }
                Dialog::CloseUnsaved => {
                    ui.label("変更を保存しますか？");
                    ui.horizontal(|ui| {
                        let can_save = preset::validate(&self.presets()).is_empty();
                        if ui
                            .add_enabled(can_save, egui::Button::new("保存"))
                            .clicked()
                        {
                            close = true;
                            // 保存してから閉じる（タイトル変更の反映は呼び出し側で行う）
                            action = Some(match self.save() {
                                EditorAction::Saved { renames, .. } => EditorAction::Saved {
                                    renames,
                                    close: true,
                                },
                                other => other,
                            });
                        }
                        if ui.button("保存しない").clicked() {
                            close = true;
                            action = Some(EditorAction::Close);
                        }
                        if ui.button("キャンセル").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if close {
            self.dialog = None;
        }
        action
    }

    /// 保存する。
    fn save(&mut self) -> EditorAction {
        let presets = self.presets();
        if let Err(e) = self.base.save(&self.path, &self.info, &presets) {
            return EditorAction::Error(e.to_string());
        }
        let renames: Vec<(String, String)> = self
            .items
            .iter()
            .filter_map(|it| match &it.original_title {
                Some(old) if *old != it.preset.title => {
                    Some((old.clone(), it.preset.title.clone()))
                }
                _ => None,
            })
            .collect();
        // 保存したファイルを読み直して、次の保存の元にする
        let selected = self.selected;
        match PresetEditor::open(&self.path, Some(Path::new(&self.sample))) {
            Ok(mut fresh) => {
                fresh.selected = selected.min(fresh.items.len().saturating_sub(1));
                fresh.sample = std::mem::take(&mut self.sample);
                *self = fresh;
            }
            Err(e) => return EditorAction::Error(e),
        }
        EditorAction::Saved {
            renames,
            close: false,
        }
    }

    fn reload(&mut self) {
        let selected = self.selected;
        let sample = self.sample.clone();
        if let Ok(mut fresh) = PresetEditor::open(&self.path, None) {
            fresh.selected = selected.min(fresh.items.len().saturating_sub(1));
            fresh.sample = sample;
            *self = fresh;
        }
    }

    /// 外部での変更を確かめる。未保存の変更がなければ自動で読み直す（F-PE-05）。
    fn watch_file(&mut self) {
        if self.last_watch.elapsed() < WATCH_INTERVAL {
            return;
        }
        self.last_watch = Instant::now();
        let now = file_mtime(&self.path);
        if now.is_some() && now != self.mtime && !self.external_changed {
            if self.is_dirty() {
                self.external_changed = true;
            } else {
                self.reload();
            }
        }
    }

    fn unique_title(&self, base: &str) -> String {
        let exists = |t: &str| self.items.iter().any(|it| it.preset.title == t);
        if !exists(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} ({n})"))
            .find(|t| !exists(t))
            .expect("いつか空きが見つかる")
    }
}

enum CmdAction {
    Up,
    Down,
    Remove,
}

fn cmd_id(preset: usize, row: usize) -> egui::Id {
    egui::Id::unique(("pe_cmd", preset, row))
}

/// 確認欄の内容を作る（変数の展開とツールの検出）。
fn build_check(commands: &[String], sample: &str, check: &CheckContext<'_>) -> Vec<CheckLine> {
    let preset = Preset {
        commands: commands.to_vec(),
        ..Default::default()
    };
    let ts = TimestampOptions {
        enabled: false,
        pos: TimestampPos::HeadWithSpace,
        format: String::new(),
    };
    let temp = std::env::temp_dir();
    let opts = PlanOptions {
        output_dir: check.output_dir,
        temporary_dir: &temp,
        // 確認だけなので、同名ファイルの有無で名前を変えない
        on_exists: OnExists::Overwrite,
        timestamp: &ts,
        tools: check.tools,
        app_dir: check.app_dir,
        path_env: std::env::var_os("PATH"),
        script: None,
    };
    let input = Path::new(sample);
    let mtime = plan::file_mtime(input);
    let job = plan::plan_job(input, &mtime, &preset, 0, &opts);
    job.commands
        .into_iter()
        .map(|c| {
            let mut notes = Vec::new();
            if c.template.trim().is_empty() {
                notes.push((Severity::Error, "コマンドが空です".to_owned()));
            } else {
                for u in &c.unknown_vars {
                    notes.push((
                        Severity::Warning,
                        format!("未定義の変数 {u}（置き換えずに文字のまま実行されます）"),
                    ));
                }
                match (&c.program, &c.resolved) {
                    (None, _) => notes.push((Severity::Error, "コマンドを解釈できません".into())),
                    (Some(p), None) => notes.push((
                        Severity::Warning,
                        format!("{p} が見つかりません（オプションの［ツール］で指定できます）"),
                    )),
                    _ => {}
                }
                if c.is_cleanup {
                    notes.push((
                        Severity::Info,
                        "一時ファイルはアプリが自動で削除するため、この行は不要です".into(),
                    ));
                }
                let lower = c.template.trim_start().to_ascii_lowercase();
                if cfg!(not(windows)) && (lower.starts_with("cmd ") || lower.starts_with("cmd.exe"))
                {
                    notes.push((Severity::Warning, "この行は Windows 専用です".into()));
                }
            }
            CheckLine {
                text: c.expanded,
                notes,
            }
        })
        .collect()
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn error_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0xf8, 0x71, 0x71)
    } else {
        Color32::from_rgb(0xdc, 0x26, 0x26)
    }
}

fn warn_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0xfb, 0xbf, 0x24)
    } else {
        Color32::from_rgb(0xd9, 0x77, 0x06)
    }
}

/// プリセット編集画面の操作テスト（egui_kittest）。
#[cfg(test)]
mod ui_tests {
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};
    use egui_kittest::Harness;

    use super::*;

    const INI: &str = "[Info]\r\nTitle=テスト集\r\nDescription=説明\r\n\r\n[Item0]\r\nTitle=A\r\nCommand0=\"\"ffmpeg\" -i \"<%InputFile%>\" \"<%OutputFile%>.mp4\"\"\r\nCommand1=\"x \"<%Bad%>\"\"\r\nCommand2=\"cmd /c \"del \"<%TemporaryFile%>*.*\"\"\"\r\n\r\n[Item1]\r\nTitle=B\r\nCommand0=\"y\"\r\n\r\n[0Item]\r\nTitle=保管\r\n";

    enum Got {
        Saved(Vec<(String, String)>, bool),
        Error(String),
        Close,
    }

    struct State {
        editor: PresetEditor,
        got: Option<Got>,
        pending: HashMap<String, usize>,
        dir: PathBuf,
    }

    struct Env {
        h: Harness<'static, State>,
    }

    impl Env {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sc_pe_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("Transcoding.ini");
            std::fs::write(&path, INI).unwrap();
            let state = State {
                editor: PresetEditor::open(&path, None).unwrap(),
                got: None,
                pending: HashMap::from([("A".to_owned(), 2)]),
                dir,
            };
            let mut h = Harness::builder()
                .with_size(egui::vec2(980.0, 620.0))
                .allow_missing_glyphs()
                .build_ui_state(
                    |ui, s: &mut State| {
                        let tools = BTreeMap::new();
                        let check = CheckContext {
                            tools: &tools,
                            app_dir: &s.dir,
                            output_dir: &s.dir,
                            pending: &s.pending,
                        };
                        if let Some(a) = s.editor.ui(ui, &check) {
                            s.got = Some(match a {
                                EditorAction::Saved { renames, close } => {
                                    Got::Saved(renames, close)
                                }
                                EditorAction::Error(e) => Got::Error(e),
                                EditorAction::Close => Got::Close,
                            });
                        }
                    },
                    state,
                );
            h.run();
            Self { h }
        }

        fn click(&mut self, label: &str) {
            self.h.get_by_label(label).click();
            self.h.run();
        }

        /// 同じ名前の部品が複数あるとき、n 番目を押す。
        fn click_nth(&mut self, label: &str, n: usize) {
            self.h.get_all_by_label(label).nth(n).unwrap().click();
            self.h.run();
        }

        fn type_into(&mut self, role_label: &str, text: &str) {
            self.h
                .get_by_role_and_label(Role::TextInput, role_label)
                .focus();
            self.h.run();
            self.h
                .get_by_role_and_label(Role::TextInput, role_label)
                .type_text(text);
            self.h.run();
        }

        fn titles(&self) -> Vec<String> {
            self.h
                .state()
                .editor
                .items
                .iter()
                .map(|i| i.preset.title.clone())
                .collect()
        }

        fn path(&self) -> PathBuf {
            self.h.state().dir.join("Transcoding.ini")
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.h.state().dir);
        }
    }

    #[test]
    fn select_shows_title_and_ignored_sections() {
        let mut e = Env::new("select");
        let title = |e: &Env| {
            e.h.get_by_role_and_label(Role::TextInput, "タイトル")
                .value()
        };
        assert_eq!(title(&e).as_deref(), Some("A"));
        e.click("B");
        assert_eq!(title(&e).as_deref(), Some("B"));
        e.click("保持されるセクション（1）");
        assert!(e.h.query_by_label("[0Item]").is_some());
        assert!(e
            .h
            .state()
            .editor
            .window_title()
            .starts_with("プリセット編集 - Transcoding.ini"));
    }

    #[test]
    fn add_shows_problem_and_blocks_save() {
        let mut e = Env::new("add");
        e.click("追加");
        assert_eq!(e.titles(), vec!["A", "B", "新しいプリセット"]);
        assert!(e
            .h
            .query_by_label("✗ 「新しいプリセット」にコマンドがありません")
            .is_some());
        assert!(e.h.get_by_label("保存").accesskit_node().is_disabled());
        assert!(e.h.query_by_label("● 未保存の変更があります").is_some());
    }

    #[test]
    fn duplicate_and_move() {
        let mut e = Env::new("dup");
        e.click("複製");
        assert_eq!(e.titles(), vec!["A", "A のコピー", "B"]);
        assert_eq!(e.h.state().editor.selected, 1);
        // 一覧の「↓」（コマンド行にも同じ名前のボタンがあるので最初のもの）
        e.click_nth("↓", 0);
        assert_eq!(e.titles(), vec!["A", "B", "A のコピー"]);
        e.click_nth("↑", 0);
        assert_eq!(e.titles(), vec!["A", "A のコピー", "B"]);
    }

    #[test]
    fn delete_asks_and_shows_pending_jobs() {
        let mut e = Env::new("delete");
        e.click("削除");
        assert!(e
            .h
            .query_by_label("プリセット「A」を削除しますか？")
            .is_some());
        assert!(e
            .h
            .query_by_label("待機中のジョブ 2 件がこのプリセットを使っています")
            .is_some());
        // ダイアログの「削除」（一覧のボタンの後ろにある）
        let n = e.h.get_all_by_label("削除").count();
        e.click_nth("削除", n - 1);
        assert_eq!(e.titles(), vec!["B"]);
    }

    #[test]
    fn check_area_shows_warnings() {
        let e = Env::new("check");
        assert!(e
            .h
            .query_by_label_contains("未定義の変数 <%Bad%>")
            .is_some());
        assert!(e.h.query_by_label_contains("この行は不要です").is_some());
        assert!(e.h.query_by_label_contains("x が見つかりません").is_some());
    }

    #[test]
    fn insert_variable_at_cursor() {
        let mut e = Env::new("insert");
        e.click("B");
        // コマンド欄にカーソルを置いてから、変数を選ぶ
        let find_y = |e: &Env| {
            e.h.get_all_by_role(Role::TextInput)
                .position(|n| n.value().as_deref() == Some("y"))
                .unwrap()
        };
        let i = find_y(&e);
        e.h.get_all_by_role(Role::TextInput).nth(i).unwrap().focus();
        e.h.run();
        e.h.get_all_by_role(Role::TextInput)
            .nth(i)
            .unwrap()
            .type_text(" ");
        e.h.run();
        e.h.get_by_role_and_label(Role::ComboBox, "変数を挿入")
            .click();
        e.h.run();
        e.click("<%InputFile%>");
        assert_eq!(
            e.h.state().editor.items[1].preset.commands[0],
            "y <%InputFile%>"
        );
    }

    #[test]
    fn save_writes_file_and_reports_rename() {
        let mut e = Env::new("save");
        e.type_into("タイトル", "2");
        e.click("保存");
        match &e.h.state().got {
            Some(Got::Saved(renames, close)) => {
                assert_eq!(renames, &vec![("A".to_owned(), "A2".to_owned())]);
                assert!(!close);
            }
            Some(Got::Error(err)) => panic!("保存に失敗: {err}"),
            _ => panic!("保存されていない"),
        }
        let set = PresetSet::load(&e.path()).unwrap();
        assert_eq!(set.presets[0].title, "A2");
        assert_eq!(set.ignored_sections, vec!["0Item"]);
        assert!(e.path().with_extension("ini.bak").is_file());
        // 保存後は未保存の表示が消える
        assert!(!e.h.state().editor.is_dirty());
    }

    #[test]
    fn close_with_unsaved_changes_asks() {
        let mut e = Env::new("close");
        e.click("追加");
        e.click("閉じる");
        assert!(e.h.query_by_label("変更を保存しますか？").is_some());
        e.click("保存しない");
        assert!(matches!(e.h.state().got, Some(Got::Close)));
    }

    #[test]
    fn revert_discards_changes() {
        let mut e = Env::new("revert");
        e.click("追加");
        e.click("元に戻す");
        e.click("破棄");
        assert_eq!(e.titles(), vec!["A", "B"]);
        assert!(!e.h.state().editor.is_dirty());
    }

    #[test]
    fn external_change_reloads_when_clean() {
        let mut e = Env::new("external");
        let changed = INI.replace("Title=B", "Title=外部で変更");
        // 更新日時が確実に変わるよう少し待ってから書く
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(e.path(), changed).unwrap();
        e.h.state_mut().editor.last_watch = Instant::now() - WATCH_INTERVAL;
        e.h.run();
        assert_eq!(e.titles(), vec!["A", "外部で変更"]);
    }
}
