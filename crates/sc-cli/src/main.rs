//! SmartConverter の CLI（`smartconverter-cli`）。
//!
//! 仕様は `docs/basic-design.md` §2.4、要件 F-CL-01〜12。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::Parser;
use sc_core::naming::OnExists;
use sc_core::plan::{self, PlanOptions};
use sc_core::preset::{Preset, PresetSet};
use sc_core::script::ScriptSupport;
use sc_core::settings::{self, Settings};
use sc_core::{paths, vars};

mod convert;
use convert::{EXIT_ABORTED, EXIT_OK};

/// 設定・引数エラーの終了コード（要件 F-CL-11）。
const EXIT_USAGE: u8 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "smartconverter-cli",
    version,
    about = "SmartConverter の CLI 版",
    after_help = "例:\n  smartconverter-cli -i XXXX.ts -p 001 -o /test/001\n  smartconverter-cli --list-presets"
)]
struct Args {
    /// 入力ファイル（繰り返し可）
    #[arg(short = 'i', long = "input", value_name = "FILE")]
    input: Vec<PathBuf>,

    /// 入力ファイル（オプションの後ろに並べてもよい）
    #[arg(value_name = "FILE")]
    files: Vec<String>,

    /// [ItemN] の N でプリセットを指定（001 → [Item1]）
    #[arg(
        short = 'p',
        long = "preset",
        value_name = "N",
        conflicts_with = "preset_title"
    )]
    preset: Option<String>,

    /// タイトルでプリセットを指定（本家互換）
    #[arg(short = 'S', long = "preset-title", value_name = "TITLE")]
    preset_title: Option<String>,

    /// 出力先フォルダ（省略時は GUI で設定した出力先）
    #[arg(short = 'o', long = "output-dir", value_name = "DIR")]
    output_dir: Option<PathBuf>,

    /// 使う Transcoding.ini
    #[arg(long, value_name = "FILE")]
    ini: Option<PathBuf>,

    /// 使う SmartConverter.ini
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// 同名ファイルがあるとき（rename | overwrite | skip）
    #[arg(long = "on-exists", value_name = "MODE", value_parser = parse_on_exists)]
    on_exists: Option<OnExists>,

    /// 子プロセスの優先度（0〜3）
    #[arg(long, value_name = "0-3", value_parser = clap::value_parser!(u8).range(0..=3))]
    priority: Option<u8>,

    /// 一時フォルダ
    #[arg(long = "temp-dir", value_name = "DIR")]
    temp_dir: Option<PathBuf>,

    /// プリセット一覧を表示して終了
    #[arg(long = "list-presets")]
    list_presets: bool,

    /// 展開後のコマンドを表示するだけで実行しない
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// ログをファイルに保存
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,

    /// 進捗を表示しない
    #[arg(short, long)]
    quiet: bool,

    /// 外部ツールの出力もそのまま表示
    #[arg(short, long)]
    verbose: bool,
}

fn parse_on_exists(s: &str) -> Result<OnExists, String> {
    OnExists::parse(s).ok_or_else(|| "rename / overwrite / skip のどれかを指定してください".into())
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("エラー: {}", error_message(&e));
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// エラーと原因を `: ` でつなぐ。sc-core のエラーは原因を文に含めているので、同じ文は繰り返さない。
fn error_message(e: &anyhow::Error) -> String {
    let mut msg = e.to_string();
    for cause in e.chain().skip(1) {
        let s = cause.to_string();
        if !msg.contains(&s) {
            msg.push_str(": ");
            msg.push_str(&s);
        }
    }
    msg
}

fn run(args: Args) -> Result<u8> {
    let app_dir = app_dir();
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| settings::default_path(&app_dir));
    let mut settings = Settings::load(&config_path)
        .with_context(|| format!("設定ファイル {} を読み込めません", config_path.display()))?;
    apply_overrides(&mut settings, &args);

    let ini_path = match &args.ini {
        Some(p) => p.clone(),
        None => settings::resolve_preset_path(&config_path, &app_dir, &settings.preset_file),
    };
    let presets = PresetSet::load(&ini_path)?;
    for w in &presets.warnings {
        eprintln!("注意: {}:{}: {}", ini_path.display(), w.line, w.message);
    }

    if args.list_presets {
        list_presets(&presets, &ini_path);
        return Ok(EXIT_OK);
    }

    let preset = select_preset(&presets, &args, &settings)?;
    let inputs = collect_inputs(&args)?;
    let output_dir = match &args.output_dir {
        Some(p) => p.clone(),
        None if !settings.output_dir.is_empty() => PathBuf::from(&settings.output_dir),
        None => {
            bail!("出力先が指定されていません（-o で指定するか、GUI で出力先を設定してください）")
        }
    };
    let output_dir = paths::absolute(&output_dir);
    if paths::is_root(&output_dir) {
        bail!(
            "ドライブのルートには出力できません: {}",
            output_dir.display()
        );
    }

    if args.dry_run {
        dry_run(&inputs, preset, &output_dir, &settings, &app_dir);
        return Ok(EXIT_OK);
    }

    // Ctrl+C（SIGINT）・SIGTERM・SIGHUP で、実行中のツールも止め、一時ファイルを消してから終了する（F-CL-12）。
    // ctrlc はどのシグナルかを教えないので、文言は Ctrl+C に限らない書き方にする。終了コードはどれも 130。
    let abort = Arc::new(AtomicBool::new(false));
    {
        let abort = abort.clone();
        ctrlc::set_handler(move || {
            if !abort.swap(true, Ordering::SeqCst) {
                eprintln!("\n中断しています…（もう一度中断すると強制終了します）");
            } else {
                std::process::exit(EXIT_ABORTED as i32);
            }
        })
        .context("中断（Ctrl+C・SIGTERM）の処理を設定できません")?;
    }

    convert::Convert {
        inputs: &inputs,
        preset,
        output_dir: &output_dir,
        settings: &settings,
        app_dir: &app_dir,
        quiet: args.quiet,
        verbose: args.verbose,
        log: args.log.as_deref(),
        abort,
    }
    .run()
}

/// 実行ファイルのあるフォルダ。
fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn apply_overrides(settings: &mut Settings, args: &Args) {
    if let Some(v) = args.on_exists {
        settings.on_exists = v;
    }
    if let Some(v) = args.priority {
        settings.priority = v;
    }
    if let Some(v) = &args.temp_dir {
        settings.temporary_folder = v.to_string_lossy().into_owned();
    }
}

fn list_presets(presets: &PresetSet, path: &Path) {
    println!(
        "{}（{}、{}）",
        path.display(),
        if presets.info.title.is_empty() {
            "名前なし"
        } else {
            &presets.info.title
        },
        presets.encoding.display_name()
    );
    println!("  N  タイトル");
    for p in &presets.presets {
        println!("{:>3}  {}", p.index, p.title);
    }
    if !presets.ignored_sections.is_empty() {
        println!(
            "（プリセットとして読まれないセクション: {}）",
            presets.ignored_sections.join(", ")
        );
    }
}

fn select_preset<'a>(
    presets: &'a PresetSet,
    args: &Args,
    settings: &Settings,
) -> Result<&'a Preset> {
    if let Some(n) = &args.preset {
        let index: usize = n
            .trim()
            .parse()
            .with_context(|| format!("-p には [ItemN] の N（数字）を指定してください: {n}"))?;
        return presets.by_index(index).with_context(|| {
            format!(
                "プリセット [Item{index}] がありません（0〜{} を指定できます）",
                presets.presets.len().saturating_sub(1)
            )
        });
    }
    if let Some(t) = &args.preset_title {
        return presets
            .by_title(t)
            .with_context(|| format!("タイトル「{t}」のプリセットがありません"));
    }
    if settings.default_preset.is_empty() {
        bail!("プリセットが指定されていません（-p または -S で指定してください）");
    }
    presets.by_title(&settings.default_preset).with_context(|| {
        format!(
            "既定のプリセット「{}」がありません（-p または -S で指定してください）",
            settings.default_preset
        )
    })
}

/// 入力ファイルを集める（Windows ではワイルドカードをアプリ側で展開する）。
fn collect_inputs(args: &Args) -> Result<Vec<PathBuf>> {
    let mut list = Vec::new();
    let raw = args
        .input
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .chain(args.files.iter().cloned());
    for arg in raw {
        if cfg!(windows) && (arg.contains('*') || arg.contains('?')) {
            let mut matched = false;
            for entry in
                glob::glob(&arg).with_context(|| format!("ワイルドカードが不正です: {arg}"))?
            {
                let p = entry?;
                if p.is_file() {
                    list.push(p);
                    matched = true;
                }
            }
            if !matched {
                bail!("一致するファイルがありません: {arg}");
            }
        } else {
            list.push(PathBuf::from(arg));
        }
    }
    if list.is_empty() {
        bail!("入力ファイルを指定してください（-i または ファイル名）");
    }
    for p in &list {
        if p.is_dir() {
            bail!("フォルダは指定できません: {}", p.display());
        }
        if !p.is_file() {
            bail!("入力ファイルがありません: {}", p.display());
        }
    }
    Ok(list.into_iter().map(|p| paths::absolute(&p)).collect())
}

fn dry_run(
    inputs: &[PathBuf],
    preset: &Preset,
    output_dir: &Path,
    settings: &Settings,
    app_dir: &Path,
) {
    let temp_dir = settings.temporary_dir();
    let path_env = std::env::var_os("PATH");
    let support = ScriptSupport::detect(&settings.tools, app_dir, path_env.as_deref());
    let opts = PlanOptions {
        output_dir,
        temporary_dir: &temp_dir,
        on_exists: settings.on_exists,
        timestamp: &settings.timestamp,
        tools: &settings.tools,
        app_dir,
        path_env,
        script: convert::script_options(settings, &support),
    };

    let total = inputs.len();
    for (i, input) in inputs.iter().enumerate() {
        let mtime = plan::file_mtime(input);
        let job = plan::plan_job(input, &mtime, preset, i as u64 + 1, &opts);
        println!("[{}/{}] {}", i + 1, total, input.display());
        println!("  プリセット: [Item{}] {}", preset.index, preset.title);
        match &job.output {
            Some(out) => {
                let note = if out.renamed {
                    "（同名ファイルがあるため連番）"
                } else {
                    ""
                };
                println!("  出力: {}.*{note}", out.base.display());
            }
            None => {
                println!("  出力: 同名のファイルがあるためスキップします");
                continue;
            }
        }
        println!("  一時ファイル: {}.*", job.temporary_base.display());
        for w in &job.warnings {
            println!("  ⚠ {w}");
        }
        if let Some(s) = &job.script {
            println!(
                "  スクリプト: {}（{} のテンプレート {} から作ります）",
                s.path.display(),
                s.kind.name(),
                s.template.display()
            );
            if let Err(e) = &s.content {
                println!("     ✗ {e}");
            }
            for u in &s.unknown_vars {
                println!("     ⚠ テンプレートの未定義の変数 {u}");
            }
            for w in &s.warnings {
                println!("     ⚠ {w}");
            }
        }
        for (n, c) in job.commands.iter().enumerate() {
            println!("  {n}: {}", c.expanded);
            for u in &c.unknown_vars {
                println!("     ⚠ 未定義の変数 {u}（置き換えずに文字のまま実行されます）");
            }
            match (&c.program, &c.resolved) {
                (None, _) => println!("     ✗ コマンドを解釈できません"),
                (Some(p), None) => println!("     ⚠ ツールが見つかりません: {p}"),
                (Some(_), Some(path)) => println!("     → {}", path.display()),
            }
            if c.is_cleanup {
                println!("     ⓘ 一時ファイルはアプリが自動で削除するため、この行は不要です");
            }
        }
    }
    println!();
    println!(
        "置換変数: {}",
        vars::NAMES
            .iter()
            .map(|n| format!("<%{n}%>"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
