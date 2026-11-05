//! 1 ジョブ分の実行内容を組み立てる（基本設計 §4.1 の 1〜5a）。
//!
//! 出力名・一時ファイル名・置換変数を決め、各コマンドを展開してツールの場所を調べる。
//! ファイルは作らない（CLI の `--dry-run`、実行、プリセット編集の確認欄で共通に使う）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;

use crate::cmdline;
use crate::naming::{self, OnExists, OutputDecision, TimestampOptions};
use crate::preset::Preset;
use crate::script::{self, PlannedScript, ScriptOptions};
use crate::vars::{self, Vars};

/// 計画に必要な設定値。
#[derive(Debug, Clone)]
pub struct PlanOptions<'a> {
    pub output_dir: &'a Path,
    pub temporary_dir: &'a Path,
    pub on_exists: OnExists,
    pub timestamp: &'a TimestampOptions,
    pub tools: &'a BTreeMap<String, String>,
    pub app_dir: &'a Path,
    /// ツールを探す PATH（`None` なら PATH は探さない）
    pub path_env: Option<std::ffi::OsString>,
    /// スケルトン機能（使わないなら `None`）
    pub script: Option<ScriptOptions<'a>>,
}

/// 展開したコマンド 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    /// プリセットに書かれたコマンド
    pub template: String,
    /// 変数を展開したコマンド
    pub expanded: String,
    /// 未定義の変数
    pub unknown_vars: Vec<String>,
    /// プログラム名（解釈できなければ `None`）
    pub program: Option<String>,
    /// 見つかったツールのパス
    pub resolved: Option<PathBuf>,
    /// 一時ファイルの削除だけを行う行（`cmd /c del …`）か
    pub is_cleanup: bool,
}

/// 1 ジョブ分の計画。
#[derive(Debug, Clone)]
pub struct JobPlan {
    pub input: PathBuf,
    /// `None` なら同名ファイルがあるためスキップ
    pub output: Option<PlannedOutput>,
    pub temporary_base: PathBuf,
    /// コマンドの展開に使った変数（スケルトンを使うなら `input_file` は生成するスクリプト）
    pub vars: Vars,
    /// スケルトンから作るスクリプト
    pub script: Option<PlannedScript>,
    /// 計画時の警告（スケルトンを使えなかった理由など）
    pub warnings: Vec<String>,
    pub commands: Vec<PlannedCommand>,
}

/// 出力の決定内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOutput {
    pub title: String,
    /// 出力ファイル（拡張子なし）
    pub base: PathBuf,
    /// 同名ファイルがあったため連番を付けたか
    pub renamed: bool,
}

/// 計画を立てる。
pub fn plan_job(
    input: &Path,
    mtime: &NaiveDateTime,
    preset: &Preset,
    job_id: u64,
    opts: &PlanOptions<'_>,
) -> JobPlan {
    let input_base = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = naming::build_title(&input_base, opts.timestamp, mtime);
    let decision = naming::decide_output(opts.output_dir, &title, opts.on_exists);
    let temporary_base = naming::temporary_base(opts.temporary_dir, job_id);

    let (output, var_title) = match decision {
        OutputDecision::Use { title, renamed } => (
            Some(PlannedOutput {
                base: opts.output_dir.join(&title),
                title: title.clone(),
                renamed,
            }),
            title,
        ),
        OutputDecision::Skip => (None, title),
    };

    let mut vars = Vars::new(
        input,
        opts.output_dir,
        &var_title,
        &temporary_base,
        opts.app_dir,
        mtime,
    );

    // スケルトン：テンプレートは元の入力で展開し、コマンドの <%InputFile%> は生成するスクリプトにする。
    let (script, warnings) = match &opts.script {
        Some(so) => script::plan_script(input, &vars, &temporary_base, opts.app_dir, so),
        None => (None, Vec::new()),
    };
    if let Some(s) = &script {
        vars.input_file = s.path.to_string_lossy().into_owned();
    }

    let commands = preset
        .commands
        .iter()
        .map(|template| {
            // Linux は引数をシェルの規則で分けるので、値の引用符などをエスケープする（引数の差し込み対策）。
            // Windows はファイル名に " を使えず、プログラムの後ろはそのまま渡すので、本家と同じく置き換えるだけ。
            let e = if cfg!(windows) {
                vars::expand(template, &vars)
            } else {
                vars::expand_posix(template, &vars)
            };
            let program = cmdline::parse(&e.text).ok().map(|c| c.program);
            let resolved = program.as_deref().and_then(|p| {
                cmdline::resolve_program(p, opts.tools, opts.app_dir, opts.path_env.as_deref())
            });
            PlannedCommand {
                is_cleanup: is_cleanup_command(template),
                template: template.clone(),
                expanded: e.text,
                unknown_vars: e.unknown,
                program,
                resolved,
            }
        })
        .collect();

    JobPlan {
        input: input.to_owned(),
        output,
        temporary_base,
        vars,
        script,
        warnings,
        commands,
    }
}

/// 一時ファイルを消すだけの行か（`cmd /c del "<%TemporaryFile%>…"`、`rm "<%TemporaryFile%>…"`）。
///
/// アプリが一時ファイルを自動で消すので、この行は不要（要件 F-EX-07）。画面で案内するために使う。
pub fn is_cleanup_command(template: &str) -> bool {
    let t = template.to_ascii_lowercase();
    let deletes =
        t.contains(" del ") || t.contains("\"del ") || t.starts_with("rm ") || t.contains(" rm ");
    deletes && t.contains("<%temporaryfile%>")
}

/// ファイルの更新日時（ローカル時刻）。取れなければ現在時刻。
pub fn file_mtime(path: &Path) -> NaiveDateTime {
    let time = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| std::time::SystemTime::now());
    chrono::DateTime::<chrono::Local>::from(time).naive_local()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::TimestampPos;

    #[test]
    fn plans_commands_and_flags() {
        let dir = std::env::temp_dir().join(format!("sc_plan_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let preset = Preset {
            extra: Vec::new(),
            index: 0,
            title: "P".into(),
            commands: vec![
                r#""no_such_tool_xyz" -i "<%InputFile%>" "<%OutputFile%>.mp4""#.into(),
                r#"x "<%Bad%>""#.into(),
                r#"cmd /c "del "<%TemporaryFile%>*.*"""#.into(),
            ],
        };
        let ts = TimestampOptions {
            enabled: true,
            pos: TimestampPos::HeadWithSpace,
            format: "[mm-dd]".into(),
        };
        let tools = BTreeMap::new();
        let opts = PlanOptions {
            output_dir: &dir,
            temporary_dir: &dir,
            on_exists: OnExists::Rename,
            timestamp: &ts,
            tools: &tools,
            app_dir: &dir,
            path_env: None,
            script: None,
        };
        let mtime = chrono::NaiveDate::from_ymd_opt(2024, 3, 5)
            .unwrap()
            .and_hms_opt(7, 8, 9)
            .unwrap();
        let plan = plan_job(&dir.join("movie.ts"), &mtime, &preset, 1, &opts);

        let out = plan.output.as_ref().unwrap();
        assert_eq!(out.title, "[03-05] movie");
        assert_eq!(out.base, dir.join("[03-05] movie"));
        assert!(plan.commands[0].expanded.contains("[03-05] movie.mp4"));
        assert_eq!(
            plan.commands[0].program.as_deref(),
            Some("no_such_tool_xyz")
        );
        assert!(plan.commands[0].resolved.is_none());
        assert_eq!(plan.commands[1].unknown_vars, vec!["<%Bad%>"]);
        assert!(!plan.commands[0].is_cleanup);
        assert!(plan.commands[2].is_cleanup);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
