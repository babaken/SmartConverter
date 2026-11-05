# SmartConverter 基本設計書

| 項目 | 内容 |
|---|---|
| 文書バージョン | 1.0（確定） |
| 作成日 | 2026-11-02 |
| 前提文書 | [要件定義書](requirements.md) v0.6 |
| 対象リリース | v0.9 |

---

## 1. システム概要

SmartConverter は、`Transcoding.ini` に書かれたプリセット（外部コマンドの並び）に従い、
ドロップされた動画ファイルを 1 つずつ変換する GUI / CLI アプリである。
エンコード処理そのものは外部ツール（ffmpeg、QSVEncC、qaac、MP4Box など）が行い、SmartConverter は
「コマンドの組み立て → 順次実行 → 進捗・ログの表示 → 後片付け」を受け持つ。

```
┌──────────── SmartConverter ────────────┐
│  GUI (egui)        CLI                 │
│      │               │                 │
│      └──────┬────────┘                 │
│          sc-core                       │      ┌───────────────┐
│  INI読込 → 変数展開 → コマンド実行 ────┼────▶│ 外部ツール     │
│                       ▲   │            │      │ ffmpeg/QSVEncC │
│              進捗解析 ┘   └ 一時ファイル │◀────│ qaac/MP4Box…  │
│                             削除        │      └───────────────┘
└────────────────────────────────────────┘
```

## 2. 全体構成

### 2.1 Cargo ワークスペース

```
SmartConverter/
├─ Cargo.toml                 … ワークスペース定義
├─ crates/
│  ├─ sc-core/                … GUI に依存しない中核ライブラリ
│  ├─ sc-gui/                 … GUI アプリ（バイナリ名 SmartConverter）
│  └─ sc-cli/                 … CLI アプリ（バイナリ名 smartconverter-cli）
├─ assets/
│  ├─ icon/                   … アイコン（SVG 原本、生成した ICO/PNG）
│  ├─ linux/                  … .desktop と、~/.local に入れるスクリプト（install-local.sh）
│  ├─ windows/                … マニフェスト
│  └─ fonts/                  … 予備の日本語フォント（§9.3）
├─ presets/                   … 同梱のプリセット例（両 OS 共通）
├─ templates/                 … 同梱のスケルトン例（default.avs / default.vpy）
├─ docs/
└─ xtask/                     … アイコン生成・パッケージ作成などの補助タスク
```

### 2.2 バイナリを 2 つに分ける理由

Windows では、GUI アプリ（`windows_subsystem = "windows"`）は標準出力をコンソールに出せない。
そのため GUI と CLI を別のバイナリにする。

| バイナリ | 用途 |
|---|---|
| `SmartConverter(.exe)` | GUI。`-S "タイトル"` / `-p N` / `-o フォルダ` とファイルを受け取ると、GUI を開いてそのファイルをキューに入れ、すぐ変換を始める（本家互換） |
| `smartconverter-cli(.exe)` | CLI。GUI を出さずに変換する（要件 F-CL-01〜12、§2.4） |

### 2.4 CLI の仕様

```
smartconverter-cli [オプション] [-i <入力>]... [<入力>...]

  -i, --input <FILE>        入力ファイル（繰り返し可。オプションの後ろに並べてもよい）
  -p, --preset <N>          [ItemN] の N でプリセットを指定（001 → [Item1]）
  -S, --preset-title <T>    タイトルでプリセットを指定（本家互換）。-p と同時に指定できない
  -o, --output-dir <DIR>    出力先フォルダ（なければ作る）。省略時は GUI で設定した出力先
      --ini <FILE>          使う Transcoding.ini
      --config <FILE>       使う SmartConverter.ini
      --on-exists <MODE>    rename | overwrite | skip
      --priority <0-3>      子プロセスの優先度
      --temp-dir <DIR>      一時フォルダ
      --list-presets        プリセット一覧を表示して終了
      --dry-run             展開後のコマンドを表示するだけで実行しない
      --log <FILE>          ログをファイルに保存
  -q, --quiet               進捗を表示しない
  -v, --verbose             外部ツールの出力もそのまま表示
  -h, --help / -V, --version
```

使用例：

```sh
# [Item1] のプリセットで変換し、/test/001/XXXX.mp4 に出力
smartconverter-cli -i XXXX.ts -p 001 -o /test/001

# 複数ファイル（Windows でもワイルドカードを展開する）
smartconverter-cli -p 5 -o D:\out *.ts

# プリセット一覧
smartconverter-cli --list-presets
  N  タイトル
  0  QVGA標準画質15fpsモノラル
  1  QVGA標準画質15fpsステレオ
  …
```

- 優先順位：コマンドラインの指定 → `SmartConverter.ini`（`--config` または既定の場所）→ 既定値。CLI は GUI と同じ `SmartConverter.ini` を読むので、`-o` などを省略すると GUI で設定した値（出力先フォルダ・優先度・同名ファイルの扱いなど）がそのまま使われる。
- 進捗表示：標準エラー出力にプログレスバーを 2 段で表示する（`indicatif` の `MultiProgress`）。

  ```
  全体 [1/3] ████████░░░░░░░░░░░░░░░░  33%  経過 00:12:40
  XXXX.ts  コマンド 2/4 ffmpeg
       [██████████████▌░░░░░░░░░] 63%  21540/34190 frames  412 fps  経過 03:12  残り 01:52
  ```

  - 上段：全体（何ファイル目か、完了ファイル数の割合）。ファイルが 1 つなら上段は出さない。
  - 下段：実行中のファイルとコマンド、進捗率。フレーム数・速度は、取れるツール（ffmpeg、QSVEncC など）のときだけ出す。
  - 進捗率が分からないツールは、左右に動くバー（スピナー）と経過時間だけを出す。
  - 端末の幅に合わせてバーの長さを変える。日本語のファイル名は表示幅（全角 2 桁）で数えて、長ければ途中を `…` で省略する。
  - Windows では起動時に仮想端末モード（ANSI エスケープ）を有効にする。使えない古いコンソールでは、`
` による 1 行上書き表示に切り替える。
  - 標準エラー出力が端末でない（パイプ・リダイレクト）ときは、バーを出さず、10 秒ごとに `[1/3] XXXX.ts コマンド 2/4 ffmpeg 63% 残り 01:52` を 1 行で出す。
  - `-v` を付けたときは、外部ツールの出力をバーの上に流し、バーは常に最下段に残す。
- 各ファイルの結果は標準出力に 1 行ずつ出す：`OK  XXXX.ts -> /test/001/XXXX.mp4` / `NG  YYYY.ts (終了コード 1: qaac64)`
- 終了コード：全成功 0／一部失敗 1／設定・引数エラー 2／Ctrl+C で中断 130。
- 処理の中身（キューの進め方、実行、後片付け）は GUI と同じ `sc-core` を使う。

### 2.3 sc-core のモジュール

| モジュール | 役割 |
|---|---|
| `text` | 文字コード判別（UTF-8 BOM / UTF-8 / CP932）、改行の正規化 |
| `ini` | 行を保持する INI パーサ・ライタ（コメントや無効セクションも消さずに書き戻す） |
| `preset` | `Transcoding.ini` の読み書き。`PresetSet` / `Preset` モデル |
| `settings` | `SmartConverter.ini` の読み書き。本家 `3GP_Converter.ini` のインポート |
| `vars` | 置換変数の展開（`<%Name%>`） |
| `cmdline` | コマンド文字列の分解（Windows 規則 / POSIX 規則）と、ツールのパス解決 |
| `script` | `.avs` / `.vpy` スケルトンの生成、AviSynth+ / VapourSynth があるかの検出 |
| `job` | `Job` モデルと状態遷移 |
| `runner` | 1 ジョブ分のコマンド列を実行するワーカー |
| `progress` | 外部ツールの出力から進捗を解析する |
| `naming` | 出力ファイル名の決定（タイムスタンプ、使えない文字の置換、同名ファイルの回避） |
| `platform` | OS 依存処理（優先度、プロセスツリーの終了、コンソール非表示、ルート判定） |
| `error` | エラー型（`thiserror`） |

## 3. データモデル

### 3.1 プリセット

```rust
pub struct PresetSet {
    pub path: PathBuf,
    pub encoding: TextEncoding,      // 読み込み時の文字コード（表示用）
    pub info: PresetInfo,            // [Info] Title / Description
    pub presets: Vec<Preset>,        // [Item0].. の連番（欠番で終わり）
    doc: IniDocument,                // 元の行をそのまま保持（書き戻し用）
}

pub struct Preset {
    pub index: usize,                // N of [ItemN]
    pub title: String,
    pub commands: Vec<String>,       // Command0.. の連番（外側の "…" を外した状態）
}
```

- **行を保持する INI**：`[0Item]` や `[*テンプ or ゴミ]` のような「無効化セクション」、コメント、
  知らないキーは `IniDocument` にそのまま残す。プリセット編集で保存しても消えない。
- 保存時は `[ItemN]` を 0 から振り直し、値は外側を `"…"` で囲んで書く（本家と同じ書式）。
- 保存前に `Transcoding.ini.bak` を作る（直前の 1 世代）。

### 3.2 ジョブ

```rust
pub struct Job {
    pub id: JobId,                   // 連番
    pub input: PathBuf,
    pub preset_title: String,        // タイトルで参照（再読み込み後も追える）
    pub state: JobState,
    pub output_base: Option<PathBuf>,// 実行開始時に確定した <%OutputFile%>
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
    pub progress: JobProgress,
    pub message: Option<String>,     // 失敗理由など
}

pub enum JobState { Pending, Running, Succeeded, Failed, Aborted, PresetMissing }

pub struct JobProgress {
    pub command_index: usize,        // 実行中のコマンドは何番目か
    pub command_count: usize,
    pub command_name: String,        // 例 "QSVEncC"
    pub ratio: Option<f32>,          // 0.0〜1.0（解析できなければ None）
    pub eta: Option<Duration>,
    pub estimated_size: Option<u64>,
}
```

### 3.3 ジョブの状態遷移

```
            ┌──────── 削除 ────────▶ (キューから消える)
            │
 追加 ──▶ Pending ──開始──▶ Running ──全コマンド成功──▶ Succeeded
            ▲  │                │  ├─終了コード≠0 / 起動失敗──▶ Failed
            │  │                │  └─中断──────────────────────▶ Aborted
            │  └─プリセットが消えた──▶ PresetMissing
            │                         │
            └──── 再実行 / プリセット再設定 ─┘（Failed / Aborted / PresetMissing から）
```

- 実行開始時に、その時点のプリセットのコマンド列を **コピーして** 使う。実行中にプリセットを編集しても、実行中のジョブには影響しない。
- `Transcoding.ini` を再読み込みしたとき、待機中のジョブのプリセットタイトルが見つからなければ `PresetMissing` にする。

## 4. 処理フロー

### 4.1 ジョブ 1 件の実行

```
1. 出力先チェック    出力先がドライブのルートなら Failed（F-QU-08）
2. 出力名の決定      naming: 入力ファイル名(拡張子なし) + タイムスタンプ → 同名ファイルの回避
3. 一時ファイル名    <TemporaryFolder>/<名前>/<名前>（名前は sc_<pid>_<jobid>_<乱数12桁>）。
                     実行の直前にジョブ専用のフォルダ <名前> を新しく作り（既にあれば Failed。Linux は 0700）、
                     終了後にフォルダごと消す。共有の /tmp で、ほかのユーザーが置いたファイルや
                     シンボリックリンクに書き込まされないようにするため
4. スクリプト処理    入力の拡張子にスケルトンが割り当てられていれば、.avs/.vpy を一時ファイルとして生成し、
                     <%InputFile%> をそれに差し替える（入力自体が .avs/.vpy ならそのまま使う）。詳細は §4.3
5. コマンド実行      Command0..N について：
                       a. 変数を展開（未定義の変数は文字のまま残し、ログに警告）。Linux では値を引用符の状態に
                          合わせてエスケープする（ファイル名の " や ' で引数を差し込まれないように。§6.1）
                       b. 先頭のツール名をパス解決（[Tools] の上書き → 実行ファイルと同じ場所 → PATH）
                       c. 起動（コンソール非表示、優先度設定、プロセスツリーとして管理）
                       d. stdout/stderr を行単位で読み取り → ログ ＋ 進捗解析
                       e. 終了コード≠0 なら Failed、残りのコマンドは実行しない
6. 後片付け          成功・失敗・中断のどれでも、<一時ファイル名>* を削除
7. 結果              Succeeded / Failed / Aborted をイベントで通知
```

強制終了などで 6 まで進まずに残ったジョブ専用のフォルダは、次に GUI を起動したとき・CLI で変換を始めるときに片付ける
（`naming::remove_stale_temporary_dirs`）。対象は `sc_<pid>_<jobid>_<乱数12桁>` の形のフォルダで、その PID のプロセスが
動いていないもの。シンボリックリンクは対象外、Linux では自分が持ち主のものだけ。OS の一時フォルダはすぐ消し、
設定で指定した一時フォルダ（`TemporaryFolder`）は、ネットワーク上の共有フォルダでほかの PC のジョブを消さないよう、
更新から 24 時間以上たったものだけを消す。

### 4.3 スケルトン（`script` モジュール）

| 項目 | 内容 |
|---|---|
| 割り当て | `[Script]` の `ExtN=拡張子:テンプレート` を先頭から見て、入力の拡張子（大文字小文字を区別しない）に最初に一致したもの |
| テンプレートの場所 | 絶対パスはそのまま。相対パス（本家の `\cores\…` の形も含む）はアプリのフォルダから |
| 種類 | テンプレートの拡張子で決める（`.avs` → AviSynth+、`.vpy` → VapourSynth）。それ以外は警告して使わない |
| 展開 | テンプレート内の置換変数は、元の入力ファイルで展開する（未定義の変数は文字のまま残してログに警告）。コマンド側は `<%InputFile%>` だけが生成したスクリプトに変わり、`<%InputFileBase%>`・`<%Title%>`・出力名などは元の入力のまま |
| 安全のため | テンプレートが使う変数の値に `"` か改行があれば、スクリプトを作らずに Failed にする（文字列が閉じて、ファイル名に書いた命令がスクリプトとして実行されるのを防ぐ。Linux ではファイル名に `"` を使える） |
| 生成するファイル | `<%TemporaryFile%>.avs` / `.vpy`。ほかの一時ファイルと一緒に消える（F-AV-07）。ファイルは実行の直前に作る（`--dry-run` や確認欄では作らない） |
| 文字コード | Windows の `.avs` は Shift-JIS（AviSynth+ がスクリプトを ANSI として読むため。表せない文字があればログに警告）。`.vpy` と Linux の `.avs` は UTF-8 |
| 検出（F-AV-05） | AviSynth+：本体のライブラリ（Windows は System32・アプリのフォルダ・PATH の `AviSynth.dll`、Linux は `LD_LIBRARY_PATH`・`/usr/lib` などの `libavisynth.so`）。VapourSynth：`vspipe`（ツールと同じ探し方）、無ければ `VSScript.dll` / `libvapoursynth-script.so`、それも無ければ（Windows）レジストリ `HKCU` / `HKLM` の `SOFTWARE\VapourSynth` の `VSPipeEXE`・`VSScriptDLL`（pip などでユーザー単位に入れると、PATH に入らない Python のパッケージの中に置かれるため）。ライブラリは読み込まず、ファイルがあるかだけを見る。起動時とオプション画面の［再検出］で調べる |
| 使えないとき | その種類のスケルトンは使わず、元のファイルを入力にしてログに警告する（変換は続ける） |

本家は `cores\avscheck.exe` で AviSynth の有無を調べていたが、この確認方法は引き継がない（[本家の動作](original-behavior.md) §6）。

### 4.2 キューの進め方

- 同時に実行するジョブは 1 つ（F-QU-07）。
- `Running` のジョブが終わったら、キューの上から最初の `Pending` を自動で始める。
- **ドロップしたら自動で開始**（本家互換）：オプション「ファイルを追加したらすぐ変換を始める」（既定 ON）。
  OFF のときは「変換開始」ボタンで始める。
- 「処理中を中断」：実行中のジョブを `Aborted` にする。`AllAbort` が ON なら、残りのキューも止める
  （待機中のジョブは `Pending` のまま、自動開始も止める）。
- 全ジョブが終わったら、通知を出す。`ExitIfFinished` が ON ならアプリを終了する。

## 5. スレッドとイベント

```
┌ GUI スレッド (egui) ─────────────┐        ┌ Runner スレッド ─────────────┐
│ AppState                         │ RunJob │ 1 ジョブ分の実行             │
│  ├ queue: Vec<Job>               │───────▶│  ├ 子プロセス起動             │
│  ├ presets: PresetSet            │ Abort  │  ├ stdout 読み取りスレッド ─┐ │
│  ├ settings                      │───────▶│  ├ stderr 読み取りスレッド ─┤ │
│  └ log: LogBuffer                │        │  └ 進捗解析 ◀───────────────┘ │
│                                  │◀───────│ RunnerEvent                  │
│ 毎フレーム try_recv で反映       │        └──────────────────────────────┘
└──────────────────────────────────┘
```

- `sc-core::runner::run_job` はイベントを **コールバック** で返す（CLI はそのまま表示に使う）。GUI は run_job を Runner スレッドで動かし、
  コールバックの中で標準ライブラリの `mpsc` チャネルに送って、`ctx.request_repaint()` で画面スレッドを起こす。
- キューは GUI スレッドだけが持つ。Runner には「1 ジョブ分のスナップショット」（入力・コマンド列・設定値）を渡す。
  → 排他制御が要らない。

```rust
pub enum RunnerCommand { Run(JobSnapshot), Abort }

pub enum RunnerEvent {
    Started { job: JobId, output_base: PathBuf },
    CommandStarted { job: JobId, index: usize, count: usize, display: String },
    Output { job: JobId, stream: Stream, line: String, replace_last: bool },
    Progress { job: JobId, ratio: Option<f32>, eta: Option<Duration>, size: Option<u64> },
    CommandFinished { job: JobId, index: usize, exit_code: Option<i32> },
    Finished { job: JobId, result: JobResult },
}
```

- **ログの `\r` 対応**：進捗表示のツールは `\r` で同じ行を上書きする。`\r` で終わった行は `replace_last: true` で送り、
  ログ表示では最後の行を置き換える（ログが膨れない）。
- ログバッファはジョブごとに最大 20,000 行（超えたら古い順に捨てる。ファイル保存は捨てずにすべて書く）。

## 6. コマンド実行の詳細

### 6.1 コマンド文字列の解釈

| OS | 方式 |
|---|---|
| Windows | 先頭のトークン（プログラム名）だけを Windows の規則（`CommandLineToArgvW` 相当）で取り出し、パスを解決する。残りの部分は **加工せず** `CommandExt::raw_arg` で渡す（本家の CreateProcess と同じ解釈になり、既存プリセットの引用符がそのまま動く） |
| Linux | `shell-words` で argv に分解して実行する。シェルは経由しない（`|`、`>`、`&&` は使えない。必要ならプリセットに `sh -c "…"` と書く）。置換変数の値は、展開する位置の引用符の状態（なし・`'…'`・`"…"`）に合わせてエスケープし、値が必ず 1 つの引数の一部になるようにする（`vars::expand_posix`） |

### 6.2 ツールのパス解決

1. `SmartConverter.ini` の `[Tools]` にツール名があれば、そのパスを使う（例 `qaac64=C:\tools\qaac\qaac64.exe`）
2. 名前がパスを含む場合（`/`、`\` を含む）はそのまま使う。`<%AppPath%>` は展開済み
3. SmartConverter の実行ファイルと同じディレクトリ、およびその `tools/` サブディレクトリ
4. （Windows のみ）`%SystemRoot%\System32`、`%SystemRoot%`（CreateProcess と同じ。PATH の並びで 32 ビット版の cmd.exe が選ばれないように）
5. 環境変数 `PATH`（Windows は `PATHEXT` も考慮）

アプリのフォルダを PATH より先に探すのは、Windows の CreateProcess や本家と同じ動きで、ツールを同梱する
ポータブルな使い方の前提でもある（この順番は変えない。v1.0 の検討で決定）。そのため、アプリは他人が書き込めない
フォルダに置く（README に記載）。Linux のアプリのフォルダ（`~/.local/bin` など）は本人しか書き込めず、たいてい PATH にも
入っているので、実害はほとんどない。設定ファイルが `~/.config` にあっても、アプリのフォルダを探す意味は変わらない。

見つからなければ、起動前に `Failed`（メッセージ「ツールが見つかりません: qaac64」）。

### 6.3 OS 依存処理（`platform` モジュール）

| 処理 | Windows | Linux |
|---|---|---|
| コンソール非表示 | `CREATE_NO_WINDOW` | 不要 |
| 優先度 0/1/2/3 | `IDLE` / `NORMAL` / `HIGH` / `REALTIME` の `PRIORITY_CLASS` | nice 19 / 0 / -5 / -10（負の値は権限がなければ 0 に丸め、ログに警告） |
| プロセスツリーの管理 | Job Object（`KILL_ON_JOB_CLOSE`）。アプリが落ちても子プロセスが残らない。ツールは止めた状態（`CREATE_SUSPENDED`）で起動し、Job に入れてから動かす（入れる前に起動された孫プロセスが Job の外に出ないように） | 子プロセスを新しいプロセスグループで起動（`process_group(0)`） |
| 中断 | `TerminateJobObject` | `killpg(SIGTERM)` → 3 秒後も残っていれば `SIGKILL` |
| ドライブのルート判定 | `Path::parent()` が `None`（`D:\`、`\\server\share\`） | `/` |
| 長いパス | マニフェストで `longPathAware` を有効にする | 不要 |
| アプリが終わらされたとき | Job Object を閉じると中のプロセスも終わる（`KILL_ON_JOB_CLOSE`） | SIGTERM・SIGHUP・SIGINT を受けたら中断してから終わる（CLI・GUI とも `ctrlc` の `termination`）。SIGKILL などで突然終わったときのために、子に `PR_SET_PDEATHSIG`（SIGTERM）を付ける（直接の子だけ） |

子プロセスは別のプロセスグループで起動するので（Ctrl+C が子に直接届かないように）、アプリが終わるときに
自分で止めないと、子は残って動き続ける（ログアウトやシャットダウンのときも同じ）。

## 7. 進捗解析

```rust
pub trait ProgressParser: Send {
    fn matches(program: &str) -> bool where Self: Sized;
    fn feed(&mut self, line: &str) -> Option<ProgressUpdate>;
}
```

| パーサ | 対象 | 判定方法 |
|---|---|---|
| `FfmpegParser` | ffmpeg | フレーム数で計算する（§7.1）。映像がないとき（`-vn` など）は `time=` と総時間で計算する |
| `RigayaParser` | QSVEncC / NVEncC / VCEEncC | `[xx.x%]` と `remain hh:mm:ss`、`est out size xxMB` |
| `X26xParser` | x264 / x265 | `[xx.x%]` |
| `Mp4BoxParser` | MP4Box | `(xx/100)` |
| `GenericParser` | その他 | 行中の `xx.x%` を拾う。拾えなければ進捗は「不明」 |

- 予想終了時刻：ツールが残り時間を出していればそれを使い、なければ `経過時間 / 進捗率` から計算する。
- 予想ファイルサイズ：出力ファイル（`<%OutputFile%>`）を書くコマンドのときだけ出す。ツールが出していればそれを使い、なければ `<出力名>.*`（ジョブの開始前になかったもの・開始後に更新日時か大きさが変わったもの）の合計サイズ / 進捗率。完了時の出力ファイルの一覧も同じ基準で選ぶ（直前のジョブが同じ名前・別の拡張子で作ったファイルを入れないため）。途中のコマンド（WAV への書き出しなど）では出さない（途中のファイルの大きさは、できあがるファイルの大きさと関係がなく、大きく外れるため）。
- ジョブ全体の表示は「コマンド i / N」と、そのコマンドの進捗率（要件 F-PG-03）。

### 7.1 ffmpeg の進捗計算（フレーム数）

プリセットのコマンドには手を加えず、ffmpeg が標準エラー出力に出す情報だけで計算する。

1. **総フレーム数を求める**（ffmpeg の起動直後に出るヘッダから）
   - `Duration: 00:23:40.12` → 総時間
   - 映像ストリームの行 `Stream #0:0 … 29.97 fps` → フレームレート
   - 総フレーム数 ≒ 総時間 × フレームレート
   - `-t` / `-to` / `-ss` がコマンドにあれば、総時間をその範囲に補正する
   - `-r` で出力フレームレートを変えている場合は、出力側のフレームレートを使う
2. **現在のフレーム数**：進捗行 `frame=21540 fps=412 … time=00:11:58.40 … speed=13.7x` の `frame=`
3. **進捗率** ＝ 現在のフレーム数 / 総フレーム数（100% を超えたら 99% に止める）
4. **残り時間** ＝（総フレーム数 − 現在のフレーム数）/ `fps=` の値。fps が取れないときは経過時間と進捗率から計算する

| 状況 | 扱い |
|---|---|
| 映像がない（音声だけの抽出など） | `time=` / 総時間 で進捗率を出す |
| 総時間が取れない（ライブ入力・壊れたファイル） | 進捗率は「不明」。フレーム数・速度・経過時間だけを出す |
| 入力が `.avs` / `.vpy` | ヘッダに総フレーム数相当の情報（総時間とフレームレート）が出るので、同じ方法で計算する |

> ffprobe を別に呼ぶ方法もあるが、ffprobe がない環境もあり、入力を 2 回開くことになるので使わない。
> GUI の進捗表示も同じパーサを使う。

## 8. 設定ファイル

### 8.1 SmartConverter.ini（UTF-8）

```ini
[General]
PresetFile=Transcoding.ini          ; 相対パスは設定ファイルのフォルダ → 実行ファイルのフォルダの順に探す（settings::resolve_preset_path）
DefaultPreset=QVGA標準画質15fpsステレオ
AutoStartOnAdd=1                    ; ファイルを追加したらすぐ変換を始める
AllAbort=1
ExitIfFinished=0
Language=ja

[Output]
OutputDir=D:\Videos\Mobile
TemporaryFolder=                    ; 空なら OS の一時ディレクトリ
OnExists=Rename                     ; Overwrite / Rename / Skip
DeletePartial=1                     ; 中断したとき、そのジョブで新しくできた出力（途中までのファイル）を消す
AddTime=0
AddTimePos=0                        ; 0:先頭(1字空け) 2:先頭（1・3＝末尾は本家と同じく何も付けない）
AddTimeFormat=[mm-dd]

[Process]
Priority=0                          ; 0:アイドル 1:通常 2:高 3:リアルタイム

[Script]
Disable=0
Ext0=AVI:cores\AVS_Skelton.avs
Ext1=TS:templates\default.vpy

[Tools]
; ツール名=パス（PATH より優先）
; qaac64=C:\tools\qaac\qaac64.exe

[Ui]
Theme=System                        ; System / Light / Dark
StartMinimized=0
Notify=1                            ; 処理中・完了を通知
LogToFile=0
MainWindow=100,100,820,560          ; x,y,幅,高さ（自動保存）
LogWindow=
PresetWindow=
```

- 場所（F-ST-03、`settings::default_path`）：実行ファイルと同じディレクトリに `SmartConverter.ini` があればそこ（ポータブル）。
  なければ、Windows は実行ファイルのディレクトリに書き込めればそこ、書き込めなければ `%APPDATA%\SmartConverter\`。
  Linux は `$XDG_CONFIG_HOME/smartconverter/`（既定 `~/.config/smartconverter/`）で、実行ファイルのディレクトリ（`~/.local/bin` など）には
  書き込めても作らない。Linux でポータブルに使うときは、実行ファイルの隣に空の `SmartConverter.ini` を置く。
- `PresetFile` の相対パスは、設定ファイルのディレクトリ → 実行ファイルのディレクトリの順に探す（同梱の `Transcoding.ini` を使えるように）。
- 初回起動で設定ファイルがないときは、本家 `3GP_Converter.ini` を取り込むかどうかを聞く（F-ST-02）。
  `DefaultSetitng`（本家の綴り誤り）、`ExePriority`、`[Avisynth]` などを読み替える。

### 8.2 タイムスタンプ書式

本家（Delphi 製）の `FormatDateTime` と同じ書式にする（[本家動作調査記録](original-behavior.md) §2）。

| トークン | 意味 | 例（2024-03-05 07:08:10 火曜） |
|---|---|---|
| `yyyy` / `yy` | 西暦 4 桁 / 2 桁 | `2024` / `24` |
| `m` / `mm` | 月（ゼロ埋めなし / 2 桁）。**`h`・`hh` の直後なら分** | `3` / `03` |
| `mmm` / `mmmm` | 月の名前（短い / 長い。日本語環境では `3月`） | `3月` |
| `d` / `dd` | 日（ゼロ埋めなし / 2 桁） | `5` / `05` |
| `ddd` / `dddd` | 曜日（短い / 長い） | `火` / `火曜日` |
| `h` / `hh` | 時（ゼロ埋めなし / 2 桁） | `7` / `07` |
| `n` / `nn` | 分（ゼロ埋めなし / 2 桁） | `8` / `08` |
| `s` / `ss` | 秒（ゼロ埋めなし / 2 桁） | `10` / `10` |
| `"…"` / `'…'` | 囲んだ文字をそのまま出す | |
| その他の文字 | そのまま出す | `[`、`-`、`_`、空白など |

- 画面の候補は本家と同じ 6 つ：`[mm-dd]`、`[mm-dd hh-mm]`、`[yy-mm-dd]`、`[yy-mm-dd hh-mm]`、`[yy-mm-dd hh-mm-ss]`、`[yyyymmdd]`。
- 曜日・月の名前は、本家に合わせて日本語で出す（UI の言語設定に従う）。
- 結果にファイル名として使えない文字（`:` など）が入ったら、F-FN-04 に従って置き換える。

## 9. GUI 実装方針

### 9.1 ウィンドウ

- eframe のマルチビューポート（`show_viewport_immediate`）で、メイン・ログ・オプション・プリセット編集を
  **OS の別ウィンドウ** として開く。immediate 方式なので、サブ画面は画面スレッドの状態をそのまま使える（ロックが要らない）。
- マルチビューポートに対応しない環境では、egui が自動でメインウィンドウ内の表示に切り替える。
- 変換の進み具合の受け取り・`Transcoding.ini` の監視・トレイの操作は `eframe::App::logic` で行う。
  `logic` は最小化中・トレイに格納中も呼ばれる（`ui` は呼ばれない）ので、隠れている間も変換が続き、トレイから戻せる。
- ウィンドウの位置と大きさは `[Ui]` に保存して、次回復元する。Wayland ではアプリが位置を扱えないため、大きさだけを復元する。
- Linux では `app_id` を `smartconverter` にする。Wayland のウィンドウアイコンは、同じ名前の `.desktop` ファイルとアイコンで表示される（アプリから渡したアイコンは使われない）。
- Linux のウィンドウ：Wayland のセッションではネイティブの Wayland で開く（既定）。winit 0.30（egui 0.37 が使う版）は Wayland でファイルのドロップを受け取れないため、
  `SMARTCONVERTER_X11=1` のときは X11（XWayland）で開く（`.desktop` の「X11 で起動」アクション）。Wayland ではキューが空のときの案内を
  「［ファイルを追加］で追加」に変える。
  - 今後の方針（v1.0 の検討で決定）：winit を自分で直すことはしない（回避策で実用上は困らないため）。egui が winit 0.31
    （Wayland のドロップに対応）を使う版を出したら、それに上げて、`SMARTCONVERTER_X11` での切り替え・Wayland 用の案内・
    `.desktop` の「X11 で起動」をやめ、`winit` の直接の依存も外す。`SMARTCONVERTER_X11` は、1 つの版のあいだは受け付けて何もしない。

### 9.2 テーマ

- 既定は OS のライト/ダークに追従（`ThemePreference::System`）。オプションで「ライト」「ダーク」に固定できる。
- アクセントカラーはアイコンの青（`#0284c7`、ダーク時 `#38bdf8`）。

### 9.3 日本語フォント

1. 起動時に OS の日本語フォントを読み込む（Windows：Yu Gothic UI → Meiryo UI → MS UI Gothic、Linux：fontconfig で `sans-serif:lang=ja` を照会）
2. 見つからなければ、同梱の Noto Sans JP（SIL OFL）を使う

### 9.4 ドラッグ＆ドロップ

- egui の `RawInput::dropped_files` を使う。ドラッグ中は `hovered_files` でドロップ領域を強調表示する。
- フォルダは登録しない（F-QU-01）。対象外のファイルはログに理由を出す。

### 9.5 トレイと通知

- トレイ：Windows は `tray-icon`。Linux は `ksni`（StatusNotifierItem を D-Bus で直接扱う純 Rust の実装で、GTK と AppIndicator が要らない）を cargo feature `tray`（既定で有効）で使う。KDE Plasma は標準で対応している。GNOME は AppIndicator 拡張が必要。D-Bus のセッションが無いなどで作れなければ、トレイなしで動く。
  - Wayland ではアプリがウィンドウを隠せない（winit が対応していない）ため、最小化してもトレイに格納せずタスクバーに残す。トレイのメニューはそのまま使える。
  - 開発の途中で `tray-icon` の GTK 版から `ksni` に変えた。理由：GTK のイベントループを別に回す必要がなく、ビルドに GTK・AppIndicator の開発パッケージが要らないため。
- 通知：`notify-rust`（Windows はトースト通知、Linux は freedesktop 通知）

## 10. エラー処理

| 分類 | 例 | 扱い |
|---|---|---|
| 設定エラー | `Transcoding.ini` の読み込み失敗、プリセット 0 件 | ダイアログで行番号付きで表示。変換は開始できない |
| ジョブエラー | ツールが見つからない、終了コード≠0（未定義の変数はエラーにせず警告のみ） | そのジョブを `Failed` にして、次のジョブへ進む。ログに原因を出す |
| 中断 | ユーザー操作 | `Aborted`。一時ファイルを削除 |
| 内部エラー | パニック | `catch_unwind` で Runner スレッドの異常を捕まえ、ジョブを `Failed` にする。パニックの内容は操作ログに書く |

- `sc-core` は `thiserror` で型付きのエラーを返す。アプリ側（`sc-gui` / `sc-cli`）は `anyhow` で包んで表示する。
- アプリ自身のログは `tracing` で、`LogToFile=1` のとき設定ディレクトリの `logs/` に日付ごとに書く。
- 操作ログ（GUI）：起動・終了・操作（ボタン、テーマの切り替え、行の操作など）・ジョブの開始と終了・パニックの内容を、設定ディレクトリの `Log/SmartConverter.log` に常に書く（1 MB を超えたら `.log.old` に移す）。画面が急に消えたときに原因を追うため（リリース版の Windows 版はコンソールがなく、標準のパニック表示が残らない）。最後の行が「終了」でなければ異常終了。入力ファイルの名前やパスは書かない（ジョブ番号とプリセット名だけ）。

## 11. 主な依存クレート

| 用途 | クレート |
|---|---|
| GUI | `eframe` / `egui`、`egui_extras`（表） |
| ファイル選択ダイアログ | `rfd` |
| 文字コード | `encoding_rs` |
| 引数分割（Linux） | `shell-words` |
| ファイル監視 | 2 秒ごとに更新日時を確かめる（`notify` は使わない。ネットワークドライブやエディタの保存方法の違いに左右されないため） |
| スレッド間通信 | `crossbeam-channel` |
| 日時 | `chrono` |
| 正規表現（進捗解析） | `regex` |
| CLI 引数 / プログレスバー | `clap` / `indicatif` |
| トレイ / 通知 | `tray-icon`（Windows）・`ksni`（Linux） / `notify-rust` |
| OS API | `windows-sys`（Windows）、`nix`（Linux） |
| エラー / ログ | `thiserror`、`anyhow`、`tracing` |

## 12. ビルド・配布

| 対象 | 成果物 |
|---|---|
| Windows x64 | `SmartConverter.exe`、`smartconverter-cli.exe`（アイコン・マニフェストを埋め込み、`winresource` を使う）を zip にまとめる |
| Linux x86_64 | `SmartConverter`、`smartconverter-cli`、`.desktop`、PNG アイコンを tar.gz にまとめる。AppImage は任意 |

配布物は `cargo xtask dist` で `dist/SmartConverter-<版>-<windows-x64|linux-x86_64>.zip|.tar.gz` に作る（各 OS で実行する）。
最上位にフォルダを 1 つ置き、その中を次の構成にする。

| パス | 内容 |
|---|---|
| `SmartConverter(.exe)`、`smartconverter-cli(.exe)` | 実行ファイル |
| `Transcoding.ini` | プリセット例（`presets/3gpp_aac.ini` の写し）。展開してすぐ使えるように、既定のプリセットファイル名で置く |
| `presets/3gpp_aac.ini`、`templates/default.avs`・`default.vpy` | プリセット例・スケルトン例の原本 |
| `README.md`、`docs/preset-guide.md` | 使い方、プリセットの書き方（置換変数の一覧） |
| `LICENSE-MIT`、`LICENSE-APACHE`、`THIRD-PARTY-LICENSES.txt` | ライセンス。`THIRD-PARTY-LICENSES.txt` は、配布する実行ファイルの通常の依存（`cargo metadata` の解決結果をたどる。ビルド用・テスト用は除く）について、クレートに同梱のライセンス文（`LICENSE*`・`LICENSES/`・同梱フォントの `fonts/*.txt`）を集めたもの。同じ文は 1 回だけ載せる。ライセンス文を同梱していないクレートは、`license` の MIT / Apache-2.0 / BSL-1.0 の標準の文を載せる（MIT の著作権者は `authors`） |
| `install.sh`、`share/applications/`、`share/icons/hicolor/` | Linux のみ。`install.sh`（`assets/linux/install-local.sh` と同じもの）で `~/.local` に入れる |

- 配布用のビルドは GitHub Actions の `release.yml` で行う（Linux は Ubuntu 24.04 ＝ glibc 2.39）。手動実行では成果物（artifact）として残し、`v*` のタグを push するとリリースの下書き（pre-release、`SHA256SUMS.txt` 付き）を作る。

- アイコン生成：`cargo xtask icons` で SVG から PNG（16〜512px）と ICO（16/24/32/48/64/128/256）を作る。
  16〜32px は簡略版の `smartconverter-small.svg` を使う。
- 同梱するプリセット例：`presets/3gpp_aac.ini`。本家 0.34 の既定の設定（`Transcoding_3GPP_AAC.ini`、3GPP＋AAC の一般設定 6 件）を、
  画面の大きさ・フレームレート・ビットレートはそのままに、今の ffmpeg だけで同じものを作るように書き直したもの（QuickTime の `QT3GPPFlatten` は `-movflags +faststart` に置き換える）。
  ffmpeg だけを使うので両 OS で同じものを使う。一時ファイルの削除行（`cmd /c del` / `rm`）は入れない（アプリが消すため）。
  `presets/` の中身は `sc-core` のテスト（`preset_examples.rs`）で、読み込めること・入力チェックを通ること・ffmpeg だけを使い Windows と POSIX の両方の規則で分けられることを確かめる。
- 同梱するスケルトン例：`templates/default.avs`（L-SMASH Works）、`templates/default.vpy`（同。パスは `r"…"` で囲む）。
- Linux のデスクトップ連携：`assets/linux/smartconverter.desktop`（`StartupWMClass=smartconverter`、Wayland の `app_id` と同じ）。
  手元で確かめるときは `sh assets/linux/install-local.sh` で `~/.local` に入れる（バイナリ・.desktop・アイコン）。
  配布物から入れるときは、同じスクリプトが `install.sh` として入っている（配布物の構成を見分けて、まだ無ければ `Transcoding.ini` も `~/.local/bin` に入れる）。

## 13. テスト方針

| 対象 | 内容 |
|---|---|
| `text` | UTF-8 BOM あり / なし、CP932、壊れたバイト列 |
| `ini` | 読み込み → 書き戻しで内容が変わらないこと（今の `Transcoding.ini` を使う） |
| `vars` | すべての変数、未定義の変数（文字のまま残ること）、パス区切り、タイムスタンプ（秒を丸めないこと） |
| `cmdline` | Windows の引用符規則（`""ffmpeg" -i "a b.ts""` など）、POSIX の分割 |
| `progress` | 各ツールの実際の出力を記録したファイルを使った解析テスト |
| `runner` | 疑似ツール（テスト用の小さなバイナリ）を使った、成功・失敗・中断・一時ファイル削除のテスト |
| 結合 | CLI の `--dry-run` で、今のプリセットが期待どおりのコマンドに展開されること |
| GUI 操作 | `egui_kittest` で、ログ画面・オプション画面などの操作（押す・入力する・コピーする）と、アクセシビリティの名前を確かめる |
