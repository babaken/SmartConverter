//! CLI の結合テスト（受け入れテスト AT-PR-01、AT-PR-05、AT-CL-01〜03、AT-CL-05、AT-QU-08 の自動化できる部分）。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../sc-core/tests/fixtures/ini/transcoding_sjis.ini"
);

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sc_cli_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SmartConverter.ini"),
            format!(
                "[Output]\nOutputDir={}\n",
                dir.join("default_out").display()
            ),
        )
        .unwrap();
        std::fs::write(dir.join("テスト 動画.ts"), b"dummy").unwrap();
        Self { dir }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_smartconverter-cli"))
            .arg("--config")
            .arg(self.dir.join("SmartConverter.ini"))
            .arg("--ini")
            .arg(FIXTURE)
            .args(args)
            .output()
            .unwrap()
    }

    fn input(&self) -> String {
        self.dir
            .join("テスト 動画.ts")
            .to_string_lossy()
            .into_owned()
    }

    fn path(&self, p: &str) -> String {
        self.dir.join(p).to_string_lossy().into_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn list_presets_shows_eight_items() {
    let sb = Sandbox::new("list");
    let o = sb.run(&["--list-presets"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("  1  QVGA標準画質15fpsステレオ（2 段）"));
    assert!(out.contains("  7  音だけ Opus 96kbps"));
    assert!(!out.contains("  8  "));
    assert!(out.contains("0Item"));
}

#[test]
fn dry_run_with_zero_padded_index_and_output_dir() {
    let sb = Sandbox::new("dry");
    let out_dir = sb
        .dir
        .join("test")
        .join("001")
        .to_string_lossy()
        .into_owned();
    let o = sb.run(&["-i", &sb.input(), "-p", "001", "-o", &out_dir, "--dry-run"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("[Item1] QVGA標準画質15fpsステレオ（2 段）"));
    let expected = Path::new(&out_dir).join("テスト 動画.3gp");
    assert!(
        out.contains(&expected.to_string_lossy().into_owned()),
        "{out}"
    );
    // dry-run では出力先を作らない
    assert!(!Path::new(&out_dir).exists());
}

#[test]
fn output_dir_defaults_to_gui_setting() {
    let sb = Sandbox::new("default_out");
    let o = sb.run(&["-p", "6", &sb.input(), "--dry-run"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let expected = Path::new(&sb.path("default_out")).join("テスト 動画.m4a");
    assert!(stdout(&o).contains(&expected.to_string_lossy().into_owned()));
}

#[test]
fn preset_by_title() {
    let sb = Sandbox::new("title");
    let o = sb.run(&["-S", "音だけ AAC 96kbps", &sb.input(), "--dry-run"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stdout(&o).contains("[Item6] 音だけ AAC 96kbps"));
}

#[test]
fn missing_ini_reports_cause_once() {
    let sb = Sandbox::new("missing_ini");
    let o = Command::new(env!("CARGO_BIN_EXE_smartconverter-cli"))
        .arg("--config")
        .arg(sb.dir.join("SmartConverter.ini"))
        .arg("--ini")
        .arg(sb.dir.join("nothere.ini"))
        .arg("--list-presets")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    let err = stderr(&o);
    assert!(err.contains("nothere.ini を読み込めません"), "{err}");
    // 原因（OS のエラー文）は 1 回だけ
    assert_eq!(err.matches("(os error 2)").count(), 1, "{err}");
}

#[test]
fn usage_errors_exit_with_2() {
    let sb = Sandbox::new("errors");
    let input = sb.input();
    let cases: Vec<Vec<&str>> = vec![
        vec!["-p", "1", "-S", "音だけ AAC 96kbps", &input, "--dry-run"],
        vec!["-p", "9", &input, "--dry-run"],
        vec!["-p", "x", &input, "--dry-run"],
        vec!["-p", "1", "--dry-run"],
        vec!["-p", "1", "no_such_file.ts", "--dry-run"],
    ];
    for args in cases {
        let o = sb.run(&args);
        assert_eq!(o.status.code(), Some(2), "args {args:?}: {}", stderr(&o));
    }
    let dir = sb.dir.to_string_lossy().into_owned();
    let o = sb.run(&["-p", "1", &dir, "--dry-run"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("フォルダは指定できません"));
}

#[test]
fn drive_root_is_rejected() {
    let sb = Sandbox::new("root");
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let o = sb.run(&["-p", "1", "-o", root, &sb.input(), "--dry-run"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("ドライブのルート"));
}
