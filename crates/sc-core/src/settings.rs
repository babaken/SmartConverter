//! アプリ設定（SmartConverter.ini）の読み書きと、本家設定（3GP_Converter.ini）の取り込み
//! （要件 §4.8、基本設計 §8.1）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::ini::{encode_value, IniDocument};
use crate::naming::{OnExists, TimestampOptions, TimestampPos};
use crate::paths;
use crate::text;

/// 設定ファイル名。
pub const FILE_NAME: &str = "SmartConverter.ini";

/// テーマ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// スクリプトのスケルトン割り当て（`ExtN=拡張子:テンプレート`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptTemplate {
    /// 拡張子（`.` なし、大文字小文字を区別しない）
    pub ext: String,
    pub template: String,
}

/// アプリ設定。
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    // [General]
    pub preset_file: String,
    pub default_preset: String,
    pub auto_start_on_add: bool,
    pub all_abort: bool,
    pub exit_if_finished: bool,
    pub language: String,
    // [Output]
    pub output_dir: String,
    pub temporary_folder: String,
    pub on_exists: OnExists,
    /// 中断したとき、そのジョブで新しくできた出力（途中までのファイル）を消す
    pub delete_partial: bool,
    pub timestamp: TimestampOptions,
    // [Process]
    pub priority: u8,
    // [Script]
    pub script_disabled: bool,
    pub script_templates: Vec<ScriptTemplate>,
    // [Tools]
    pub tools: BTreeMap<String, String>,
    // [Ui]
    pub theme: Theme,
    pub ui_scale: u16,
    pub start_minimized: bool,
    pub minimize_to_tray: bool,
    pub notify: bool,
    pub log_to_file: bool,
    pub main_window: String,
    pub log_window: String,
    pub preset_window: String,

    /// 読み込んだファイルの行（保存時にコメントなどを残すため）
    doc: IniDocument,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preset_file: "Transcoding.ini".into(),
            default_preset: String::new(),
            auto_start_on_add: true,
            all_abort: true,
            exit_if_finished: false,
            language: "ja".into(),
            output_dir: String::new(),
            temporary_folder: String::new(),
            on_exists: OnExists::Rename,
            delete_partial: true,
            timestamp: TimestampOptions {
                enabled: false,
                pos: TimestampPos::HeadWithSpace,
                format: "[mm-dd]".into(),
            },
            priority: 0,
            script_disabled: false,
            script_templates: Vec::new(),
            tools: BTreeMap::new(),
            theme: Theme::System,
            ui_scale: 100,
            start_minimized: false,
            minimize_to_tray: true,
            notify: true,
            log_to_file: false,
            main_window: String::new(),
            log_window: String::new(),
            preset_window: String::new(),
            doc: IniDocument::default(),
        }
    }
}

impl Settings {
    /// ファイルから読み込む。ファイルがなければ既定値を返す。
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Self::from_text(&text::decode(&bytes).0)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// 文字列から読み込む。値が無い・不正な項目は既定値になる。
    pub fn from_text(text: &str) -> Self {
        let doc = IniDocument::parse(text);
        let d = Self::default();
        let s =
            |sec: &str, key: &str, def: &str| doc.get(sec, key).unwrap_or_else(|| def.to_owned());
        let b = |sec: &str, key: &str, def: bool| {
            doc.get(sec, key).map_or(def, |v| parse_bool(&v, def))
        };
        let n = |sec: &str, key: &str, def: u32| {
            doc.get(sec, key)
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(def)
        };

        let mut tools = BTreeMap::new();
        for key in doc.keys("Tools") {
            if let Some(v) = doc.get("Tools", key) {
                tools.insert(key.to_owned(), v);
            }
        }

        Self {
            preset_file: s("General", "PresetFile", &d.preset_file),
            default_preset: s("General", "DefaultPreset", &d.default_preset),
            auto_start_on_add: b("General", "AutoStartOnAdd", d.auto_start_on_add),
            all_abort: b("General", "AllAbort", d.all_abort),
            exit_if_finished: b("General", "ExitIfFinished", d.exit_if_finished),
            language: s("General", "Language", &d.language),
            output_dir: s("Output", "OutputDir", &d.output_dir),
            temporary_folder: s("Output", "TemporaryFolder", &d.temporary_folder),
            on_exists: doc
                .get("Output", "OnExists")
                .and_then(|v| OnExists::parse(&v))
                .unwrap_or(d.on_exists),
            delete_partial: b("Output", "DeletePartial", d.delete_partial),
            timestamp: TimestampOptions {
                enabled: b("Output", "AddTime", d.timestamp.enabled),
                pos: TimestampPos::from_ini(n("Output", "AddTimePos", 0).min(255) as u8),
                format: s("Output", "AddTimeFormat", &d.timestamp.format),
            },
            priority: n("Process", "Priority", d.priority as u32).min(3) as u8,
            script_disabled: b("Script", "Disable", d.script_disabled),
            script_templates: read_ext_list(&doc, "Script"),
            tools,
            theme: doc
                .get("Ui", "Theme")
                .and_then(|v| Theme::parse(&v))
                .unwrap_or(d.theme),
            ui_scale: n("Ui", "Scale", d.ui_scale as u32).clamp(80, 200) as u16,
            start_minimized: b("Ui", "StartMinimized", d.start_minimized),
            minimize_to_tray: b("Ui", "MinimizeToTray", d.minimize_to_tray),
            notify: b("Ui", "Notify", d.notify),
            log_to_file: b("Ui", "LogToFile", d.log_to_file),
            main_window: s("Ui", "MainWindow", ""),
            log_window: s("Ui", "LogWindow", ""),
            preset_window: s("Ui", "PresetWindow", ""),
            doc,
        }
    }

    /// 文字列にする（読み込んだファイルのコメントや知らないキーは残す）。
    pub fn to_text(&self) -> String {
        let mut doc = self.doc.clone();
        let mut set = |sec: &str, key: &str, value: &str| doc.set(sec, key, &encode_value(value));
        let bool_s = |v: bool| if v { "1" } else { "0" };

        set("General", "PresetFile", &self.preset_file);
        set("General", "DefaultPreset", &self.default_preset);
        set("General", "AutoStartOnAdd", bool_s(self.auto_start_on_add));
        set("General", "AllAbort", bool_s(self.all_abort));
        set("General", "ExitIfFinished", bool_s(self.exit_if_finished));
        set("General", "Language", &self.language);
        set("Output", "OutputDir", &self.output_dir);
        set("Output", "TemporaryFolder", &self.temporary_folder);
        set("Output", "OnExists", self.on_exists.as_str());
        set("Output", "DeletePartial", bool_s(self.delete_partial));
        set("Output", "AddTime", bool_s(self.timestamp.enabled));
        set(
            "Output",
            "AddTimePos",
            &self.timestamp.pos.to_ini().to_string(),
        );
        set("Output", "AddTimeFormat", &self.timestamp.format);
        set("Process", "Priority", &self.priority.to_string());
        set("Script", "Disable", bool_s(self.script_disabled));
        for (i, t) in self.script_templates.iter().enumerate() {
            set(
                "Script",
                &format!("Ext{i}"),
                &format!("{}:{}", t.ext, t.template),
            );
        }
        for (k, v) in &self.tools {
            set("Tools", k, v);
        }
        set("Ui", "Theme", self.theme.as_str());
        set("Ui", "Scale", &self.ui_scale.to_string());
        set("Ui", "StartMinimized", bool_s(self.start_minimized));
        set("Ui", "MinimizeToTray", bool_s(self.minimize_to_tray));
        set("Ui", "Notify", bool_s(self.notify));
        set("Ui", "LogToFile", bool_s(self.log_to_file));
        set("Ui", "MainWindow", &self.main_window);
        set("Ui", "LogWindow", &self.log_window);
        set("Ui", "PresetWindow", &self.preset_window);
        doc.to_text()
    }

    /// 一時ファイルを置くフォルダ（`TemporaryFolder`。空なら OS の一時フォルダ）。
    pub fn temporary_dir(&self) -> PathBuf {
        if self.temporary_folder.trim().is_empty() {
            std::env::temp_dir()
        } else {
            PathBuf::from(self.temporary_folder.trim())
        }
    }

    /// 前に強制終了されたときに残った一時フォルダを片付ける（起動時に呼ぶ）。消した数を返す。
    /// 設定の一時フォルダと OS の一時フォルダの両方を見る（設定を変える前のものも片付けるため）。
    ///
    /// OS の一時フォルダはこの PC 専用なので、プロセスが動いていなければすぐ消す。設定で指定した一時フォルダは、
    /// ネットワーク上の共有フォルダかもしれない（ほかの PC で実行中のジョブの PID は判定できない）ので、
    /// 更新から [`SHARED_TEMP_MIN_AGE`] 以上たったものだけを消す。
    pub fn remove_stale_temporary_dirs(&self) -> usize {
        let configured = self.temporary_dir();
        let system = std::env::temp_dir();
        let mut n = crate::naming::remove_stale_temporary_dirs(&system, None);
        if paths::absolute(&configured) != paths::absolute(&system) {
            n += crate::naming::remove_stale_temporary_dirs(&configured, Some(SHARED_TEMP_MIN_AGE));
        }
        n
    }

    /// UTF-8 で保存する。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| Error::Write {
                path: path.to_owned(),
                source,
            })?;
        }
        crate::paths::write_atomic(path, self.to_text()).map_err(|source| Error::Write {
            path: path.to_owned(),
            source,
        })
    }

    /// 本家の `3GP_Converter.ini` から設定を取り込む（要件 F-ST-02）。
    ///
    /// `[Avisynth]` のテンプレートのパスが `\cores\…` のように `\` で始まるときは、
    /// 本家のフォルダ（ini と同じ場所）からの相対パスとして絶対パスにする。
    pub fn import_original(&mut self, path: &Path) -> Result<()> {
        let bytes = std::fs::read(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let doc = IniDocument::parse(&text::decode(&bytes).0);
        let base = path.parent().unwrap_or(Path::new(""));
        let get = |key: &str| doc.get("INI", key);
        let get_bool = |key: &str, def: bool| get(key).map_or(def, |v| parse_bool(&v, def));

        if let Some(v) = get("OutputDir") {
            self.output_dir = v;
        }
        // 本家の綴り（DefaultSetitng）と、正しい綴りの両方を読む。
        if let Some(v) = get("DefaultSetitng").or_else(|| get("DefaultSetting")) {
            self.default_preset = v;
        }
        if let Some(v) = get("ExePriority").and_then(|v| v.trim().parse::<u8>().ok()) {
            self.priority = v.min(3);
        }
        self.exit_if_finished = get_bool("ExitIfFinished", self.exit_if_finished);
        self.script_disabled = get_bool("DisableAVS", self.script_disabled);
        self.start_minimized = get_bool("Minimized", self.start_minimized);
        self.notify = get_bool("EncodeBalloon", self.notify);
        if let Some(v) = get("TemporaryFolder") {
            self.temporary_folder = v;
        }
        self.all_abort = get_bool("AllAbort", self.all_abort);
        self.timestamp.enabled = get_bool("AddTime", self.timestamp.enabled);
        if let Some(v) = get("AddTimePos").and_then(|v| v.trim().parse::<u8>().ok()) {
            self.timestamp.pos = TimestampPos::from_ini(v);
        }
        if let Some(v) = get("AddTimeFormat") {
            self.timestamp.format = v;
        }

        // 本家と同じフォルダの Transcoding.ini を、プリセットファイルとして使う。
        let transcoding = base.join("Transcoding.ini");
        if transcoding.is_file() {
            self.preset_file = transcoding.to_string_lossy().into_owned();
        }

        let templates = read_ext_list(&doc, "Avisynth");
        if !templates.is_empty() {
            self.script_templates = templates
                .into_iter()
                .map(|t| ScriptTemplate {
                    template: resolve_original_path(base, &t.template),
                    ext: t.ext,
                })
                .collect();
        }
        Ok(())
    }
}

/// 設定で指定した一時フォルダで、残った一時フォルダを片付けるまでの時間（共有フォルダ対策）。
pub const SHARED_TEMP_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// プリセットファイル（`PresetFile`）の場所を決める。
///
/// 絶対パスはそのまま。相対パスは「設定ファイルのフォルダ」→「実行ファイルのフォルダ」の順に探し、
/// 先にあったほうを使う。どちらにもなければ設定ファイルのフォルダのパスを返す（エラーの表示や、新しく作るとき用）。
///
/// 設定ファイルは OS 標準の設定フォルダに置かれることがある（`default_path`。Linux は既定で ~/.config）。
/// そのときも、実行ファイルの隣に同梱した `Transcoding.ini` を使えるようにするため。
pub fn resolve_preset_path(config_path: &Path, app_dir: &Path, preset_file: &str) -> PathBuf {
    let p = PathBuf::from(preset_file);
    if p.is_absolute() {
        return p;
    }
    let in_config = config_path.parent().unwrap_or(Path::new(".")).join(&p);
    if in_config.is_file() {
        return in_config;
    }
    let in_app = app_dir.join(&p);
    if in_app.is_file() {
        return in_app;
    }
    in_config
}

/// 設定ファイルの場所を決める（要件 F-ST-03）。
///
/// - 実行ファイルのフォルダに設定ファイルがあれば、そこ（ポータブル）。
/// - Windows：実行ファイルのフォルダに書き込めればそこ、書き込めなければ `%APPDATA%\SmartConverter\`。
/// - Linux など：`$XDG_CONFIG_HOME/smartconverter/`（既定 `~/.config/smartconverter/`）。
///   実行ファイルのフォルダ（`~/.local/bin` など）には、書き込めても作らない。
///   ポータブルに使いたいときは、実行ファイルの隣に空の `SmartConverter.ini` を置く。
pub fn default_path(app_dir: &Path) -> PathBuf {
    let portable = app_dir.join(FILE_NAME);
    if portable.is_file() || (cfg!(windows) && is_writable_dir(app_dir)) {
        return portable;
    }
    let sub = if cfg!(windows) {
        "SmartConverter"
    } else {
        "smartconverter"
    };
    dirs::config_dir()
        .map(|d| d.join(sub).join(FILE_NAME))
        .unwrap_or(portable)
}

fn is_writable_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".sc_write_test_{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn parse_bool(value: &str, default: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

/// `ExtN=拡張子:テンプレート` を N=0 から連番で読む。
fn read_ext_list(doc: &IniDocument, section: &str) -> Vec<ScriptTemplate> {
    let mut list = Vec::new();
    for i in 0.. {
        let Some(v) = doc.get(section, &format!("Ext{i}")) else {
            break;
        };
        if let Some((ext, template)) = v.split_once(':') {
            list.push(ScriptTemplate {
                ext: ext.trim().trim_start_matches('.').to_owned(),
                template: template.trim().to_owned(),
            });
        }
    }
    list
}

fn resolve_original_path(base: &Path, p: &str) -> String {
    let rel = p.strip_prefix('\\').filter(|r| !r.starts_with('\\'));
    match rel {
        Some(rel) => base.join(rel).to_string_lossy().into_owned(),
        None => p.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 設定ファイルの場所：実行ファイルの隣にあればそこ。なければ Windows は書き込めれば隣、
    /// Linux は書き込めても ~/.config（$XDG_CONFIG_HOME）。
    #[test]
    fn config_location() {
        let app = std::env::temp_dir().join(format!("sc_config_loc_{}", std::process::id()));
        std::fs::create_dir_all(&app).unwrap();
        let portable = app.join(FILE_NAME);
        let _ = std::fs::remove_file(&portable);
        if cfg!(windows) {
            assert_eq!(default_path(&app), portable);
        } else {
            let p = default_path(&app);
            assert_ne!(p, portable);
            assert!(
                p.ends_with(Path::new("smartconverter").join(FILE_NAME)),
                "{p:?}"
            );
        }
        // 隣に置けば、どの OS でもポータブル
        std::fs::write(&portable, "").unwrap();
        assert_eq!(default_path(&app), portable);
        std::fs::remove_dir_all(&app).unwrap();
    }

    /// 設定ファイルが OS 標準の設定フォルダにあっても、実行ファイルの隣の Transcoding.ini を使える（AT-UI-11）。
    #[test]
    fn preset_path_falls_back_to_app_dir() {
        let root = std::env::temp_dir().join(format!("sc_preset_path_{}", std::process::id()));
        let app = root.join("app");
        let conf = root.join("conf");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&conf).unwrap();
        let config = conf.join(FILE_NAME);

        // どちらにもない → 設定ファイルのフォルダ
        assert_eq!(
            resolve_preset_path(&config, &app, "Transcoding.ini"),
            conf.join("Transcoding.ini")
        );
        // 実行ファイルの隣にだけある → そちら
        std::fs::write(app.join("Transcoding.ini"), "").unwrap();
        assert_eq!(
            resolve_preset_path(&config, &app, "Transcoding.ini"),
            app.join("Transcoding.ini")
        );
        // 両方にある → 設定ファイルのフォルダが先
        std::fs::write(conf.join("Transcoding.ini"), "").unwrap();
        assert_eq!(
            resolve_preset_path(&config, &app, "Transcoding.ini"),
            conf.join("Transcoding.ini")
        );
        // 絶対パスはそのまま
        let abs = root.join("x.ini");
        assert_eq!(
            resolve_preset_path(&config, &app, &abs.to_string_lossy()),
            abs
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn defaults_when_empty() {
        let s = Settings::from_text("");
        assert_eq!(s.preset_file, "Transcoding.ini");
        assert!(s.auto_start_on_add);
        assert_eq!(s.on_exists, OnExists::Rename);
        assert!(s.delete_partial);
        assert_eq!(s.theme, Theme::System);
    }

    #[test]
    fn round_trip_keeps_values_and_comments() {
        let text = "; my comment\n[General]\nDefaultPreset=QVGA標準画質15fpsステレオ\nUnknownKey=keep\n[Output]\nOutputDir=D:\\out\nAddTime=1\nAddTimePos=2\nAddTimeFormat=[yyyymmdd]\nOnExists=Skip\nDeletePartial=0\n[Tools]\nqaac64=C:\\tools\\qaac64.exe\n[Script]\nExt0=TS:C:\\t\\default.vpy\n";
        let s = Settings::from_text(text);
        assert_eq!(s.default_preset, "QVGA標準画質15fpsステレオ");
        assert_eq!(s.output_dir, r"D:\out");
        assert!(s.timestamp.enabled);
        assert_eq!(s.timestamp.pos, TimestampPos::Head);
        assert_eq!(s.on_exists, OnExists::Skip);
        assert!(!s.delete_partial);
        assert_eq!(
            s.tools.get("qaac64").map(String::as_str),
            Some(r"C:\tools\qaac64.exe")
        );
        assert_eq!(s.script_templates[0].ext, "TS");

        let saved = s.to_text();
        assert!(saved.starts_with("; my comment\n"));
        assert!(saved.contains("UnknownKey=keep"));
        assert_eq!(
            Settings::from_text(&saved),
            s.clone_without_doc_for_test(&saved)
        );
    }

    impl Settings {
        /// 比較用：同じ値で doc だけを差し替えたもの。
        fn clone_without_doc_for_test(&self, text: &str) -> Self {
            let mut c = self.clone();
            c.doc = IniDocument::parse(text);
            c
        }
    }

    #[test]
    fn imports_original_settings() {
        let dir = std::env::temp_dir().join(format!("sc_settings_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ini = dir.join("3GP_Converter.ini");
        std::fs::write(
            &ini,
            "[INI]\r\nOutputDir=D:\\Videos\r\nDefaultSetitng=QVGA標準画質15fpsステレオ\r\nExePriority=1\r\nExitIfFinished=1\r\nDisableAVS=0\r\nAllAbort=0\r\nAddTime=1\r\nAddTimePos=0\r\nAddTimeFormat=[mm-dd]\r\n[Avisynth]\r\nExt0=AVI:\\cores\\AVS_Skelton.avs\r\nExt1=TS:\\cores\\AVS_Skelton.avs\r\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("Transcoding.ini"),
            "[Item0]
Title=A
",
        )
        .unwrap();
        let mut s = Settings::default();
        s.import_original(&ini).unwrap();
        assert_eq!(Path::new(&s.preset_file), dir.join("Transcoding.ini"));
        assert_eq!(s.output_dir, r"D:\Videos");
        assert_eq!(s.default_preset, "QVGA標準画質15fpsステレオ");
        assert_eq!(s.priority, 1);
        assert!(s.exit_if_finished);
        assert!(!s.all_abort);
        assert!(s.timestamp.enabled);
        assert_eq!(s.script_templates.len(), 2);
        assert_eq!(
            Path::new(&s.script_templates[0].template),
            dir.join(r"cores\AVS_Skelton.avs")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
