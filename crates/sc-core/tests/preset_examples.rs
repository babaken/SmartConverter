//! 同梱するプリセット例（presets/）とスケルトン例（templates/）の確認。

use std::path::{Path, PathBuf};

use sc_core::cmdline;
use sc_core::preset::{self, PresetSet};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load(name: &str) -> PresetSet {
    PresetSet::load(&repo().join("presets").join(name)).expect("プリセット例を読み込めない")
}

/// プログラム名（Windows 規則の分解は OS によらず使える）。
fn program(command: &str) -> String {
    cmdline::split_program_windows(command).0
}

/// 同梱するプリセット例（両 OS 共通）。
const EXAMPLES: &[&str] = &["3gpp_aac.ini"];

#[test]
fn examples_are_valid() {
    for name in EXAMPLES {
        let set = load(name);
        assert!(set.presets.len() >= 5, "{name}");
        assert!(
            set.ignored_sections.is_empty(),
            "{name}: {:?}",
            set.ignored_sections
        );
        assert_eq!(
            preset::validate(&set.presets),
            Vec::<String>::new(),
            "{name}"
        );
        for p in &set.presets {
            for c in &p.commands {
                // 一時ファイルはアプリが消すので、削除の行は入れない
                assert!(!sc_core::plan::is_cleanup_command(c), "{name}: {c}");
                assert!(c.contains("<%InputFile%>") || c.contains("<%TemporaryFile%>"));
            }
        }
    }
}

/// 両 OS で使えるように ffmpeg だけを使い、Windows の規則でも POSIX シェルの規則でも同じに分けられる。
#[test]
fn examples_use_only_ffmpeg_on_both_os() {
    for name in EXAMPLES {
        for p in &load(name).presets {
            for c in &p.commands {
                assert_eq!(program(c), "ffmpeg", "{name}: {}", p.title);
                assert!(!c.contains(".exe") && !c.contains("<%AppPath%>"), "{c}");
                let words = shell_words_split(c);
                assert_eq!(words[0], "ffmpeg");
                assert!(words.iter().any(|w| w == "<%InputFile%>"), "{c}");
                assert!(words.iter().any(|w| w.starts_with("<%OutputFile%>")), "{c}");
            }
        }
    }
}

#[test]
fn skeleton_examples_expand_input() {
    let vars = sc_core::vars::Vars::new(
        Path::new("/videos/a b.ts"),
        Path::new("/out"),
        "a b",
        Path::new("/tmp/sc_1"),
        Path::new("/app"),
        &chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
            .unwrap()
            .and_hms_opt(3, 4, 5)
            .unwrap(),
    );
    for (name, needle) in [
        ("default.avs", "LWLibavVideoSource(\""),
        ("default.vpy", "LWLibavSource(r\""),
    ] {
        let bytes = std::fs::read(repo().join("templates").join(name)).unwrap();
        let text = sc_core::text::decode(&bytes).0;
        let e = sc_core::vars::expand(&text, &vars);
        assert!(e.unknown.is_empty(), "{name}: {:?}", e.unknown);
        let input = &vars.input_file;
        assert!(
            e.text.contains(&format!("{needle}{input}\")")),
            "{name}: {}",
            e.text
        );
    }
}

/// POSIX シェルの規則で分ける（`cmdline::parse` は OS で規則が変わるため、ここで直接使う）。
fn shell_words_split(c: &str) -> Vec<String> {
    shell_words::split(c).expect("引用符が閉じていない")
}
