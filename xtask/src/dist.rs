//! `cargo xtask dist`：配布物を作る（基本設計 §12）。
//!
//! リリースビルドをして、`dist/SmartConverter-<版>-<OS>-<CPU>/` に必要なファイルを集め、
//! Windows では zip、Linux では tar.gz にまとめる。
//! 依存クレートのライセンス文は `THIRD-PARTY-LICENSES.txt` にまとめて同梱する。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::workspace_root;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 配布物に入れる実行ファイルのパッケージ。
const PACKAGES: &[&str] = &["sc-gui", "sc-cli"];

struct Target {
    /// アーカイブ名に入れる名前（例 `windows-x64`）
    label: &'static str,
    exe_suffix: &'static str,
    /// 同梱するプリセット例（`Transcoding.ini` としても置く）
    preset: &'static str,
    zip: bool,
}

fn target() -> Result<Target> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok(Target {
            label: "windows-x64",
            exe_suffix: ".exe",
            preset: "3gpp_aac.ini",
            zip: true,
        }),
        ("linux", "x86_64") => Ok(Target {
            label: "linux-x86_64",
            exe_suffix: "",
            preset: "3gpp_aac.ini",
            zip: false,
        }),
        (os, arch) => bail!("この環境（{os} / {arch}）の配布物には対応していません"),
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let mut skip_build = false;
    for a in args {
        match a.as_str() {
            "--skip-build" => skip_build = true,
            _ => bail!("dist の引数が正しくありません: {a}（使えるのは --skip-build）"),
        }
    }
    let t = target()?;
    let root = workspace_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    if !skip_build {
        let mut cmd = Command::new(&cargo);
        cmd.current_dir(&root)
            .args(["build", "--release", "--locked"]);
        for p in PACKAGES {
            cmd.args(["-p", p]);
        }
        let status = cmd.status().context("cargo build を起動できません")?;
        if !status.success() {
            bail!("cargo build --release が失敗しました");
        }
    }

    let name = format!("SmartConverter-{VERSION}-{}", t.label);
    let dist = root.join("dist");
    let stage = dist.join(&name);
    if stage.exists() {
        fs::remove_dir_all(&stage).with_context(|| format!("{} を消せません", stage.display()))?;
    }
    fs::create_dir_all(&stage)?;

    let release = root.join("target").join("release");
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut executables = BTreeSet::new();
    for bin in ["SmartConverter", "smartconverter-cli"] {
        let file = format!("{bin}{}", t.exe_suffix);
        files.push((release.join(&file), file.clone()));
        executables.insert(file);
    }
    for f in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"] {
        files.push((root.join(f), f.into()));
    }
    files.push((
        root.join("docs/preset-guide.md"),
        "docs/preset-guide.md".into(),
    ));
    let preset = root.join("presets").join(t.preset);
    files.push((preset.clone(), format!("presets/{}", t.preset)));
    files.push((preset, "Transcoding.ini".into()));
    for f in ["default.avs", "default.vpy"] {
        files.push((root.join("templates").join(f), format!("templates/{f}")));
    }
    if !t.zip {
        let linux = root.join("assets/linux");
        files.push((linux.join("install-local.sh"), "install.sh".into()));
        executables.insert("install.sh".into());
        files.push((
            linux.join("smartconverter.desktop"),
            "share/applications/smartconverter.desktop".into(),
        ));
        for s in [16, 24, 32, 48, 64, 128, 256, 512] {
            files.push((
                root.join(format!("assets/icon/png/smartconverter-{s}.png")),
                format!("share/icons/hicolor/{s}x{s}/apps/smartconverter.png"),
            ));
        }
    }

    for (src, rel) in &files {
        let dst = stage.join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, &dst).with_context(|| format!("{} をコピーできません", src.display()))?;
    }
    let licenses = third_party_licenses(&root, &cargo)?;
    fs::write(stage.join("THIRD-PARTY-LICENSES.txt"), licenses)?;
    println!("集めました: {}", stage.display());

    let archive = if t.zip {
        let path = dist.join(format!("{name}.zip"));
        write_zip(&stage, &name, &path, &executables)?;
        path
    } else {
        let path = dist.join(format!("{name}.tar.gz"));
        write_tar_gz(&stage, &name, &path, &executables)?;
        path
    };
    println!("書き出し: {}", archive.display());
    Ok(())
}

/// `stage` の中のファイルを、名前順に `(相対パス, 絶対パス)` で返す。区切りは `/`。
fn walk(stage: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut dirs = vec![stage.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let rel = path
                    .strip_prefix(stage)?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((rel, path));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn write_zip(stage: &Path, top: &str, out: &Path, exe: &BTreeSet<String>) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let file = fs::File::create(out).with_context(|| format!("{} を作れません", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    for (rel, path) in walk(stage)? {
        let mode = if exe.contains(&rel) { 0o755 } else { 0o644 };
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(mode)
            .last_modified_time(zip_time(mtime(&path)?));
        zip.start_file(format!("{top}/{rel}"), opts)?;
        zip.write_all(&fs::read(&path)?)?;
    }
    zip.finish()?;
    Ok(())
}

/// 更新日時（UNIX 時間の秒）。
fn mtime(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0))
}

/// UNIX 時間（UTC）を zip の日時にする。
fn zip_time(secs: u64) -> zip::DateTime {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // 1970-01-01 からの日数を年月日にする（Howard Hinnant の civil_from_days）。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
    zip::DateTime::from_date_and_time(
        year,
        month,
        day,
        (rem / 3600) as u8,
        (rem / 60 % 60) as u8,
        (rem % 60) as u8,
    )
    .unwrap_or_default()
}

fn write_tar_gz(stage: &Path, top: &str, out: &Path, exe: &BTreeSet<String>) -> Result<()> {
    let file = fs::File::create(out).with_context(|| format!("{} を作れません", out.display()))?;
    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    for (rel, path) in walk(stage)? {
        let data = fs::read(&path)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(if exe.contains(&rel) { 0o755 } else { 0o644 });
        header.set_mtime(mtime(&path)?);
        header.set_cksum();
        tar.append_data(&mut header, format!("{top}/{rel}"), data.as_slice())?;
    }
    tar.into_inner()?.finish()?;
    Ok(())
}

/// 配布する実行ファイルが使う（通常の依存の）クレートのライセンス文をまとめる。
fn third_party_licenses(root: &Path, cargo: &std::ffi::OsStr) -> Result<String> {
    let host = host_triple()?;
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked"])
        .args(["--filter-platform", &host])
        .output()
        .context("cargo metadata を起動できません")?;
    if !output.status.success() {
        bail!(
            "cargo metadata が失敗しました: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let meta: Value = serde_json::from_slice(&output.stdout)?;

    let packages: BTreeMap<&str, &Value> = meta["packages"]
        .as_array()
        .context("packages がありません")?
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let nodes: BTreeMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .context("resolve がありません")?
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();

    // sc-gui・sc-cli から、通常の依存（ビルド用・テスト用を除く）だけをたどる。
    let mut queue: VecDeque<&str> = packages
        .iter()
        .filter(|(_, p)| PACKAGES.contains(&p["name"].as_str().unwrap_or("")))
        .map(|(id, _)| *id)
        .collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["kind"].is_null());
            if normal {
                if let Some(pkg) = dep["pkg"].as_str() {
                    queue.push_back(pkg);
                }
            }
        }
    }

    let mut list: Vec<&Value> = seen
        .iter()
        .filter_map(|id| packages.get(id).copied())
        .filter(|p| !p["source"].is_null()) // このワークスペースのクレートは除く
        .collect();
    list.sort_by_key(|p| {
        (
            p["name"].as_str().unwrap_or("").to_owned(),
            p["version"].as_str().unwrap_or("").to_owned(),
        )
    });

    let mut out = String::new();
    out.push_str(
        "SmartConverter が使っているサードパーティーのクレートと、そのライセンスです。\n\
         This file lists the third-party crates used by SmartConverter and their licenses.\n\n",
    );
    for p in &list {
        out.push_str(&format!(
            "- {} {}  ({})\n",
            p["name"].as_str().unwrap_or("?"),
            p["version"].as_str().unwrap_or("?"),
            p["license"].as_str().unwrap_or("ライセンスの記載なし"),
        ));
    }
    // 同じ文（Apache-2.0 の全文など）は 1 回だけ載せ、使っているクレートを並べる。
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing = Vec::new();
    for p in &list {
        let name = p["name"].as_str().unwrap_or("?");
        let version = p["version"].as_str().unwrap_or("?");
        let dir = Path::new(p["manifest_path"].as_str().unwrap_or(""))
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let mut texts = license_files(&dir, p["license_file"].as_str());
        if texts.is_empty() {
            // ライセンス文を同梱していないクレートは、作者を入れた標準のライセンス文を載せる。
            texts = standard_license_texts(root, p);
            if texts.is_empty() {
                missing.push(format!("{name} {version}"));
                continue;
            }
        }
        for (file, text) in texts {
            let text = text.replace("\r\n", "\n").trim().to_owned();
            let label = format!("{name} {version} — {file}");
            match index.get(&text) {
                Some(&i) => groups[i].1.push(label),
                None => {
                    index.insert(text.clone(), groups.len());
                    groups.push((text, vec![label]));
                }
            }
        }
    }
    for (text, labels) in &groups {
        out.push_str(&format!("\n{}\n", "=".repeat(78)));
        for l in labels {
            out.push_str(l);
            out.push('\n');
        }
        out.push_str(&format!("{}\n\n{text}\n", "=".repeat(78)));
    }
    if !missing.is_empty() {
        eprintln!(
            "警告: ライセンスファイルが見つからないクレート（一覧には載せています）: {}",
            missing.join(", ")
        );
    }
    Ok(out)
}

/// クレートのフォルダにあるライセンス関係のファイルを読む。
///
/// 直下の `LICENSE*` などに加えて、`LICENSES/` の中のファイルと、
/// 同梱フォントのライセンス（`fonts/*.txt`、epaint_default_fonts）も読む。
fn license_files(dir: &Path, license_file: Option<&str>) -> Vec<(String, String)> {
    let mut names = BTreeSet::new();
    if let Some(f) = license_file {
        names.insert(f.to_owned());
    }
    let list = |sub: &str, pred: &dyn Fn(&str) -> bool, names: &mut BTreeSet<String>| {
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            return;
        };
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.path().is_file() && pred(&n) {
                names.insert(if sub.is_empty() {
                    n
                } else {
                    format!("{sub}/{n}")
                });
            }
        }
    };
    let is_license = |n: &str| {
        let upper = n.to_ascii_uppercase();
        [
            "LICENSE",
            "LICENCE",
            "COPYING",
            "NOTICE",
            "COPYRIGHT",
            "UNLICENSE",
        ]
        .iter()
        .any(|p| upper.starts_with(p))
    };
    list("", &is_license, &mut names);
    list("LICENSES", &|_| true, &mut names);
    list(
        "fonts",
        &|n| n.to_ascii_lowercase().ends_with(".txt"),
        &mut names,
    );
    names
        .into_iter()
        .filter_map(|n| {
            let text = fs::read(dir.join(&n)).ok()?;
            Some((n, String::from_utf8_lossy(&text).into_owned()))
        })
        .collect()
}

/// ライセンス文を同梱していないクレートのために、`license` に書かれた MIT / Apache-2.0 の
/// 標準の文を返す（MIT の著作権表示には Cargo.toml の `authors` を使う）。
fn standard_license_texts(root: &Path, p: &Value) -> Vec<(String, String)> {
    let license = p["license"].as_str().unwrap_or("");
    let ids: BTreeSet<&str> = license
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = Vec::new();
    if ids.contains("MIT") {
        let authors: Vec<&str> = p["authors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let holder = if authors.is_empty() {
            format!("the {} authors", p["name"].as_str().unwrap_or("?"))
        } else {
            authors.join(", ")
        };
        out.push((
            "MIT（標準の文。クレートに同梱されていないため）".into(),
            MIT_TEMPLATE.replace("{holder}", &holder),
        ));
    }
    if ids.contains("BSL-1.0") {
        out.push((
            "BSL-1.0（標準の文。クレートに同梱されていないため）".into(),
            BSL_TEXT.into(),
        ));
    }
    if ids.contains("Apache-2.0") {
        if let Ok(text) = fs::read_to_string(root.join("LICENSE-APACHE")) {
            out.push((
                "Apache-2.0（標準の文。クレートに同梱されていないため）".into(),
                text,
            ));
        }
    }
    out
}

const MIT_TEMPLATE: &str = "Copyright (c) {holder}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
";

const BSL_TEXT: &str = "Boost Software License - Version 1.0 - August 17th, 2003

Permission is hereby granted, free of charge, to any person or organization
obtaining a copy of the software and accompanying documentation covered by
this license (the \"Software\") to use, reproduce, display, distribute,
execute, and transmit the Software, and to prepare derivative works of the
Software, and to permit third-parties to whom the Software is furnished to
do so, all subject to the following:

The copyright notices in the Software and this entire statement, including
the above license grant, this restriction and the following disclaimer,
must be included in all copies of the Software, in whole or in part, and
all derivative works of the Software, unless such copies or derivative
works are solely in the form of machine-executable object code generated by
a source language processor.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT
SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE LIABLE
FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
";

fn host_triple() -> Result<String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("rustc を起動できません")?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
        .context("rustc -vV に host がありません")
}
