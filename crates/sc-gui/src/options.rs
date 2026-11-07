//! オプション画面（画面設計 S-03）。
//!
//! 設定のコピー（下書き）を編集し、［OK］［適用］で呼び出し側に返す。
//! テーマと UI の倍率は、押す前からプレビューとしてすぐ反映し、［キャンセル］で元に戻す。

use std::path::{Path, PathBuf};

use eframe::egui;
use sc_core::naming::{self, OnExists, TimestampPos};
use sc_core::paths;
use sc_core::script::{self, ScriptKind, ScriptSupport};
use sc_core::settings::{ScriptTemplate, Settings, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Convert,
    FileName,
    Script,
    Tools,
    Display,
    Advanced,
}

impl Category {
    const ALL: [Category; 6] = [
        Category::Convert,
        Category::FileName,
        Category::Script,
        Category::Tools,
        Category::Display,
        Category::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            Category::Convert => "変換",
            Category::FileName => "ファイル名",
            Category::Script => "スクリプト",
            Category::Tools => "ツール",
            Category::Display => "表示",
            Category::Advanced => "詳細",
        }
    }
}

/// 画面の操作の結果。
pub enum OptionsAction {
    /// 設定を反映して保存する（`close` なら画面も閉じる）
    Apply {
        settings: Box<Settings>,
        close: bool,
    },
    /// 取り消して閉じる（テーマなどを元に戻す）
    Cancel,
    /// 本家の設定を取り込む
    ImportOriginal,
    /// 設定フォルダを開く
    OpenConfigDir,
    /// AviSynth+ / VapourSynth を調べ直す
    RedetectScripts,
}

/// 画面に出す検出結果（呼び出し側が調べて渡す）。
pub struct OptionsEnv<'a> {
    pub config_path: &'a Path,
    pub app_dir: &'a Path,
    pub tools: &'a [ToolStatus],
    pub scripts: &'a ScriptSupport,
}

/// プリセットで使われているツールの検出結果（呼び出し側が調べて渡す）。
pub struct ToolStatus {
    pub name: String,
    pub path: Option<PathBuf>,
}

pub struct OptionsView {
    draft: Settings,
    category: Category,
    new_tool: (String, String),
}

impl OptionsView {
    pub fn new(settings: &Settings) -> Self {
        Self {
            draft: settings.clone(),
            category: Category::Convert,
            new_tool: (String::new(), String::new()),
        }
    }

    /// 編集中の設定（テーマ・倍率のプレビュー用）。
    pub fn draft(&self) -> &Settings {
        &self.draft
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, env: &OptionsEnv<'_>) -> Option<OptionsAction> {
        let mut action = None;

        egui::Panel::bottom("opt_bottom").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let valid = self.problem().is_none();
                    if ui.add_enabled(valid, egui::Button::new("適用")).clicked() {
                        action = Some(OptionsAction::Apply {
                            settings: Box::new(self.draft.clone()),
                            close: false,
                        });
                    }
                    if ui.button("キャンセル").clicked() {
                        action = Some(OptionsAction::Cancel);
                    }
                    if ui.add_enabled(valid, egui::Button::new("OK")).clicked() {
                        action = Some(OptionsAction::Apply {
                            settings: Box::new(self.draft.clone()),
                            close: true,
                        });
                    }
                    if let Some(p) = self.problem() {
                        ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), p);
                    }
                });
            });
            ui.add_space(6.0);
        });

        egui::Panel::left("opt_categories")
            .resizable(false)
            .exact_size(120.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                for c in Category::ALL {
                    if ui.selectable_label(self.category == c, c.label()).clicked() {
                        self.category = c;
                    }
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.heading(self.category.label());
                    ui.add_space(6.0);
                    match self.category {
                        Category::Convert => self.convert_ui(ui),
                        Category::FileName => self.file_name_ui(ui),
                        Category::Script => {
                            if let Some(a) = self.script_ui(ui, env) {
                                action = Some(a);
                            }
                        }
                        Category::Tools => self.tools_ui(ui, env.tools),
                        Category::Display => self.display_ui(ui),
                        Category::Advanced => {
                            if let Some(a) = self.advanced_ui(ui, env.config_path) {
                                action = Some(a);
                            }
                        }
                    }
                });
        });

        if action.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(OptionsAction::Cancel);
        }
        action
    }

    /// 入力の誤り（保存できない理由）。
    fn problem(&self) -> Option<&'static str> {
        let t = self.draft.temporary_folder.trim();
        if !t.is_empty() && paths::is_root(Path::new(t)) {
            return Some("一時フォルダにドライブのルートは指定できません");
        }
        None
    }

    fn convert_ui(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.draft;
        ui.checkbox(
            &mut d.auto_start_on_add,
            "ファイルを追加したらすぐ変換を始める",
        );
        ui.checkbox(
            &mut d.all_abort,
            "「処理中を中断」で残りの処理もすべて止める",
        );
        ui.checkbox(
            &mut d.delete_partial,
            "中断したとき、途中までの出力ファイルを消す",
        )
        .on_hover_text("そのジョブで新しくできたファイルだけを消します。上書きしたファイルは元に戻せないので残します");
        ui.add_space(6.0);
        egui::Grid::new("convert")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let name_label = ui.label("子プロセスの優先度");
                let labels = [
                    "0 アイドル（推奨）",
                    "1 通常",
                    "2 高（非推奨）",
                    "3 リアルタイム（非推奨）",
                ];
                let combo = egui::ComboBox::from_id_salt("priority")
                    .selected_text(labels[d.priority.min(3) as usize])
                    .show_ui(ui, |ui| {
                        for (i, l) in labels.iter().enumerate() {
                            ui.selectable_value(&mut d.priority, i as u8, *l);
                        }
                    });
                combo.response.labelled_by(name_label.id);
                ui.end_row();

                let name_label = ui.label("同名のファイルがあるとき");
                let name = |o: OnExists| match o {
                    OnExists::Rename => "連番を付ける",
                    OnExists::Overwrite => "上書きする",
                    OnExists::Skip => "スキップする",
                };
                let combo = egui::ComboBox::from_id_salt("on_exists")
                    .selected_text(name(d.on_exists))
                    .show_ui(ui, |ui| {
                        for o in [OnExists::Rename, OnExists::Overwrite, OnExists::Skip] {
                            ui.selectable_value(&mut d.on_exists, o, name(o));
                        }
                    });
                combo.response.labelled_by(name_label.id);
                ui.end_row();

                let name_label = ui.label("一時フォルダ");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut d.temporary_folder)
                            .hint_text("（OS の既定）")
                            .desired_width(300.0),
                    )
                    .labelled_by(name_label.id);
                    if ui.button("選択").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            d.temporary_folder = p.display().to_string();
                        }
                    }
                });
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.checkbox(
            &mut d.exit_if_finished,
            "すべての処理が終わったら SmartConverter を終了",
        );
    }

    fn file_name_ui(&mut self, ui: &mut egui::Ui) {
        let ts = &mut self.draft.timestamp;
        ui.checkbox(&mut ts.enabled, "出力ファイル名にタイムスタンプを付ける");
        ui.add_enabled_ui(ts.enabled, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("位置");
                ui.radio_value(&mut ts.pos, TimestampPos::HeadWithSpace, "先頭（1 字空け）");
                ui.radio_value(&mut ts.pos, TimestampPos::Head, "先頭");
            });
            if ts.pos == TimestampPos::TailIgnored {
                ui.weak("（本家の設定から取り込んだ「末尾」は、本家と同じく何も付けません）");
            }
            ui.horizontal(|ui| {
                let name_label = ui.label("書式");
                let combo = egui::ComboBox::from_id_salt("ts_format")
                    .selected_text(&ts.format)
                    .show_ui(ui, |ui| {
                        for f in naming::FORMAT_CANDIDATES {
                            ui.selectable_value(&mut ts.format, f.to_owned(), f);
                        }
                    });
                combo.response.labelled_by(name_label.id);
                ui.add(egui::TextEdit::singleline(&mut ts.format).desired_width(200.0))
                    .labelled_by(name_label.id);
            });
            ui.weak(
                "yyyy・yy＝年　mm＝月（hh の直後は分）　dd＝日　ddd＝曜日　hh＝時　nn＝分　ss＝秒",
            );
            ui.add_space(6.0);
            let now = chrono::Local::now().naive_local();
            let sample = naming::build_title("サンプル動画", ts, &now);
            ui.horizontal(|ui| {
                ui.label("サンプル");
                ui.monospace(format!("{sample}.mp4"));
            });
        });
    }

    fn script_ui(&mut self, ui: &mut egui::Ui, env: &OptionsEnv<'_>) -> Option<OptionsAction> {
        let mut action = None;
        let d = &mut self.draft;
        let mut enabled = !d.script_disabled;
        ui.checkbox(
            &mut enabled,
            "スケルトン機能を使う（拡張子ごとのテンプレートから .avs / .vpy を作る）",
        );
        d.script_disabled = !enabled;
        ui.weak(
            "入力の拡張子にテンプレートが割り当てられていれば、テンプレートの置換変数を展開したスクリプトを作り、\
             それを <%InputFile%> としてプリセットを実行します。.avs / .vpy を直接追加したときはそのまま使います。",
        );
        ui.add_space(6.0);

        // 検出結果
        egui::Grid::new("script_support")
            .num_columns(2)
            .show(ui, |ui| {
                for kind in [ScriptKind::AviSynth, ScriptKind::VapourSynth] {
                    ui.label(kind.name());
                    match env.scripts.get(kind) {
                        // ✓ とパスを 1 つの文字列にすると、間の空白で先に改行されて ✓ だけが 1 行目に残る。
                        Some(p) => {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                                ui.label("✓");
                                path_label(ui, egui::RichText::new(p.display().to_string()));
                            })
                            .response
                        }
                        None => ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "✗ 見つかりません（この種類のテンプレートは使いません）",
                        ),
                    };
                    ui.end_row();
                }
            });
        if ui.button("再検出").clicked() {
            action = Some(OptionsAction::RedetectScripts);
        }
        ui.add_space(8.0);

        let mut remove = None;
        egui::Grid::new("scripts")
            .num_columns(5)
            .striped(true)
            .show(ui, |ui| {
                let ext_label = ui.strong("拡張子");
                let template_label = ui.strong("テンプレート");
                ui.strong("種類");
                ui.strong("");
                ui.strong("");
                ui.end_row();
                for (i, t) in d.script_templates.iter_mut().enumerate() {
                    sized_input(ui, &mut t.ext, 60.0).labelled_by(ext_label.id);
                    sized_input(ui, &mut t.template, 300.0).labelled_by(template_label.id);
                    template_status(ui, &t.template, env);
                    if ui.button("参照").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("スクリプト", &["avs", "vpy"])
                            .pick_file()
                        {
                            t.template = p.display().to_string();
                        }
                    }
                    if ui.button("削除").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = remove {
            d.script_templates.remove(i);
        }
        if ui.button("追加").clicked() {
            d.script_templates.push(ScriptTemplate {
                ext: "TS".into(),
                template: String::new(),
            });
        }
        ui.add_space(6.0);
        ui.weak(
            "テンプレートは相対パスならアプリのフォルダから探します。.vpy では Windows のパスの \\ が\
             エスケープとして読まれないよう、r\"<%InputFile%>\" のように r を付けてください。\
             DirectShowSource など Windows 専用の関数は Linux では使えません。",
        );
        action
    }

    fn tools_ui(&mut self, ui: &mut egui::Ui, tools: &[ToolStatus]) {
        let d = &mut self.draft;
        ui.label("ツール名とパス（PATH より優先して使う）");
        let mut remove = None;
        egui::Grid::new("tools")
            .num_columns(4)
            .striped(true)
            .show(ui, |ui| {
                for (name, path) in d.tools.iter_mut() {
                    let name_label = ui.monospace(name);
                    sized_input(ui, path, 300.0).labelled_by(name_label.id);
                    if ui.button("参照").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            *path = p.display().to_string();
                        }
                    }
                    if ui.button("削除").clicked() {
                        remove = Some(name.clone());
                    }
                    ui.end_row();
                }
            });
        if let Some(n) = remove {
            d.tools.remove(&n);
        }
        ui.horizontal(|ui| {
            let name_label = ui.label("名前");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_tool.0)
                    .hint_text("例 qaac64")
                    .desired_width(120.0),
            )
            .labelled_by(name_label.id);
            let path_label = ui.label("パス");
            ui.add(egui::TextEdit::singleline(&mut self.new_tool.1).desired_width(220.0))
                .labelled_by(path_label.id);
            if ui.button("参照").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_file() {
                    if self.new_tool.0.is_empty() {
                        self.new_tool.0 = sc_core::cmdline::tool_name(&p.to_string_lossy());
                    }
                    self.new_tool.1 = p.display().to_string();
                }
            }
            let ok = !self.new_tool.0.trim().is_empty() && !self.new_tool.1.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new("追加")).clicked() {
                d.tools.insert(
                    self.new_tool.0.trim().to_owned(),
                    self.new_tool.1.trim().to_owned(),
                );
                self.new_tool = (String::new(), String::new());
            }
        });

        ui.add_space(10.0);
        ui.strong("プリセットで使われているツール");
        ui.weak("（［適用］した設定で調べます）");
        egui::Grid::new("tool_status")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for t in tools {
                    match &t.path {
                        Some(p) => {
                            ui.colored_label(
                                egui::Color32::from_rgb(0x16, 0xa3, 0x4a),
                                format!("✓ {}", t.name),
                            );
                            path_label(ui, egui::RichText::new(p.display().to_string()).weak());
                        }
                        None => {
                            ui.colored_label(
                                egui::Color32::from_rgb(0xdc, 0x26, 0x26),
                                format!("✗ {}", t.name),
                            );
                            ui.weak("見つかりません");
                        }
                    }
                    ui.end_row();
                }
            });
    }

    fn display_ui(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.draft;
        ui.horizontal(|ui| {
            ui.label("テーマ");
            ui.radio_value(&mut d.theme, Theme::System, "OS に合わせる");
            ui.radio_value(&mut d.theme, Theme::Light, "ライト");
            ui.radio_value(&mut d.theme, Theme::Dark, "ダーク");
        });
        ui.horizontal(|ui| {
            let name_label = ui.label("UI の倍率");
            ui.add(egui::Slider::new(&mut d.ui_scale, 80..=200).suffix("%"))
                .labelled_by(name_label.id);
        });
        ui.add_space(6.0);
        ui.checkbox(&mut d.start_minimized, "起動時に最小化する");
        let tray = crate::tray::SUPPORTED;
        ui.add_enabled(
            tray,
            egui::Checkbox::new(&mut d.minimize_to_tray, "最小化するとトレイに格納する"),
        )
        .on_disabled_hover_text("この環境ではトレイを使えません");
        ui.checkbox(&mut d.notify, "変換の完了・失敗を通知する（最小化中）");
    }

    fn advanced_ui(&mut self, ui: &mut egui::Ui, config_path: &Path) -> Option<OptionsAction> {
        let mut action = None;
        let d = &mut self.draft;
        egui::Grid::new("advanced")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let name_label = ui.label("プリセットファイル");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.preset_file).desired_width(300.0))
                        .labelled_by(name_label.id);
                    if ui.button("選択").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("INI", &["ini"])
                            .pick_file()
                        {
                            d.preset_file = p.display().to_string();
                        }
                    }
                });
                ui.end_row();

                ui.label("設定ファイル");
                ui.horizontal(|ui| {
                    ui.weak(config_path.display().to_string());
                    if ui.button("フォルダを開く").clicked() {
                        action = Some(OptionsAction::OpenConfigDir);
                    }
                });
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.checkbox(
            &mut d.log_to_file,
            "ログをファイルにも保存する（設定フォルダの logs）",
        );
        ui.add_space(10.0);
        if ui.button("本家の設定を取り込む…").clicked() {
            action = Some(OptionsAction::ImportOriginal);
        }
        action
    }
}

/// テンプレート 1 行分の種類と状態（ファイルがあるか、その種類が使えるか）。
/// 表（Grid）の中に、幅 `width` の 1 行の入力欄を置く。
/// `desired_width` だけだと、Grid が前の描画の列の幅（最初は最小の 40）を使い、その幅に縮まったまま広がらない。
fn sized_input(ui: &mut egui::Ui, text: &mut String, width: f32) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    ui.add_sized([width, height], egui::TextEdit::singleline(text))
}

/// パスを表示する。長ければ画面の幅で折り返して、最後まで出す（選んでコピーもできる）。
/// 表（Grid）の中の文字は既定では折り返さないので、そのままだと長いパスが画面の中身の幅を広げ、
/// あとに並べる説明文が画面の外にはみ出す（AT-UI-03、150% で見つかった）。
/// 省略（…）にしなかったのは、パスを読んだりコピーしたりして使うことがあるため。
fn path_label(ui: &mut egui::Ui, text: egui::RichText) -> egui::Response {
    ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Wrap))
}

fn template_status(ui: &mut egui::Ui, template: &str, env: &OptionsEnv<'_>) {
    let path = script::template_path(template.trim(), env.app_dir);
    let kind = ScriptKind::from_path(&path);
    let label = kind.map_or("？", |k| k.ext());
    let problem = if template.trim().is_empty() {
        Some("テンプレートを指定してください".to_owned())
    } else if kind.is_none() {
        Some("拡張子が .avs / .vpy ではありません".to_owned())
    } else if !path.is_file() {
        Some(format!("ファイルがありません：{}", path.display()))
    } else if kind.is_some_and(|k| env.scripts.get(k).is_none()) {
        Some(format!(
            "{} が見つからないため使いません",
            kind.map_or("", |k| k.name())
        ))
    } else {
        None
    };
    match problem {
        Some(p) => {
            ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {label}"))
                .on_hover_text(p);
        }
        None => {
            ui.label(label).on_hover_text(path.display().to_string());
        }
    }
}

/// オプション画面の操作テスト（egui_kittest で、画面を表示せずにボタンを押して確かめる）。
#[cfg(test)]
mod ui_tests {
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};
    use egui_kittest::Harness;

    use super::*;

    /// 画面が返した操作（比較しやすい形）。
    enum Got {
        Apply(Box<Settings>, bool),
        Cancel,
        Import,
        OpenDir,
        Redetect,
    }

    struct State {
        view: OptionsView,
        got: Option<Got>,
        tools: Vec<ToolStatus>,
        scripts: ScriptSupport,
    }

    fn harness(settings: Settings) -> Harness<'static, State> {
        let state = State {
            view: OptionsView::new(&settings),
            got: None,
            tools: vec![
                ToolStatus {
                    name: "ffmpeg".into(),
                    path: Some(PathBuf::from("C:/bin/ffmpeg.exe")),
                },
                ToolStatus {
                    name: "qaac64".into(),
                    path: None,
                },
            ],
            scripts: ScriptSupport {
                avisynth: Some(PathBuf::from("C:/Windows/System32/AviSynth.dll")),
                vapoursynth: None,
            },
        };
        let mut h = Harness::builder()
            .with_size(egui::vec2(720.0, 480.0))
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, s: &mut State| {
                    let env = OptionsEnv {
                        config_path: Path::new("C:/cfg/SmartConverter.ini"),
                        app_dir: Path::new("C:/app"),
                        tools: &s.tools,
                        scripts: &s.scripts,
                    };
                    let a = s.view.ui(ui, &env);
                    if let Some(a) = a {
                        s.got = Some(match a {
                            OptionsAction::Apply { settings, close } => Got::Apply(settings, close),
                            OptionsAction::Cancel => Got::Cancel,
                            OptionsAction::ImportOriginal => Got::Import,
                            OptionsAction::OpenConfigDir => Got::OpenDir,
                            OptionsAction::RedetectScripts => Got::Redetect,
                        });
                    }
                },
                state,
            );
        h.run();
        h
    }

    fn click(h: &mut Harness<'_, State>, label: &str) {
        h.get_by_label(label).click();
        h.run();
    }

    fn applied<'h>(h: &'h Harness<'_, State>) -> (&'h Settings, bool) {
        match &h.state().got {
            Some(Got::Apply(s, close)) => (s, *close),
            _ => panic!("OK / 適用 が返っていない"),
        }
    }

    #[test]
    fn long_paths_wrap_inside_the_window() {
        // 日本語のフォルダ名のパスと、空白も日本語もない英数字だけのパス。
        let ja = format!(
            "C:/{}/vapoursynth/vapoursynth.dll",
            "深いフォルダ/".repeat(30)
        );
        let ascii = format!("C:/{}/bin/ffmpeg.exe", "very_long_folder_name".repeat(12));
        let mut h = harness(Settings::default());
        h.state_mut().scripts.vapoursynth = Some(PathBuf::from(&ja));
        h.state_mut().tools[0].path = Some(PathBuf::from(&ascii));
        let width = 720.0;
        let fits = |h: &Harness<'_, State>, text: &str| {
            let r = h.get_by_label_contains(text).rect();
            assert!(r.max.x <= width, "{text} が画面の外にはみ出した: {r:?}");
            r
        };

        click(&mut h, "スクリプト");
        fits(&h, "入力の拡張子にテンプレート");
        fits(&h, "テンプレートは相対パスなら");
        // 省略せず、折り返して最後まで出す（2 行以上になる）。
        let r = fits(&h, "vapoursynth/vapoursynth.dll");
        assert!(r.height() > 30.0, "パスが折り返されていない: {r:?}");

        click(&mut h, "ツール");
        let r = fits(&h, "bin/ffmpeg.exe");
        assert!(r.height() > 30.0, "パスが折り返されていない: {r:?}");
    }

    #[test]
    fn delete_partial_checkbox() {
        let label = "中断したとき、途中までの出力ファイルを消す";
        // 既定は ON。外して OK すると OFF で返る
        let mut h = harness(Settings::default());
        click(&mut h, label);
        click(&mut h, "OK");
        assert!(!applied(&h).0.delete_partial);
        // OFF の設定から開くと OFF で、入れると ON で返る
        let mut s = Settings::default();
        s.delete_partial = false;
        let mut h = harness(s);
        click(&mut h, label);
        click(&mut h, "OK");
        assert!(applied(&h).0.delete_partial);
    }

    #[test]
    fn inputs_in_tables_have_their_width() {
        // 表（Grid）の中の入力欄が、列の最小の幅（40）に縮まっていないこと（150% の確認で見つかった）。
        let mut s = Settings::default();
        s.tools
            .insert("ffmpeg".into(), "D:/tools/ffmpeg.exe".into());
        let mut h = harness(s);
        let width = |h: &Harness<'_, State>, label: &str| h.get_by_label(label).rect().width();

        click(&mut h, "ツール");
        assert!(
            width(&h, "ffmpeg") >= 290.0,
            "ツールのパス: {}",
            width(&h, "ffmpeg")
        );

        click(&mut h, "スクリプト");
        click(&mut h, "追加");
        h.run();
        assert!(
            width(&h, "拡張子") >= 55.0,
            "拡張子: {}",
            width(&h, "拡張子")
        );
        assert!(
            width(&h, "テンプレート") >= 290.0,
            "テンプレート: {}",
            width(&h, "テンプレート")
        );

        click(&mut h, "詳細");
        assert!(
            width(&h, "プリセットファイル") >= 290.0,
            "プリセットファイル: {}",
            width(&h, "プリセットファイル")
        );
    }

    #[test]
    fn change_theme_and_ok() {
        let mut h = harness(Settings::default());
        click(&mut h, "表示");
        click(&mut h, "ライト");
        // プレビュー用の下書きにすぐ反映される
        assert_eq!(h.state().view.draft().theme, Theme::Light);
        click(&mut h, "OK");
        let (s, close) = applied(&h);
        assert_eq!(s.theme, Theme::Light);
        assert!(close);
    }

    #[test]
    fn apply_keeps_window_open() {
        // 既定のカテゴリは「変換」
        let mut h = harness(Settings::default());
        click(&mut h, "すべての処理が終わったら SmartConverter を終了");
        click(&mut h, "適用");
        let (s, close) = applied(&h);
        assert!(s.exit_if_finished);
        assert!(!close);
    }

    #[test]
    fn cancel_returns_cancel() {
        let mut h = harness(Settings::default());
        click(&mut h, "キャンセル");
        assert!(matches!(h.state().got, Some(Got::Cancel)));
    }

    #[test]
    fn root_temp_folder_disables_ok() {
        let mut s = Settings::default();
        s.temporary_folder = if cfg!(windows) {
            "C:\\".into()
        } else {
            "/".into()
        };
        let h = harness(s);
        assert!(h.get_by_label("OK").accesskit_node().is_disabled());
        assert!(h.get_by_label("適用").accesskit_node().is_disabled());
        assert!(h
            .query_by_label("一時フォルダにドライブのルートは指定できません")
            .is_some());
    }

    #[test]
    fn timestamp_sample_follows_position() {
        let mut h = harness(Settings::default());
        click(&mut h, "ファイル名");
        click(&mut h, "出力ファイル名にタイムスタンプを付ける");
        // 既定は「先頭（1 字空け）」
        assert!(h.query_by_label_contains("] サンプル動画.mp4").is_some());
        click(&mut h, "先頭");
        assert!(h.query_by_label_contains("]サンプル動画.mp4").is_some());
        assert!(h.query_by_label_contains("] サンプル動画.mp4").is_none());
    }

    #[test]
    fn add_tool_and_show_status() {
        let mut h = harness(Settings::default());
        click(&mut h, "ツール");
        // 検出状況（呼び出し側から渡したもの）が出る
        assert!(h.query_by_label("✓ ffmpeg").is_some());
        assert!(h.query_by_label("✗ qaac64").is_some());
        // 名前とパスを入れて追加
        for (i, text) in ["qaac64", "C:/tools/qaac64.exe"].iter().enumerate() {
            h.get_all_by_role(Role::TextInput).nth(i).unwrap().focus();
            h.run();
            h.get_all_by_role(Role::TextInput)
                .nth(i)
                .unwrap()
                .type_text(text);
            h.run();
        }
        click(&mut h, "追加");
        click(&mut h, "OK");
        let (s, _) = applied(&h);
        assert_eq!(
            s.tools.get("qaac64").map(String::as_str),
            Some("C:/tools/qaac64.exe")
        );
    }

    #[test]
    fn script_template_rows() {
        let mut h = harness(Settings::default());
        click(&mut h, "スクリプト");
        click(&mut h, "追加");
        click(&mut h, "追加");
        assert_eq!(h.state().view.draft().script_templates.len(), 2);
        h.get_all_by_label("削除").next().unwrap().click();
        h.run();
        assert_eq!(h.state().view.draft().script_templates.len(), 1);
        // スケルトン機能の切り替え
        click(
            &mut h,
            "スケルトン機能を使う（拡張子ごとのテンプレートから .avs / .vpy を作る）",
        );
        assert!(h.state().view.draft().script_disabled);
    }

    #[test]
    fn script_detection_is_shown() {
        let mut h = harness(Settings::default());
        click(&mut h, "スクリプト");
        // ✓ とパスは別のラベル（パスだけを折り返すため）
        assert!(h.query_by_label("✓").is_some());
        assert!(h
            .query_by_label("C:/Windows/System32/AviSynth.dll")
            .is_some());
        assert!(h.query_by_label_contains("✗ 見つかりません").is_some());
        click(&mut h, "追加");
        // テンプレートが空の行は種類が分からない
        assert!(h.query_by_label("⚠ ？").is_some());
        click(&mut h, "再検出");
        assert!(matches!(h.state().got, Some(Got::Redetect)));
    }

    #[test]
    fn advanced_buttons() {
        let mut h = harness(Settings::default());
        click(&mut h, "詳細");
        click(&mut h, "本家の設定を取り込む…");
        assert!(matches!(h.state().got, Some(Got::Import)));
        click(&mut h, "フォルダを開く");
        assert!(matches!(h.state().got, Some(Got::OpenDir)));
    }
}
