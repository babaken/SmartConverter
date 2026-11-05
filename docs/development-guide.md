# SmartConverter 開発ガイド

| 項目 | 内容 |
|---|---|
| 文書バージョン | 1.0 |
| 作成日 | 2026-11-02 |
| 関連文書 | [基本設計書](basic-design.md)、[受け入れテスト項目](acceptance-tests.md) |

---

## 1. 開発環境

### 1.1 Windows（主な開発環境）

| 項目 | 内容 |
|---|---|
| OS | Windows 11 x64 |
| Rust | `rustup` の stable、`x86_64-pc-windows-msvc`（既定） |
| C++ ビルドツール | Visual Studio Build Tools（「C++ によるデスクトップ開発」）。MSVC のリンカに必要 |
| 外部ツール（動作確認用） | ffmpeg / ffprobe、QSVEncC、MP4Box、opusenc（qaac64 は使わない）、AviSynth+、VapourSynth（スクリプト機能の確認に必要） |

- `rust-toolchain.toml` は置かない。置くと、既定ホストが GNU 版の環境（rustup の Default host が `x86_64-pc-windows-gnu`）では GNU 版が選ばれ、`dlltool` などが無くてビルドできないため。Windows では `rustup default stable-x86_64-pc-windows-msvc` にしておく。
- 最低対応バージョン（MSRV）は 開発を始めた時点の stable（1.99）とし、`Cargo.toml` の `rust-version` に書く。
- アイコンの生成（`cargo xtask icons`）は Rust のクレート（resvg、ico）だけで行い、外部ツールは要らない。生成した PNG / ICO はリポジトリに入れるので、通常のビルドでは xtask を実行しなくてよい。

### 1.2 Linux

**検証用 PC は EndeavourOS（Arch 系）＋ KDE Plasma（Wayland セッション）。** 受け入れテストの「L」はこの環境で行う。
CI は GitHub Actions の Ubuntu 24.04 でビルドとテストだけを確認する（GUI の見た目や操作は確認しない）。

```sh
# ビルドに必要なもの
sudo pacman -S --needed base-devel rustup git libxkbcommon wayland libxcb fontconfig xdg-desktop-portal-kde
rustup default stable

# 動作確認用の外部ツール
sudo pacman -S --needed ffmpeg gpac opus-tools vapoursynth noto-fonts-cjk
# fdkaac・QSVEncC・AviSynth+・L-SMASH Works は AUR（例：yay -S fdkaac qsvencc avisynthplus vapoursynth-plugin-lsmashsource）

# ビルドして ~/.local に入れる（.desktop とアイコンも入るので、Wayland でアイコンが出る）
cargo build --release
sh assets/linux/install-local.sh          # 外すときは --uninstall
SmartConverter                            # または KDE のメニューから起動
```

- トレイ（cargo feature `tray`、既定で有効）は `ksni` を使い、D-Bus で直接話すので GTK・AppIndicator の開発パッケージは要らない。
  トレイなしでビルドするときは `cargo build -p sc-gui --no-default-features`。
- ファイル選択ダイアログ（`rfd`）は XDG デスクトップポータルを使う。KDE では `xdg-desktop-portal-kde` が入っていれば出る。
- 日本語フォントは `noto-fonts-cjk` を入れる。入れない状態でも、同梱フォントで表示されることを一度確認する。
- QSVEncC を Linux で使うには Intel GPU と、`intel-media-driver`・`onevpl-intel-gpu` が必要。

#### KDE Plasma（Wayland）での注意

| 項目 | 内容 |
|---|---|
| ウィンドウの位置 | Wayland では、アプリがウィンドウの位置を決めたり取得したりできない。位置は保存・復元せず、大きさだけを復元する（基本設計 §9.1） |
| ウィンドウのアイコン | Wayland では、アプリから渡したアイコンは使われない。`app_id`（`smartconverter`）と同じ名前の `.desktop` ファイルとアイコンをインストールすると、タイトルバーとタスクバーに表示される。開発中は `~/.local/share/applications/` と `~/.local/share/icons/hicolor/` に置いて確認する |
| トレイ | KDE は StatusNotifierItem に標準で対応しているので、そのまま使える。Wayland ではアプリがウィンドウを隠せないため、最小化してもトレイには格納せず、タスクバーに残る（トレイのメニューは使える） |
| 通知 | freedesktop 通知が使える |
| X11 での確認 | ログイン画面で「Plasma (X11)」を選べば X11 でも確認できる。主な確認は Wayland で行う |

#### glibc のバージョン

Arch は常に最新の glibc を使う。**Arch でビルドした実行ファイルは、Ubuntu などの古い glibc の環境では動かないことがある。**
配布用のビルドは CI（Ubuntu 24.04 か、それより古い環境）で行い、検証用 PC ではその配布物を動かして確認する。
検証用 PC でビルドしたものは、その PC での開発・確認だけに使う。

## 2. よく使うコマンド

```sh
cargo build                          # 全体のビルド
cargo run -p sc-gui                  # GUI を起動
cargo run -p sc-cli -- --list-presets --ini <path>
cargo test                           # 全テスト
cargo fmt --all                      # 整形
cargo clippy --all-targets -- -D warnings
cargo xtask icons                    # SVG からアイコン（PNG / ICO）を作る
cargo xtask dist                     # 配布物（Windows は zip、Linux は tar.gz）を dist/ に作る
cargo xtask dist --skip-build        # ビルドせずに、今の target/release から作る
```

コミット前に `cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test` が通ることを確認する。

Windows で Linux 向けのコードを確かめるときは、Linux 用の標準ライブラリを入れて clippy だけを動かす（リンクはしないので、Linux の環境が無くてもよい）。

```powershell
rustup target add x86_64-unknown-linux-gnu
cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
cargo clippy -p sc-gui --no-default-features --target x86_64-unknown-linux-gnu -- -D warnings   # トレイなし
```

### 2.1 GUI の動作確認

- GUI は実行ファイルと同じフォルダの `SmartConverter.ini` を読む。`cargo run -p sc-gui` のときは `target/debug/SmartConverter.ini`。
- デバッグビルドでは、環境変数 `SC_DEBUG_OPEN` に `log`・`options`・`about`・`presets` を書くと、その画面を開いた状態で起動する
  （キー入力を送れない環境で、サブ画面の表示を確かめるため。リリースビルドでは無効）。

```powershell
$env:SC_DEBUG_OPEN = "log,options"; cargo run -p sc-gui -- -p 6 sample.ts
```

### 2.2 GUI の操作テスト（egui_kittest）

- 画面の操作は `egui_kittest` で自動テストにする。画面を表示せずにボタンを押し、文字を入力して結果を確かめられる
  （キー入力を送れない環境や CI でも動く）。例：`crates/sc-gui/src/log_window.rs`、`options.rs` の `ui_tests`。
- 部品はアクセシビリティの名前（ボタンの文字やラベル）で探す。ハーネスは **名前のない部品があるとテストを失敗にする** ので、
  入力欄・コンボボックス・スライダーには、横のラベルを `labelled_by` で結び付ける。
- 日本語フォントのない環境（CI の Linux）でも動くよう、ハーネスは `allow_missing_glyphs()` で作る。
- クリップボードへのコピーは、`harness.output().platform_output.commands` の `CopyText` で確かめる。
- トレイ・OS の通知・ファイル選択ダイアログ・実際のドラッグ＆ドロップは対象外（手動で確認する）。

## 3. コーディング規約

| 項目 | 規約 |
|---|---|
| エディション | Rust 2021 |
| 整形 | `rustfmt` の既定設定 |
| 警告 | `clippy` の警告をエラーとして扱う（CI でも同じ） |
| 名前 | 識別子は英語。モジュール構成は [基本設計書](basic-design.md) §2.3 に従う |
| コメント | 日本語でよい。「なぜそうしたか」を書く（本家互換のための挙動には、[本家動作調査記録](original-behavior.md) の該当箇所を書く） |
| エラー処理 | `sc-core` では `unwrap()` / `expect()` を使わない（テストを除く）。`thiserror` の型で返す |
| OS 依存 | `#[cfg(windows)]` / `#[cfg(unix)]` は `platform` モジュールの中だけに書く |
| UI の文字列 | 画面に出す文字列は 1 か所（`sc-gui` の `i18n` モジュール）にまとめる |
| テスト | 新しい処理には単体テストを付ける。外部ツールを使うテストは `#[ignore]` を付け、手動で `cargo test -- --ignored` で実行する |
| 個人情報 | コード・テスト・テストデータに、個人のパス（`C:\Users\<名前>` など）や、個人の録画ファイル名を入れない |

## 4. テストデータ（外部ツールの出力サンプル）

進捗解析のテスト用に、各ツールの標準エラー出力を記録する。

```sh
ffmpeg -y -i sample.ts -c:v libx264 -an video.mp4 > ffmpeg_video.txt 2>&1
QSVEncC --avhw -i sample.ts -c hevc -o out.hevc  > qsvencc.txt 2>&1
```

- 置き場所は `crates/sc-core/tests/fixtures/progress/`。記録したコマンドとツールのバージョンは、同じフォルダの `README.md` に書く。
- 入力は相対パス（`sample.ts`）で渡し、出力に個人のパスが入らないようにする。入ってしまったら一般的なパスに書き換えてから保存する。
- 短い動画だと進捗の途中経過が記録されないので、5 分程度の動画を使う。
- テスト用の動画（`sample.ts` など）は、自分で撮影・生成したもの（ffmpeg の `testsrc` など）を使い、録画番組は使わない。

## 5. ブランチとリリース

このリポジトリには、リリースした版だけを置いています（リリースごとに 1 コミット）。
Issue・PR は受け付けていますが、PR はこのリポジトリで直接マージせず、内容を開発用の環境に取り込んで、次のリリースで反映します。
以下は、開発用の環境での進め方です。

### 5.1 ブランチ

```
main ◀── feat/<英語>   新機能（例 feat/preset-editor）
     ◀── fix/<英語>    不具合修正（例 fix/sjis-detect）
     ◀── docs/<英語>   文書のみの変更
```

- `main` から作り、`main` へ **squash マージ** で 1 コミットにして戻す。
- `main` は常にビルドとテストが通る状態に保つ。

### 5.2 コミットメッセージ

```
<領域>: <何をしたか（日本語）>

<必要なら理由や補足>
```

- 領域：`core`、`gui`、`cli`、`xtask`、`docs`、`ci` など（例 `core: Shift-JIS の INI を読み込めるようにする`）。

### 5.3 リリース

1. ワークスペースの `Cargo.toml` の `version` を上げる（`sc-cli`・`sc-gui` の `sc-core` の依存の `version` も合わせる）。
2. [受け入れテスト](acceptance-tests.md) を両 OS で実施する。
3. `main` に `v<版>` のタグを付けて push する。
4. `release.yml` が両 OS の配布物を作り、リリースの下書き（pre-release）を作る。配布物を確かめてから、GitHub の画面で公開する。

配布物だけを確かめたいときは、Actions の「Release」を手動実行すると、成果物（artifact）として残る（配布物の受け取りと SHA-256 の計算まで行い、下書きは作らない）。

ワークフローのアクションは Node.js 24 で動く版を使う（`actions/checkout@v7`、`actions/upload-artifact@v7`、`actions/download-artifact@v8`、`Swatinem/rust-cache@v2`）。アクションを上げるときは、リリースノートで Node.js の版と互換性のない変更を確かめる。
