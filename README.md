# SmartConverter

外部エンコーダ（ffmpeg、QSVEncC、MP4Box など）を、プリセット（`Transcoding.ini`）に書いた順に実行する、
ドラッグ＆ドロップ式の動画変換フロントエンドです。携帯動画変換君の操作感を受け継ぎ、Rust で作り直しました。
Windows と Linux で動きます。

- 携帯動画変換君の `Transcoding.ini`（Shift-JIS）と置換変数を、そのまま使えます。
- ファイルを落とすとすぐに変換が始まり、1 つずつ順に処理します。進捗率・残り時間・予想サイズを表示します。
- 一時ファイルは自動で消えるので、OS ごとの `del` / `rm` の行は要りません。
- AviSynth+ / VapourSynth のスクリプト（`.avs` / `.vpy`）の入力と、拡張子ごとのテンプレート（スケルトン）に対応しています。
- 同じプリセットで変換できる CLI 版（`smartconverter-cli`）が付いています。

> 外部エンコーダ（ffmpeg など）は同梱していないので、別に用意してください。

## 動作環境

| OS | 内容 |
|---|---|
| Windows | Windows 10 / 11（x64） |
| Linux | x86_64。glibc 2.39 以降（Ubuntu 24.04 でビルド）。X11 と Wayland のどちらでも動きます |

日本語の表示には、OS の日本語フォントを使います（Windows：游ゴシック・メイリオなど、Linux：Noto Sans CJK など）。

## 入れ方

### Windows

1. `SmartConverter-<版>-windows-x64.zip` を好きな場所に展開します。
2. `SmartConverter.exe` を起動します。設定（`SmartConverter.ini`）は同じフォルダに保存されます（書き込めない場所なら `%APPDATA%\SmartConverter\`）。
   ツールは SmartConverter と同じフォルダ（または `tools` フォルダ）を PATH より先に探すので、SmartConverter は他人が書き込めないフォルダに置いてください。
3. 使うエンコーダ（ffmpeg など）を PATH に置くか、［ツール］→［オプション］の［ツール］で場所を指定します。

携帯動画変換君を使っていた場合は、初めて起動したとき（または［ファイル］→［本家の設定を取り込む］）に、`3GP_Converter.ini` の設定と `Transcoding.ini` を取り込めます。

### Linux

```sh
tar xf SmartConverter-<版>-linux-x86_64.tar.gz
cd SmartConverter-<版>-linux-x86_64
./SmartConverter            # そのまま起動する
sh install.sh               # または ~/.local に入れる（メニューとアイコンも入る）
sh install.sh --uninstall   # 削除する
```

設定（`SmartConverter.ini`）は `~/.config/smartconverter/` に保存されます。実行ファイルの隣に空の `SmartConverter.ini` を置くと、そこを使います（ポータブル）。

> **前の版の上から `install.sh` で入れ直すとき**
> - `~/.local/bin/Transcoding.ini`（プリセット）は、自分で編集したものかもしれないので上書きしません。
>   中身が違うときは、新しい版のプリセット例を `Transcoding.ini.new` として隣に置くので、使うときは名前を変えてください。
> - v0.9 では設定が `~/.local/bin/SmartConverter.ini` にできていました。このファイルが残っているとポータブルとして扱われ、
>   設定とログが `~/.local/bin/` に置かれます。`~/.config/smartconverter/` に移すか消してください（`install.sh` も案内を出します）。

Wayland ではウィンドウのアイコンが `.desktop` ファイルから決まるので、`install.sh` で入れると、タイトルバーとタスクバーにアイコンが出ます。

> **Wayland ではドラッグ＆ドロップができません。** Wayland のセッションでは、ネイティブの Wayland のウィンドウで開きます。
> ただし、今使っている GUI ライブラリ（egui 0.37 / winit 0.30）が Wayland でのファイルのドロップに対応していないため、
> ファイルは［ファイル］→［ファイルを追加］（Ctrl+O）で追加してください。
> ドロップを使いたいときは `SMARTCONVERTER_X11=1 ./SmartConverter` のように起動すると、XWayland（X11）で開きます
> （`install.sh` で入れた場合は、メニューのアイコンを右クリック→「X11 で起動」でも同じです）。
> 画面の拡大率が 100% 以外だと、XWayland では文字がぼやけることがあります。
トレイは StatusNotifierItem（KDE Plasma など）に対応しています。

## 使い方

1. 上のプリセット欄で、使うプリセットを選びます。
2. 出力先のフォルダを選びます。
3. 変換するファイルをウィンドウに落とします（［ファイル］→［ファイルを追加］（Ctrl+O）でも追加できます）。

| 操作 | 内容 |
|---|---|
| ［▶ 変換開始］／［■ 処理中を中断］ | 待機中のジョブを始める／実行中のジョブを止める（途中までの出力ファイルは消します。［設定］→［変換］で残すこともできます） |
| ジョブの右クリック | プリセットの変更、再実行、並べ替え、ログ・出力フォルダ・入力ファイルの場所を開く、一覧から削除 |
| ［ツール］→［プリセット編集］（Ctrl+P） | プリセットの追加・複製・並べ替え・編集。コマンドを置換変数で展開した結果を確かめられます |
| ［ツール］→［オプション］（Ctrl+,） | 優先度、同名ファイルの扱い、タイムスタンプ、スクリプト、ツールの場所、テーマなど |
| ［表示］→［ログ］（Ctrl+L） | 実行したコマンドと外部ツールの出力 |
| ［ヘルプ］→［置換変数の一覧］ | プリセットで使える変数 |

起動するときにファイルを渡すこともできます（`SmartConverter -p 1 動画.ts` で、プリセット `[Item1]` で変換を始めます）。

プリセットの書き方と置換変数の一覧は [docs/preset-guide.md](docs/preset-guide.md) にあります。
`presets/` にプリセット例（携帯動画変換君の既定の設定を、今の ffmpeg で書き直したもの。`Transcoding.ini` と同じ）、`templates/` にスクリプトのテンプレート例があります。

### CLI

```sh
smartconverter-cli -i 動画.ts -p 001 -o 出力先フォルダ   # [Item1] のプリセットで変換
smartconverter-cli -p 1 *.ts                             # 複数のファイル（-o を省くと GUI と同じ出力先）
smartconverter-cli -S "プリセットのタイトル" 動画.ts      # タイトルでプリセットを選ぶ
smartconverter-cli --list-presets                        # プリセットの一覧
smartconverter-cli --dry-run -p 1 動画.ts                # 実行するコマンドを表示するだけ
```

| 終了コード | 意味 |
|---|---|
| 0 | すべて成功 |
| 1 | 失敗したファイルがある |
| 2 | 引数・設定・プリセットの誤り |
| 130 | Ctrl+C で中断 |

ほかのオプションは `smartconverter-cli -h` で確かめてください。

## ビルド

Rust 1.99 以降が必要です。

```sh
cargo build --release      # target/release に SmartConverter と smartconverter-cli
cargo test
cargo xtask dist           # dist/ に配布物（Windows は zip、Linux は tar.gz）
```

Linux では、`libxkbcommon`・`wayland`・`libxcb` の開発用パッケージが要ります。
開発環境と運用ルールは [docs/development-guide.md](docs/development-guide.md) を参照してください。

| パス | 内容 |
|---|---|
| `crates/sc-core` | 中核ライブラリ（INI、置換変数、コマンド実行、進捗解析、スクリプト） |
| `crates/sc-gui` | GUI（実行ファイル名 `SmartConverter`） |
| `crates/sc-cli` | CLI（実行ファイル名 `smartconverter-cli`） |
| `xtask` | 補助タスク（`cargo xtask icons`、`cargo xtask dist`） |
| `assets` | アイコン、Windows のマニフェスト、Linux の `.desktop` とインストール用スクリプト |
| `presets`、`templates` | プリセット例、スクリプトのテンプレート例 |
| `docs` | 仕様書（要件定義・基本設計・画面設計）、受け入れテスト、開発ガイド、プリセットの書き方 |

## ライセンス

MIT License と Apache License 2.0 のデュアルライセンスです（[LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)）。
配布物には、使っているクレートのライセンス文（`THIRD-PARTY-LICENSES.txt`）を同梱しています。
