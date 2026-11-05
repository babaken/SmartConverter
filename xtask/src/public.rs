//! `cargo xtask public <出力先> [--ref <リビジョン>]`：公開リポジトリ用のファイル一式を作る（開発ガイド §6.3）。
//!
//! 指定したリビジョン（既定は `HEAD`）のファイルを `git archive` で取り出し、
//! `public/public.json` の設定に従って、内部向けのファイルを除き、文書を公開用に書き換えて出力先に書き出す。
//! 最後に、内部向けの記述・個人のパス・メールアドレス・切れたリンクが残っていないかを確かめる。
//!
//! 設定と公開用の文書は `public/` に置く（`public/` 自体は公開リポジトリに入れない）。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::workspace_root;

/// 設定と公開用の文書を置くフォルダ。
const CONFIG_DIR: &str = "public/";
const CONFIG_FILE: &str = "public/public.json";

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    data: Vec<u8>,
    executable: bool,
}

type Files = BTreeMap<String, Entry>;

#[derive(Debug)]
enum Rule {
    /// `old` を `new` にする（ちょうど 1 か所あること）。
    Replace { old: String, new: String },
    /// `heading` で始まる行から最後までを、`public/` の `fragment` の中身にする。
    ReplaceFrom { heading: String, fragment: String },
    /// 見出しの行に `name` の列がある表から、その列を除く。
    DropColumn { name: String },
}

#[derive(Debug)]
struct Config {
    exclude: Vec<String>,
    markers: Vec<String>,
    rules: Vec<(String, Vec<Rule>)>,
}

pub fn run(args: &[String]) -> Result<()> {
    let mut out = None;
    let mut rev = "HEAD".to_string();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ref" => {
                rev = it
                    .next()
                    .context("--ref のあとにリビジョンを書いてください")?
                    .clone()
            }
            s if s.starts_with('-') => {
                bail!("public の引数が正しくありません: {s}（使えるのは --ref）")
            }
            s if out.is_none() => out = Some(s.to_string()),
            s => bail!("出力先は 1 つだけ指定してください: {s}"),
        }
    }
    let out = out.context("使い方: cargo xtask public <出力先> [--ref <リビジョン>]")?;
    let out = Path::new(&out);
    if out.exists() && fs::read_dir(out)?.next().is_some() {
        bail!(
            "出力先 {} が空ではありません（空のフォルダか、無い名前を指定してください）",
            out.display()
        );
    }

    let files = read_archive(&workspace_root(), &rev)?;
    let total = files.len();
    let public = build(files)?;
    let problems = check(&public.files, &public.markers);
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("  {p}");
        }
        bail!(
            "公開してはいけない記述が {} か所あります（何も書き出していません）",
            problems.len()
        );
    }

    for (path, e) in &public.files {
        let dst = out.join(path);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dst, &e.data).with_context(|| format!("{} を書き出せません", dst.display()))?;
        #[cfg(unix)]
        if e.executable {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dst, fs::Permissions::from_mode(0o755))?;
        }
    }
    println!(
        "{rev} の {total} 個のファイルから、{} 個を書き出しました: {}",
        public.files.len(),
        out.display()
    );
    println!("除いたファイル: {}", public.excluded.join("、"));
    println!("書き換えたファイル: {}", public.changed.join("、"));
    let exe: Vec<&str> = public
        .files
        .iter()
        .filter(|(_, e)| e.executable)
        .map(|(p, _)| p.as_str())
        .collect();
    if !exe.is_empty() {
        println!(
            "実行できるファイル（Windows でコミットするときは `git add --chmod=+x` を付ける）: {}",
            exe.join("、")
        );
    }
    Ok(())
}

/// `git archive` でリビジョンのファイルを取り出す。
fn read_archive(root: &Path, rev: &str) -> Result<Files> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["archive", "--format=tar", rev])
        .output()
        .context("git を起動できません")?;
    if !output.status.success() {
        bail!(
            "git archive {rev} が失敗しました: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut files = Files::new();
    let mut archive = tar::Archive::new(&output.stdout[..]);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        let executable = entry.header().mode()? & 0o111 != 0;
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data)?;
        files.insert(path, Entry { data, executable });
    }
    Ok(files)
}

struct Public {
    files: Files,
    markers: Vec<String>,
    excluded: Vec<String>,
    changed: Vec<String>,
}

/// 設定に従って、公開するファイルを組み立てる。
fn build(mut files: Files) -> Result<Public> {
    let config_text = text_of(&files, CONFIG_FILE)?;
    let config = parse_config(&config_text)?;
    let fragments: BTreeMap<String, String> = files
        .iter()
        .filter(|(p, _)| p.starts_with(CONFIG_DIR))
        .map(|(p, e)| {
            (
                p[CONFIG_DIR.len()..].to_string(),
                String::from_utf8_lossy(&e.data).into_owned(),
            )
        })
        .collect();
    files.retain(|p, _| !p.starts_with(CONFIG_DIR));

    // 名前が変わって除き損ねることがないよう、無いものはエラーにする。
    for p in &config.exclude {
        if files.remove(p).is_none() {
            bail!("除くファイル {p} がありません（{CONFIG_FILE} を直してください）");
        }
    }
    let mut changed = Vec::new();
    for (path, rules) in &config.rules {
        let mut text = text_of(&files, path)?;
        for rule in rules {
            text = apply(&text, rule, &fragments)
                .with_context(|| format!("{path} を書き換えられません"))?;
        }
        if let Some(e) = files.get_mut(path) {
            e.data = text.into_bytes();
        }
        changed.push(path.clone());
    }
    Ok(Public {
        files,
        markers: config.markers,
        excluded: config.exclude,
        changed,
    })
}

fn text_of(files: &Files, path: &str) -> Result<String> {
    let e = files
        .get(path)
        .with_context(|| format!("{path} がありません"))?;
    String::from_utf8(e.data.clone()).with_context(|| format!("{path} が UTF-8 ではありません"))
}

fn parse_config(text: &str) -> Result<Config> {
    let v: Value =
        serde_json::from_str(text).with_context(|| format!("{CONFIG_FILE} を読めません"))?;
    let strings = |key: &str| -> Result<Vec<String>> {
        v[key]
            .as_array()
            .with_context(|| format!("{CONFIG_FILE} に {key} の配列がありません"))?
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_string)
                    .context("文字列ではありません")
            })
            .collect()
    };
    let field = |r: &Value, key: &str| -> Result<String> {
        r[key]
            .as_str()
            .map(str::to_string)
            .with_context(|| format!("{CONFIG_FILE} の規則に {key} がありません: {r}"))
    };
    let mut rules = Vec::new();
    let files = v["rules"]
        .as_object()
        .with_context(|| format!("{CONFIG_FILE} に rules がありません"))?;
    for (path, list) in files {
        let list = list.as_array().context("rules の値は配列にしてください")?;
        let mut parsed = Vec::new();
        for r in list {
            parsed.push(if r.get("replace").is_some() {
                Rule::Replace {
                    old: field(r, "replace")?,
                    new: field(r, "with")?,
                }
            } else if r.get("from_heading").is_some() {
                Rule::ReplaceFrom {
                    heading: field(r, "from_heading")?,
                    fragment: field(r, "fragment")?,
                }
            } else if r.get("drop_column").is_some() {
                Rule::DropColumn {
                    name: field(r, "drop_column")?,
                }
            } else {
                bail!("{CONFIG_FILE} の規則が分かりません: {r}");
            });
        }
        rules.push((path.clone(), parsed));
    }
    Ok(Config {
        exclude: strings("exclude")?,
        markers: strings("markers")?,
        rules,
    })
}

fn apply(text: &str, rule: &Rule, fragments: &BTreeMap<String, String>) -> Result<String> {
    match rule {
        Rule::Replace { old, new } => {
            let n = text.matches(old.as_str()).count();
            if n != 1 {
                bail!("「{old}」が {n} か所あります（1 か所だけのはず）");
            }
            Ok(text.replacen(old.as_str(), new, 1))
        }
        Rule::ReplaceFrom { heading, fragment } => {
            let start = if text.starts_with(heading.as_str()) {
                0
            } else {
                text.find(&format!("\n{heading}"))
                    .map(|i| i + 1)
                    .with_context(|| format!("「{heading}」で始まる行がありません"))?
            };
            let body = fragments
                .get(fragment)
                .with_context(|| format!("{CONFIG_DIR}{fragment} がありません"))?;
            Ok(format!("{}{body}", &text[..start]))
        }
        Rule::DropColumn { name } => drop_column(text, name),
    }
}

/// Markdown の表の行を、外側の `|` を除いて区切る。
fn cells(line: &str) -> Option<Vec<&str>> {
    let inner = line.trim_end().strip_prefix('|')?.strip_suffix('|')?;
    Some(inner.split('|').collect())
}

fn drop_column(text: &str, name: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    // 今の表で除く列（番号）と列の数。
    let mut table: Option<Option<(usize, usize)>> = None;
    let mut found = false;
    for (no, line) in text.split_inclusive('\n').enumerate() {
        let Some(row) = cells(line) else {
            table = None;
            out.push_str(line);
            continue;
        };
        let target = match table {
            Some(t) => t,
            None => {
                let t = row
                    .iter()
                    .position(|c| c.trim() == name)
                    .map(|i| (i, row.len()));
                table = Some(t);
                t
            }
        };
        let Some((col, len)) = target else {
            out.push_str(line);
            continue;
        };
        if row.len() != len {
            bail!(
                "{} 行目の表の列の数が見出しと違います（{} 列、見出しは {len} 列）",
                no + 1,
                row.len()
            );
        }
        found = true;
        let kept: Vec<&str> = row
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != col)
            .map(|(_, c)| *c)
            .collect();
        out.push('|');
        out.push_str(&kept.join("|"));
        out.push('|');
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    if !found {
        bail!("「{name}」の列がある表がありません");
    }
    Ok(out)
}

/// 公開してはいけない記述を探す。見つかったものを「パス:行: 内容」で返す。
fn check(files: &Files, markers: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    for (path, e) in files {
        let data = &e.data;
        let line_of = |pos: usize| data[..pos].iter().filter(|&&b| b == b'\n').count() + 1;
        for m in markers {
            for pos in find_all(data, m.as_bytes()) {
                problems.push(format!("{path}:{}: 内部向けの記述「{m}」", line_of(pos)));
            }
        }
        // 例として書く `C:\Users\<名前>` と `C:\Users\…` は許す
        // （このファイル自体が掛からないよう、探す文字列は組み立てる）。
        let users = ["C:", "Users", ""].join("\\");
        for pos in find_all(data, users.as_bytes()) {
            let rest = &data[pos + users.len()..];
            if !rest.starts_with(b"<") && !rest.starts_with("…".as_bytes()) {
                problems.push(format!(
                    "{path}:{}: 個人のフォルダのパス（C:\\Users\\…）",
                    line_of(pos)
                ));
            }
        }
        let Ok(text) = std::str::from_utf8(data) else {
            continue;
        };
        for (pos, addr) in emails(text) {
            problems.push(format!("{path}:{}: メールアドレス {addr}", line_of(pos)));
        }
        if path.ends_with(".md") {
            for (pos, target) in broken_links(path, text, files) {
                problems.push(format!(
                    "{path}:{}: リンク先 {target} がありません",
                    line_of(pos)
                ));
            }
        }
    }
    problems
}

fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|&i| haystack[i..].starts_with(needle))
        .collect()
}

/// 例やツールのためのアドレスは許す。
const ALLOWED_EMAIL_DOMAINS: &[&str] = &[
    "anthropic.com",
    "users.noreply.github.com",
    "example.com",
    "example.org",
];

fn emails(text: &str) -> Vec<(usize, String)> {
    let local = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    let domain = |c: char| c.is_ascii_alphanumeric() || ".-".contains(c);
    let mut found = Vec::new();
    for (at, _) in text.match_indices('@') {
        let start = text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| local(*c))
            .last()
            .map_or(at, |(i, _)| i);
        let end = text[at + 1..]
            .char_indices()
            .find(|(_, c)| !domain(*c))
            .map_or(text.len(), |(i, _)| at + 1 + i);
        let host = text[at + 1..end].trim_end_matches('.');
        let tld_ok = host
            .rsplit_once('.')
            .is_some_and(|(_, tld)| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()));
        if start == at || !tld_ok {
            continue;
        }
        let allowed = ALLOWED_EMAIL_DOMAINS
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")));
        if !allowed {
            found.push((start, text[start..at + 1 + host.len()].to_string()));
        }
    }
    found
}

/// Markdown の相対リンクのうち、リンク先のファイルが無いもの。
fn broken_links(path: &str, text: &str, files: &Files) -> Vec<(usize, String)> {
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    let mut broken = Vec::new();
    for (i, _) in text.match_indices("](") {
        let rest = &text[i + 2..];
        let Some(end) = rest.find(')') else { continue };
        let target = &rest[..end];
        if target.is_empty()
            || target.starts_with('#')
            || target.contains("://")
            || target.starts_with("mailto:")
        {
            continue;
        }
        let file = target.split('#').next().unwrap_or_default();
        let mut parts: Vec<&str> = if dir.is_empty() {
            Vec::new()
        } else {
            dir.split('/').collect()
        };
        for seg in file.split('/') {
            match seg {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        let resolved = parts.join("/");
        let is_dir = files.keys().any(|k| k.starts_with(&format!("{resolved}/")));
        if !files.contains_key(&resolved) && !is_dir {
            broken.push((i, target.to_string()));
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> Files {
        list.iter()
            .map(|(p, t)| {
                (
                    p.to_string(),
                    Entry {
                        data: t.as_bytes().to_vec(),
                        executable: false,
                    },
                )
            })
            .collect()
    }

    const CONFIG: &str = r###"{
        "exclude": ["docs/plan.md"],
        "markers": ["INTERNAL"],
        "rules": {
            "docs/guide.md": [
                {"replace": "[計画](plan.md)、", "with": ""},
                {"from_heading": "## 5. ", "fragment": "guide-tail.md"}
            ],
            "docs/tests.md": [{"drop_column": "段階"}]
        }
    }"###;

    fn sample() -> Files {
        files(&[
            ("public/public.json", CONFIG),
            ("public/guide-tail.md", "## 5. 運用\n\n公開用。\n"),
            ("docs/plan.md", "INTERNAL\n"),
            ("docs/guide.md", "# ガイド\n\n関連：[計画](plan.md)、[テスト](tests.md)\n\n## 1. 環境\n\n本文\n\n## 5. 運用（INTERNAL）\n\n内部の話\n"),
            (
                "docs/tests.md",
                "# テスト\n\n| ID | 段階 | 手順 |\n|---|---|---|\n| A-01 | M1 | 起動する |\n\n| 実施日 | 結果 |\n|---|---|\n| 1/1 | OK |\n",
            ),
            ("README.md", "[ガイド](docs/guide.md)\n"),
        ])
    }

    #[test]
    fn builds_public_files() {
        let p = build(sample()).unwrap();
        assert_eq!(
            p.files.keys().collect::<Vec<_>>(),
            ["README.md", "docs/guide.md", "docs/tests.md"]
        );
        assert_eq!(
            text_of(&p.files, "docs/guide.md").unwrap(),
            "# ガイド\n\n関連：[テスト](tests.md)\n\n## 1. 環境\n\n本文\n\n## 5. 運用\n\n公開用。\n"
        );
        // 「段階」のある表だけ列を除き、ほかの表はそのまま。
        assert_eq!(
            text_of(&p.files, "docs/tests.md").unwrap(),
            "# テスト\n\n| ID | 手順 |\n|---|---|\n| A-01 | 起動する |\n\n| 実施日 | 結果 |\n|---|---|\n| 1/1 | OK |\n"
        );
        assert!(check(&p.files, &p.markers).is_empty());
    }

    #[test]
    fn rules_that_no_longer_match_are_errors() {
        // 置き換える文がなくなった。
        let mut f = sample();
        f.insert(
            "docs/guide.md".into(),
            files(&[("x", "# ガイド\n\n## 5. 運用\n")])["x"].clone(),
        );
        assert!(build(f).is_err());
        // 除くファイルがなくなった。
        let mut f = sample();
        f.remove("docs/plan.md");
        assert!(build(f).is_err());
        // 列の数が見出しと合わない。
        assert!(drop_column("| a | 段階 |\n|---|---|\n| 1 |\n", "段階").is_err());
        assert!(drop_column("| a | b |\n", "段階").is_err());
    }

    #[test]
    fn finds_what_must_not_be_published() {
        let f = files(&[
            ("a.md", "INTERNAL\n例：`C:\\Users\\<名前>`、`C:\\Users\\…\\Temp`\n[x](missing.md) [y](a.md#top) [z](https://example.com/)\n"),
            ("src/b.rs", ""),
        ]);
        // このファイル自体が公開前の確認に掛からないよう、@ は組み立てて入れる。
        let mut f = f;
        f.get_mut("src/b.rs").unwrap().data = format!(
            "// C:\\Users\\someone\\x\nconst A: &str = \"someone{at}mail.test.org\";\n// noreply{at}anthropic.com\n// a{at}b\n",
            at = '@'
        )
        .into_bytes();
        let problems = check(&f, &["INTERNAL".into()]);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems[0].starts_with("a.md:1: 内部向けの記述"));
        assert!(problems[1].starts_with("a.md:3: リンク先 missing.md"));
        assert!(problems[2].starts_with("src/b.rs:1: 個人のフォルダ"));
        assert!(problems[3].contains("someone"));
    }
}
