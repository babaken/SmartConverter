//! ログ画面（画面設計 S-02）。

use std::collections::BTreeSet;
use std::time::SystemTime;

use eframe::egui::{self, Color32, RichText, Theme};

use crate::queue::{JobId, Queue};

/// ログの最大行数（全体）。超えたら古い順に 1 割捨てる。
pub const LOG_LIMIT: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    JobStart,
    Command,
    Output,
    Success,
    Failure,
    Info,
    Warning,
}

/// ログ 1 行。
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub job: JobId,
    pub time: SystemTime,
    pub kind: LogKind,
    pub text: String,
}

impl LogEntry {
    /// 画面・コピー・保存に使う 1 行の文字列。
    pub fn format(&self) -> String {
        let t: chrono::DateTime<chrono::Local> = self.time.into();
        let mark = match self.kind {
            LogKind::JobStart => "━━",
            LogKind::Command => "▶",
            LogKind::Output => "  ",
            LogKind::Success => "✓",
            LogKind::Failure => "✗",
            LogKind::Info => "ⓘ",
            LogKind::Warning => "⚠",
        };
        format!("{} {mark} {}", t.format("%H:%M:%S"), self.text)
    }
}

/// 全ジョブのログ。
#[derive(Debug, Default)]
pub struct Log {
    pub entries: Vec<LogEntry>,
}

impl Log {
    pub fn push(&mut self, entry: LogEntry) {
        if self.entries.len() >= LOG_LIMIT {
            self.entries.drain(..LOG_LIMIT / 10);
        }
        self.entries.push(entry);
    }

    /// `\r` で上書きされた出力行：同じジョブの直前の出力行を置き換える。
    pub fn replace_last_output(&mut self, entry: LogEntry) {
        match self.entries.last_mut() {
            Some(last) if last.job == entry.job && last.kind == LogKind::Output => *last = entry,
            _ => self.push(entry),
        }
    }
}

/// ログ画面の状態。
#[derive(Debug)]
pub struct LogView {
    pub open: bool,
    /// `None` ならすべてのジョブ
    pub filter: Option<JobId>,
    search: String,
    autoscroll: bool,
    /// 選択中の行（`Log::entries` の番号）
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
}

impl Default for LogView {
    fn default() -> Self {
        Self {
            open: false,
            filter: None,
            search: String::new(),
            autoscroll: true,
            selected: BTreeSet::new(),
            anchor: None,
        }
    }
}

impl LogView {
    /// 指定したジョブに絞って開く。
    pub fn open_for(&mut self, job: Option<JobId>) {
        self.open = true;
        self.filter = job;
        self.selected.clear();
    }

    /// ログ画面を描く。消去を押されたら、消してよい行の判定に使う「実行中のジョブ」を受け取る。
    pub fn ui(&mut self, ui: &mut egui::Ui, log: &mut Log, queue: &Queue, running: Option<JobId>) {
        // 上部：絞り込み・検索・自動スクロール
        egui::Panel::top("log_top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let name_label = ui.label("表示");
                let label = match self.filter.and_then(|id| queue.get(id)) {
                    Some(j) => format!("#{} {}", j.id, j.file_name()),
                    None => "すべてのジョブ".to_owned(),
                };
                let combo = egui::ComboBox::from_id_salt("log_filter")
                    .selected_text(label)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.filter.is_none(), "すべてのジョブ")
                            .clicked()
                        {
                            self.filter = None;
                            self.selected.clear();
                        }
                        for j in &queue.jobs {
                            let text = format!("#{} {}", j.id, j.file_name());
                            if ui
                                .selectable_label(self.filter == Some(j.id), text)
                                .clicked()
                            {
                                self.filter = Some(j.id);
                                self.selected.clear();
                            }
                        }
                    });
                combo.response.labelled_by(name_label.id);
                ui.label("🔍");
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("検索")
                        .desired_width(180.0),
                );
                ui.checkbox(&mut self.autoscroll, "自動スクロール");
            });
            ui.add_space(4.0);
        });

        let visible: Vec<usize> = log
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.filter.is_none_or(|f| e.job == f))
            .filter(|(_, e)| {
                self.search.is_empty()
                    || e.text.to_lowercase().contains(&self.search.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect();

        // 下部：ボタン
        let mut close = false;
        let mut clear = false;
        egui::Panel::bottom("log_bottom").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.selected.is_empty(),
                        egui::Button::new("選択行をコピー"),
                    )
                    .clicked()
                {
                    ui.ctx()
                        .copy_text(self.text_of(log, self.selected.iter().copied()));
                }
                if ui.button("ログ全体をコピー").clicked() {
                    ui.ctx()
                        .copy_text(self.text_of(log, visible.iter().copied()));
                }
                if ui.button("ファイルに保存…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("ログを保存")
                        .set_file_name("SmartConverter.log")
                        .save_file()
                    {
                        let _ = std::fs::write(path, self.text_of(log, visible.iter().copied()));
                    }
                }
                if ui
                    .button("消去")
                    .on_hover_text("表示中のログを消します（実行中のジョブのログは残します）")
                    .clicked()
                {
                    // 消すのは描き終わってから（このフレームの表示行の一覧が古くなるため）
                    clear = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("閉じる").clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(4.0);
        });

        // 本体
        egui::CentralPanel::default().show(ui, |ui| {
            // 行の高さを文字の高さに合わせる（既定のボタンの高さだと行間が広すぎる）
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);
            ui.spacing_mut().interact_size.y = row_h;
            let dark = ui.ctx().theme() == Theme::Dark;
            let mut clicked: Option<(usize, egui::Modifiers)> = None;
            egui::ScrollArea::both()
                .auto_shrink(false)
                .stick_to_bottom(self.autoscroll)
                .show_rows(ui, row_h, visible.len(), |ui, range| {
                    for &i in &visible[range] {
                        let e = &log.entries[i];
                        let text = RichText::new(e.format()).monospace().color(kind_color(
                            e.kind,
                            dark,
                            ui.visuals().text_color(),
                        ));
                        let text = if e.kind == LogKind::JobStart {
                            text.strong()
                        } else {
                            text
                        };
                        let resp = ui.add(
                            egui::Button::selectable(self.selected.contains(&i), text)
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        if resp.clicked() {
                            clicked = Some((i, ui.input(|inp| inp.modifiers)));
                        }
                    }
                });
            // 上にスクロールしたら自動スクロールをやめる
            if ui.input(|i| i.smooth_scroll_delta.y > 0.0) && ui.ui_contains_pointer() {
                self.autoscroll = false;
            }
            if let Some((i, mods)) = clicked {
                self.select(i, mods, &visible);
            }
        });

        if ui.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::C,
            ))
        }) && !self.selected.is_empty()
        {
            ui.ctx()
                .copy_text(self.text_of(log, self.selected.iter().copied()));
        }
        if clear {
            let filter = self.filter;
            log.entries
                .retain(|e| Some(e.job) == running || filter.is_some_and(|f| e.job != f));
            self.selected.clear();
            self.anchor = None;
        }
        if close || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }
    }

    fn select(&mut self, i: usize, mods: egui::Modifiers, visible: &[usize]) {
        if mods.shift {
            if let Some(a) = self.anchor {
                let pos = |x: usize| visible.iter().position(|&v| v == x);
                if let (Some(pa), Some(pb)) = (pos(a), pos(i)) {
                    let (s, e) = if pa <= pb { (pa, pb) } else { (pb, pa) };
                    self.selected = visible[s..=e].iter().copied().collect();
                    return;
                }
            }
        }
        if mods.command {
            if !self.selected.remove(&i) {
                self.selected.insert(i);
            }
        } else {
            self.selected = [i].into();
        }
        self.anchor = Some(i);
    }

    fn text_of(&self, log: &Log, rows: impl Iterator<Item = usize>) -> String {
        let mut s = String::new();
        for i in rows {
            if let Some(e) = log.entries.get(i) {
                s.push_str(&e.format());
                s.push('\n');
            }
        }
        s
    }
}

fn kind_color(kind: LogKind, dark: bool, normal: Color32) -> Color32 {
    let pick = |l: [u8; 3], d: [u8; 3]| {
        let c = if dark { d } else { l };
        Color32::from_rgb(c[0], c[1], c[2])
    };
    match kind {
        LogKind::Command => pick([0x02, 0x84, 0xc7], [0x38, 0xbd, 0xf8]),
        LogKind::Success => pick([0x16, 0xa3, 0x4a], [0x4a, 0xde, 0x80]),
        LogKind::Failure => pick([0xdc, 0x26, 0x26], [0xf8, 0x71, 0x71]),
        LogKind::Warning => pick([0xd9, 0x77, 0x06], [0xfb, 0xbf, 0x24]),
        LogKind::Info => pick([0x6b, 0x72, 0x80], [0x9c, 0xa3, 0xaf]),
        LogKind::JobStart | LogKind::Output => normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(job: JobId, kind: LogKind, text: &str) -> LogEntry {
        LogEntry {
            job,
            time: SystemTime::now(),
            kind,
            text: text.into(),
        }
    }

    #[test]
    fn replace_last_only_same_job_output() {
        let mut log = Log::default();
        log.push(entry(1, LogKind::Output, "10%"));
        log.replace_last_output(entry(1, LogKind::Output, "20%"));
        assert_eq!(log.entries.len(), 1);
        assert_eq!(log.entries[0].text, "20%");
        log.push(entry(1, LogKind::Info, "x"));
        log.replace_last_output(entry(1, LogKind::Output, "30%"));
        assert_eq!(log.entries.len(), 3);
    }

    #[test]
    fn limit_drops_oldest() {
        let mut log = Log::default();
        for i in 0..LOG_LIMIT + 1 {
            log.push(entry(1, LogKind::Output, &i.to_string()));
        }
        assert!(log.entries.len() <= LOG_LIMIT);
        assert_eq!(log.entries.last().unwrap().text, LOG_LIMIT.to_string());
    }

    #[test]
    fn format_has_mark() {
        assert!(entry(1, LogKind::Failure, "NG").format().contains("✗ NG"));
    }
}

/// ログ画面の操作テスト（egui_kittest で、画面を表示せずにボタンを押して確かめる）。
#[cfg(test)]
mod ui_tests {
    use std::path::PathBuf;

    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};
    use egui_kittest::Harness;

    use super::*;

    struct State {
        view: LogView,
        log: Log,
        queue: Queue,
        running: Option<JobId>,
    }

    /// ジョブ 2 件（#1 と #2）のログを用意する。
    fn state() -> State {
        let mut queue = Queue::default();
        let a = queue.add(PathBuf::from("a.ts"), "P");
        let b = queue.add(PathBuf::from("b.ts"), "P");
        let mut log = Log::default();
        for (job, kind, text) in [
            (a, LogKind::JobStart, "#1 a.ts"),
            (a, LogKind::Output, "alpha output"),
            (a, LogKind::Failure, "alpha ERROR"),
            (b, LogKind::JobStart, "#2 b.ts"),
            (b, LogKind::Output, "beta output"),
        ] {
            log.push(LogEntry {
                job,
                time: SystemTime::now(),
                kind,
                text: text.into(),
            });
        }
        State {
            view: LogView {
                open: true,
                ..Default::default()
            },
            log,
            queue,
            running: None,
        }
    }

    fn harness(s: State) -> Harness<'static, State> {
        // 日本語フォントのない環境（CI の Linux）でも動くよう、無い文字は四角で描く
        let mut h = Harness::builder()
            .with_size(egui::vec2(800.0, 500.0))
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, s: &mut State| s.view.ui(ui, &mut s.log, &s.queue, s.running),
                s,
            );
        h.run();
        h
    }

    /// 直前のフレームでクリップボードにコピーされた文字列。
    fn copied(h: &Harness<'_, State>) -> Option<String> {
        h.output()
            .platform_output
            .commands
            .iter()
            .find_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
    }

    fn click(h: &mut Harness<'_, State>, label: &str) {
        h.get_by_label(label).click();
        h.step();
    }

    #[test]
    fn copy_all_shows_every_job() {
        let mut h = harness(state());
        click(&mut h, "ログ全体をコピー");
        let text = copied(&h).expect("コピーされていない");
        assert!(
            text.contains("alpha output") && text.contains("beta output"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 5);
    }

    #[test]
    fn copy_all_respects_job_filter() {
        let mut s = state();
        s.view.open_for(Some(2));
        let mut h = harness(s);
        click(&mut h, "ログ全体をコピー");
        let text = copied(&h).unwrap();
        assert!(text.contains("beta output"));
        assert!(!text.contains("alpha"), "{text}");
    }

    #[test]
    fn search_narrows_lines() {
        let mut h = harness(state());
        h.get_by_role(Role::TextInput).focus();
        h.run();
        h.get_by_role(Role::TextInput).type_text("error");
        h.run();
        // 大文字小文字を区別しない
        assert!(h.query_by_label_contains("alpha ERROR").is_some());
        assert!(h.query_by_label_contains("beta output").is_none());
        click(&mut h, "ログ全体をコピー");
        assert_eq!(copied(&h).unwrap().lines().count(), 1);
    }

    #[test]
    fn select_line_and_copy() {
        let mut h = harness(state());
        assert!(h
            .get_by_label("選択行をコピー")
            .accesskit_node()
            .is_disabled());
        h.get_by_label_contains("beta output").click();
        h.run();
        click(&mut h, "選択行をコピー");
        let text = copied(&h).unwrap();
        assert!(text.trim_end().ends_with("beta output"), "{text}");
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn clear_keeps_running_job() {
        let mut s = state();
        s.running = Some(2);
        let mut h = harness(s);
        click(&mut h, "消去");
        h.run();
        let log = &h.state().log;
        assert!(log.entries.iter().all(|e| e.job == 2), "{:?}", log.entries);
        assert_eq!(log.entries.len(), 2);
    }

    #[test]
    fn close_button_closes() {
        let mut h = harness(state());
        click(&mut h, "閉じる");
        assert!(!h.state().view.open);
    }
}
