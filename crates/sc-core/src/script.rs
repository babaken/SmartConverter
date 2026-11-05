//! AviSynth / VapourSynth スクリプトの扱い（要件 §4.10）。
//!
//! - 入力そのものが `.avs` / `.vpy` なら、そのままプリセットに渡す（F-AV-01）。
//! - 入力の拡張子にテンプレート（スケルトン）が割り当てられていれば、テンプレートの置換変数を展開した
//!   スクリプトを一時ファイルとして作り、それを `<%InputFile%>` にする（F-AV-02〜03）。
//! - AviSynth+ / VapourSynth が入っているかを調べる（F-AV-05）。無い種類のスケルトンは使わない。
//!
//! 生成するスクリプトの名前は `<%TemporaryFile%>.avs` / `.vpy` なので、ほかの一時ファイルと一緒に消える（F-AV-07）。

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::cmdline;
use crate::settings::ScriptTemplate;
use crate::text;
use crate::vars::{self, Vars};

/// スクリプトの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    AviSynth,
    VapourSynth,
}

impl ScriptKind {
    /// 拡張子（`.avs` / `.vpy`）から種類を決める。
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
        match ext.as_str() {
            "avs" => Some(Self::AviSynth),
            "vpy" => Some(Self::VapourSynth),
            _ => None,
        }
    }

    /// 拡張子（`.` なし）。
    pub fn ext(self) -> &'static str {
        match self {
            Self::AviSynth => "avs",
            Self::VapourSynth => "vpy",
        }
    }

    /// 画面に出す名前。
    pub fn name(self) -> &'static str {
        match self {
            Self::AviSynth => "AviSynth+",
            Self::VapourSynth => "VapourSynth",
        }
    }
}

/// AviSynth+ / VapourSynth の検出結果（見つかった場所）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptSupport {
    pub avisynth: Option<PathBuf>,
    pub vapoursynth: Option<PathBuf>,
}

impl ScriptSupport {
    /// その種類が使えるなら、検出した場所。
    pub fn get(&self, kind: ScriptKind) -> Option<&Path> {
        match kind {
            ScriptKind::AviSynth => self.avisynth.as_deref(),
            ScriptKind::VapourSynth => self.vapoursynth.as_deref(),
        }
    }

    /// 両方とも使えるものとして扱う（テスト用）。
    pub fn all_for_test() -> Self {
        Self {
            avisynth: Some(PathBuf::from("avisynth")),
            vapoursynth: Some(PathBuf::from("vapoursynth")),
        }
    }

    /// AviSynth+ / VapourSynth を探す。
    ///
    /// - AviSynth+：本体のライブラリ（Windows は `AviSynth.dll`、Linux は `libavisynth.so`）
    /// - VapourSynth：`vspipe`（ツールと同じ探し方）、無ければ `VSScript.dll` / `libvapoursynth-script.so`
    ///
    /// ライブラリは読み込まず、ファイルがあるかだけを見る（読み込むと初期化処理が動くため）。
    pub fn detect(
        tools: &BTreeMap<String, String>,
        app_dir: &Path,
        path_env: Option<&OsStr>,
    ) -> Self {
        let dirs = library_dirs(app_dir, path_env);
        let find_lib = |names: &[&str]| {
            dirs.iter()
                .flat_map(|d| names.iter().map(move |n| d.join(n)))
                .find(|p| p.is_file())
        };
        let (avs_libs, vs_libs): (&[&str], &[&str]) = if cfg!(windows) {
            (&["AviSynth.dll"], &["VSScript.dll"])
        } else {
            (
                &["libavisynth.so", "libavisynth.so.10", "libavisynth.so.11"],
                &["libvapoursynth-script.so", "libvapoursynth-script.so.0"],
            )
        };
        Self {
            avisynth: find_lib(avs_libs),
            vapoursynth: cmdline::resolve_program("vspipe", tools, app_dir, path_env)
                .or_else(|| find_lib(vs_libs))
                .or_else(vapoursynth_from_registry),
        }
    }
}

/// ライブラリを探すフォルダ。
/// VapourSynth のインストーラー（pip で入れた場合も）が書くレジストリ
/// （`HKCU` / `HKLM` の `SOFTWARE\VapourSynth` の `VSPipeEXE`・`VSScriptDLL`）から探す（Windows のみ）。
/// ユーザー単位で入れると、PATH に入らない Python のパッケージの中に置かれるため。
fn vapoursynth_from_registry() -> Option<PathBuf> {
    ["VSPipeEXE", "VSScriptDLL"]
        .into_iter()
        .filter_map(|v| crate::platform::registry_string(r"SOFTWARE\VapourSynth", v))
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn library_dirs(app_dir: &Path, path_env: Option<&OsStr>) -> Vec<PathBuf> {
    let mut dirs = vec![app_dir.to_owned()];
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            dirs.push(PathBuf::from(root).join("System32"));
        }
        if let Some(path) = path_env {
            dirs.extend(std::env::split_paths(path));
        }
    } else {
        if let Some(ld) = std::env::var_os("LD_LIBRARY_PATH") {
            dirs.extend(std::env::split_paths(&ld));
        }
        for d in [
            "/usr/local/lib",
            "/usr/local/lib64",
            "/usr/lib",
            "/usr/lib64",
            "/usr/lib/x86_64-linux-gnu",
            "/lib",
            "/lib64",
        ] {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs
}

/// 計画に渡すスクリプトの設定。スケルトン機能を使わないときは `None` にする。
#[derive(Debug, Clone, Copy)]
pub struct ScriptOptions<'a> {
    pub templates: &'a [ScriptTemplate],
    pub support: &'a ScriptSupport,
}

/// 入力の拡張子に割り当てられたテンプレート（大文字小文字を区別しない、最初の一致）。
pub fn find_template<'a>(
    input: &Path,
    templates: &'a [ScriptTemplate],
) -> Option<&'a ScriptTemplate> {
    let ext = input.extension()?.to_string_lossy().into_owned();
    templates
        .iter()
        .find(|t| t.ext.trim_start_matches('.').eq_ignore_ascii_case(&ext))
}

/// テンプレートのパス。相対パス（本家の `\cores\…` の形も含む）は、アプリのフォルダからとみなす。
pub fn template_path(template: &str, app_dir: &Path) -> PathBuf {
    let p = Path::new(template);
    if p.is_absolute() || template.is_empty() {
        return p.to_owned();
    }
    let rel = template.trim_start_matches(['\\', '/']);
    let rel: PathBuf = rel.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    app_dir.join(rel)
}

/// これから作るスクリプト。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedScript {
    pub kind: ScriptKind,
    pub template: PathBuf,
    /// 作るスクリプトのパス（`<%TemporaryFile%>.avs` など）。プリセットの `<%InputFile%>` になる
    pub path: PathBuf,
    /// 書き込む内容（テンプレートが読めなければ理由）
    pub content: Result<Vec<u8>, String>,
    /// テンプレート内の未定義の変数
    pub unknown_vars: Vec<String>,
    /// 書き込むときの注意（文字コードで表せない文字など）
    pub warnings: Vec<String>,
}

/// スケルトンを使うかを決める。使うなら計画を、使えない理由があれば警告を返す。
///
/// `vars` は元の入力ファイルで作ったもの。テンプレートの置換変数はこれで展開する。
pub fn plan_script(
    input: &Path,
    vars: &Vars,
    temporary_base: &Path,
    app_dir: &Path,
    opts: &ScriptOptions<'_>,
) -> (Option<PlannedScript>, Vec<String>) {
    // 入力そのものがスクリプトなら、そのまま使う。
    if ScriptKind::from_path(input).is_some() {
        return (None, Vec::new());
    }
    let Some(t) = find_template(input, opts.templates) else {
        return (None, Vec::new());
    };
    let template = template_path(&t.template, app_dir);
    let Some(kind) = ScriptKind::from_path(&template) else {
        return (
            None,
            vec![format!(
                "テンプレートの種類が分かりません（拡張子は .avs か .vpy）：{}",
                template.display()
            )],
        );
    };
    if opts.support.get(kind).is_none() {
        return (
            None,
            vec![format!(
                "{} が見つからないため、スケルトンを使わずに元のファイルを入力にします",
                kind.name()
            )],
        );
    }

    let mut path = temporary_base.as_os_str().to_owned();
    path.push(".");
    path.push(kind.ext());
    let path = PathBuf::from(path);

    let mut unknown_vars = Vec::new();
    let mut warnings = Vec::new();
    let content = match std::fs::read(&template) {
        Ok(bytes) => {
            let src = text::decode(&bytes).0;
            match unsafe_value(&src, vars) {
                Some(name) => Err(format!(
                    "<%{name}%> に \" か改行が含まれているため、スクリプトを作りません\
                     （スクリプトの文字列が閉じて、別の命令が実行されるおそれがあるため）"
                )),
                None => {
                    let e = vars::expand(&src, vars);
                    unknown_vars = e.unknown;
                    let (out, warn) = encode_script(&e.text, kind);
                    warnings.extend(warn);
                    Ok(out)
                }
            }
        }
        Err(err) => Err(format!(
            "テンプレートを読み込めません：{}（{err}）",
            template.display()
        )),
    };
    (
        Some(PlannedScript {
            kind,
            template,
            path,
            content,
            unknown_vars,
            warnings,
        }),
        Vec::new(),
    )
}

/// テンプレートが使う変数のうち、値に `"` か改行を含むものの名前を返す。
///
/// テンプレートでは変数を `"<%InputFile%>"`（AviSynth）や `r"<%InputFile%>"`（Python）のように
/// 文字列の中に書く。値に `"` や改行があると文字列が閉じて、ファイル名に書いた命令が
/// スクリプトとして実行されてしまう（Linux ではファイル名に `"` を使える）。
fn unsafe_value(template: &str, vars: &Vars) -> Option<&'static str> {
    let lower = template.to_ascii_lowercase();
    vars::NAMES.into_iter().find(|name| {
        lower.contains(&format!("<%{}%>", name.to_ascii_lowercase()))
            && vars
                .get(name)
                .is_some_and(|v| v.contains(['"', '\n', '\r']))
    })
}

/// 生成したスクリプトの文字コードを決めて、バイト列にする。
///
/// - Windows の `.avs`：AviSynth+ はスクリプトを ANSI（日本語環境では Shift-JIS）として読むので、
///   テンプレートの文字コードによらず Shift-JIS で書く。表せない文字があれば警告する。
/// - それ以外（`.vpy` と Linux の `.avs`）：UTF-8（BOM なし）。
fn encode_script(text: &str, kind: ScriptKind) -> (Vec<u8>, Option<String>) {
    if cfg!(windows) && kind == ScriptKind::AviSynth {
        let (bytes, _, lossy) = encoding_rs::SHIFT_JIS.encode(text);
        let warn = lossy.then(|| {
            "Shift-JIS で表せない文字があるため、スクリプト内のパスが正しく読めない可能性があります"
                .to_owned()
        });
        return (bytes.into_owned(), warn);
    }
    (text.as_bytes().to_vec(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl(ext: &str, template: &str) -> ScriptTemplate {
        ScriptTemplate {
            ext: ext.into(),
            template: template.into(),
        }
    }

    #[test]
    fn kinds_and_templates() {
        assert_eq!(
            ScriptKind::from_path(Path::new("a.AVS")),
            Some(ScriptKind::AviSynth)
        );
        assert_eq!(
            ScriptKind::from_path(Path::new("a.vpy")),
            Some(ScriptKind::VapourSynth)
        );
        assert_eq!(ScriptKind::from_path(Path::new("a.ts")), None);

        let list = [tpl("TS", "a.avs"), tpl(".mp4", "b.vpy"), tpl("ts", "c.avs")];
        assert_eq!(
            find_template(Path::new("x.ts"), &list).unwrap().template,
            "a.avs"
        );
        assert_eq!(
            find_template(Path::new("x.MP4"), &list).unwrap().template,
            "b.vpy"
        );
        assert!(find_template(Path::new("x.mkv"), &list).is_none());
        assert!(find_template(Path::new("noext"), &list).is_none());
    }

    #[test]
    fn template_paths() {
        let app = Path::new("app");
        assert_eq!(
            template_path(r"\cores\AVS_Skelton.avs", app),
            app.join("cores").join("AVS_Skelton.avs")
        );
        assert_eq!(
            template_path("templates/default.vpy", app),
            app.join("templates").join("default.vpy")
        );
        let abs = std::env::temp_dir().join("t.avs");
        assert_eq!(template_path(&abs.to_string_lossy(), app), abs);
    }

    #[test]
    fn encodes_by_kind() {
        let (b, w) = encode_script("x=r\"動画\"", ScriptKind::VapourSynth);
        assert_eq!(b, "x=r\"動画\"".as_bytes());
        assert!(w.is_none());

        let (b, w) = encode_script("動画", ScriptKind::AviSynth);
        if cfg!(windows) {
            assert_eq!(b, encoding_rs::SHIFT_JIS.encode("動画").0.as_ref());
            assert!(w.is_none());
            let (_, w) = encode_script("😀", ScriptKind::AviSynth);
            assert!(w.is_some());
        } else {
            assert_eq!(b, "動画".as_bytes());
        }
    }

    /// ファイル名に " があっても、スクリプトに命令を差し込めない（スクリプトを作らない）。
    #[test]
    fn rejects_values_that_break_out_of_strings() {
        let template = r#"core.lsmas.LWLibavSource(r"<%InputFile%>")"#;
        let bad = Vars {
            input_file: r#"/v/x"); import os; os.system("id"); ("#.into(),
            ..Default::default()
        };
        assert_eq!(unsafe_value(template, &bad), Some("InputFile"));
        let newline = Vars {
            input_file: "/v/a\nimport os.ts".into(),
            ..Default::default()
        };
        assert_eq!(unsafe_value(template, &newline), Some("InputFile"));
        // テンプレートが使わない変数は見ない
        let other = Vars {
            input_file: "/v/ok.ts".into(),
            title: "a\"b".into(),
            ..Default::default()
        };
        assert_eq!(unsafe_value(template, &other), None);
    }

    /// PATH に無くても、レジストリに書かれた VapourSynth を見つける（Windows で、レジストリがある環境のみ）。
    #[test]
    fn detects_vapoursynth_from_registry() {
        let from_reg = vapoursynth_from_registry();
        if let Some(p) = &from_reg {
            assert!(p.is_file());
            let s =
                ScriptSupport::detect(&BTreeMap::new(), Path::new("Z:/none"), Some(OsStr::new("")));
            assert!(s.get(ScriptKind::VapourSynth).is_some());
        }
        eprintln!("registry: {from_reg:?}");
    }

    #[test]
    fn detect_does_not_panic() {
        let s = ScriptSupport::detect(&BTreeMap::new(), Path::new("."), None);
        let _ = s.get(ScriptKind::AviSynth);
    }
}
