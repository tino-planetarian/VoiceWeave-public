# Mac 版に挑戦したい方へ

VoiceWeave は Windows 用のアプリですが、土台の Tauri 2 は macOS にも対応しています。この文書は、Mac 版を自分で作りたい人のための案内です。AI に開発を手伝わせるときは、[AGENTS.md](../AGENTS.md) と一緒にこの文書を読ませてください。

## 進め方の方針

- Mac 版などの派生は、**各自のフォークで自由に進めてください**。
- 本家（このリポジトリ）へ取り込む予定はありません。Pull Request も基本的には受け付けません。自分の版は、自分のリポジトリで完結させてください。
- 複数の人が、それぞれのやり方で別々の版を作ってかまいません。
- ライセンスは [PolyForm Noncommercial 1.0.0](../LICENSE) です。派生版も非営利に限り、`LICENSE` と、README 末尾の **Required Notice**（著作権表示）を残してください。

**この文書の内容は、Mac の実機では一つも確かめていません。** 作者が Windows のコードを読んで整理した見取り図です。

- 「使えるはず」「候補」と書いたものは、すべて未確認です。
- 実際に動くかどうかは、移植する人が Mac で確かめてください。

## 現在の状態

**今のままでは、Mac ではビルドが通りません。**

- `src-tauri/Cargo.toml` の `windows`、`windows-sys`、`clipboard-win` が、Windows 専用の指定（`[target.'cfg(windows)'.dependencies]`）になっていません。
- いくつかのファイルが、Windows の機能を条件なしで直接呼んでいます。

最初の作業は、**Mac でビルドが通る状態にすること**です。手順は「おすすめの順番」の 1〜3 に書いています。

## 再利用できる見込みの部分

ここに挙げた部分も、**Mac で動くことは確かめていません**。中身の考え方（処理の手順や判定）は流用できる見込みが高い、という意味です。それぞれ、周りの部品の移植と Mac での確認が必要です。

| 部分 | 流用できる見込みのもの | 別に必要な移植・確認 |
| --- | --- | --- |
| 画面（`src/` の React） | 設定、辞書、診断、録音中の小窓の表示 | 文言が Windows 前提（F8、Ctrl、タスクトレイ、GPU の案内など）。小窓の窓の作り方は `overlay.rs` 側 |
| 後処理（`dictionary.rs`、`postprocess.rs`、`spoken.rs`、`glossary.rs`） | 辞書・言いよどみ・数字などの文字列処理 | 文字列だけを扱うので、Mac 固有の作業は少ない見込み |
| 整形（`polish.rs`、`ollama.rs`） | 整形の指示、Ollama との通信の中身 | Ollama の起動や導入の確認方法（`ollama_install.rs` は Windows 用の導入手順） |
| 録音の中核（`pipeline.rs`、`coordinator.rs`、`recording.rs`、`streaming.rs`） | 録音 → 認識 → 後処理 → 入力の流れと状態の管理 | 入力先の取得・入力・画面の読み取りなど、Windows 用の部品を呼んでいる |
| 録音（`audio.rs`） | `cpal` を使った録音。`cpal` 自体は macOS にも対応 | マイクの許可、Mac のマイクでの音切れ（Xrun）や遅れの確認。録音スレッドの優先度を上げる部分は Windows 専用 |
| 認識部品（`worker/`、Python） | 通信の仕組み、Parakeet（`sherpa-onnx`）と Whisper（`faster-whisper`）の呼び出し | 両ライブラリには macOS 向けの配布があるが、この組み合わせでの動作は未確認。PyInstaller での固め方、モデルの導入手順の確認 |

**速さについて:** Apple シリコンの CPU で Parakeet がどのくらいの速さで動くかは、**測っていません**。
- Windows 版の速さの数字を、Mac 版の目安にしないでください。
- 実際の声で測ってから判断してください。

## Mac 用に作り直す部分

`src-tauri/src/` の中で、Windows 専用の機能を使っている部分です。「Mac での置き換えの候補」は調べ始める手がかりです。動作は確かめていません。

| ファイル | 今やっていること（Windows） | Mac での置き換えの候補 | 重さ |
| --- | --- | --- | --- |
| `input.rs` | 入力先（最前面の窓・フォーカス・プロセス）の記録と照合、クリップボードへのコピー、Ctrl+V の送信、修飾キーが離されたかの確認 | 入力先の照合は NSWorkspace や Accessibility API、コピーは NSPasteboard（変更回数は `changeCount`）、貼り付けは CGEvent で Cmd+V。キー送信には**アクセシビリティの許可**が要る見込み | 重い |
| `hotkeys.rs` | 全体ショートカットの登録（`RegisterHotKey`。キーボードフックは使っていない） | Carbon の `RegisterEventHotKey`、または CGEventTap。方式によって必要な許可が変わる。Tauri の global-shortcut プラグインは**現在は入っていない**ので、使うなら追加と検討が必要 | 中 |
| `shortcut.rs` | ショートカット文字列の解析と確認 | Win32 のキーコード（VK）と修飾キー（Win キー）を前提にしているので、Mac のキーと Cmd に合わせて直す。既定の F8 などのキーが Mac でそのまま使えるかも確認が必要 | 中 |
| `screen.rs` | 画面の文字を参考情報として読む（UI Automation、任意の設定） | Accessibility API（AXUIElement）。最初は「未対応」にしても、アプリの主な機能には影響しない設計 | 重い |
| `overlay.rs` | 録音中の小窓（最前面、フォーカスを奪わない） | Tauri の窓設定（`always_on_top`、`focused(false)` はすでに使っている）と、必要なら NSWindow のレベル指定 | 中 |
| `cloud.rs` | API キーの保管（資格情報マネージャー）と HTTPS 通信（WinHTTP）。Groq での認識、OpenAI / Anthropic での整形 | キーは Keychain（`keyring` クレートなど、**未導入**）、通信は `reqwest` など（**未導入**）。送る内容は Windows 版と同じにして、画面の説明と一致させる | 中 |
| `session_guard.rs` | 画面ロック・スリープからの復帰を見張り、録音を止める | スリープは NSWorkspace の通知。画面ロックは別の通知（distributed notification など）が候補。Mac での確実な検出方法は要確認 | 中 |
| `autostart.rs` | サインイン時の自動起動（レジストリ） | LaunchAgent、またはログイン項目。Tauri の autostart プラグインは**現在は入っていない** | 軽い |
| `job.rs` | アプリが落ちても、子プロセス（認識部品）を確実に止める（ジョブオブジェクト） | Mac には同じ仕組みがない。プロセスグループ、終了時の後始末、親の終了を子が検知する方法などを組み合わせる | 中 |
| `diagnostics.rs`、`memory_gate.rs` | メモリの使用状況 | `sysctl` や `host_statistics64`（`sysinfo` クレートなど、**未導入**）。Mac はメモリ圧縮があるので、「足りない」と判断するしきい値も見直しが必要 | 中 |
| `settings.rs`、`models.rs` | ファイルの扱いの細部（属性、パス） | 標準ライブラリでの置き換え | 軽い |
| `audio.rs` | 録音スレッドの優先度（MMCSS） | なくても動く見込み。必要ならスレッドの優先度を指定 | 軽い |
| `lib.rs` | 開発版の Python の場所（`.venv/Scripts/python.exe`）、配布版の認識部品の場所（`voiceweave_worker.exe`）、ブラウザーを開く（explorer.exe） | Mac では `.venv/bin/python`、`.exe` のない名前、`open` コマンド | 軽い |
| `worker.rs`、`ollama*.rs`、`install.rs` | 子プロセスの窓を隠す設定 | Mac では不要 | 軽い |
| `gpu.rs`、`cuda.rs`、`install.rs` の CUDA 部分、`worker/voiceweave_worker/cuda_install.py` | NVIDIA GPU での高速化 | Mac には当てはまらない。外すか「対象外」と表示 | 軽い |
| `worker/voiceweave_worker/resident.py` | 認識部品をメモリに留める（Windows のみ。他の OS では何もしない） | そのままで問題ない見込み | 軽い |

## 守る約束（入力の安全条件）

AGENTS.md の「守る約束」は、Mac 版でも**緩めずに**守ってください。今の Windows 版の実装は次のとおりです。

- **入力先の確認:** 録音を始めたときの入力先（最前面の窓、フォーカスのある部品、プロセス）を記録します。結果は、まずクリップボードにコピーします。そのあと、次のどれかに当たれば**貼り付けず、コピーだけ**にします（`input.rs` の `deliver_inner`）。
  - 入力先を記録できなかった
  - 入力先が変わった
  - 修飾キーが 1 秒以内に離されなかった
  - クリップボードが他から書き換えられた
  - 取り消された
- **ロック・スリープ:** 画面のロック、またはスリープからの復帰を検出したら、**録音を止めます**。その回の結果は**入力せず、クリップボードにコピーだけ**します（`session_guard.rs`、`pipeline.rs` の `keep_after_lock`）。ロック解除後に、同じ窓へ勝手に入力されるのを防ぐためです。
- **記録:** 音声・認識した文章・辞書・ウィンドウ名を、ログ・診断・Git に残しません。
- **API キー:** ファイル・ログ・画面に出しません。Windows 版は資格情報マネージャーに保存しています。Mac では Keychain を使うのが自然です。

Mac で入力先の照合やロックの検出がまだ確実にできない段階では、**貼り付けをせず、コピーだけにする**状態で使ってください。照合できないまま貼り付けるのは、安全条件を緩めることになります。

## おすすめの順番

1. `Cargo.toml` の Windows 専用の部品を `[target.'cfg(windows)'.dependencies]` に移す。
2. 上の表のファイルを、Windows 用と Mac 用に分ける（`#[cfg(windows)]` / `#[cfg(target_os = "macos")]`）。最初は Mac 側を「未対応」と返す仮の中身にする。
   - 入力は「コピーだけ」にする。
   - ロック・スリープの検出ができるまでは、録音そのものを使わない判断もありえる。
3. Mac で `npm run tauri dev` が起動し、設定画面が表示されるところまで進める。
4. 認識部品（`worker/`）を Mac で動かす。CPU の Parakeet から始める。PyInstaller の設定（`voiceweave-worker.spec`）と、Windows 用のスクリプト（`scripts/*.ps1`。準備・ビルド・起動・試験）は、Mac 用に書き直しが要る。
5. 録音 → 認識 → クリップボードに結果、まで通す。実際の声での速さと音切れを、ここで測る。
6. ロック・スリープの検出を作る。
7. ショートカット（`hotkeys.rs`、`shortcut.rs`）と、入力先の照合と貼り付け（`input.rs`）を作る。ここで、アクセシビリティなどの許可の流れも作る。照合ができて初めて、貼り付けを有効にする。
8. 自動起動、API キー（Keychain）、メモリの監視などの残りを作る。
9. 画面の読み取り（`screen.rs`）は、必要なら最後に作る。

## Mac 特有の注意

- **許可:** マイクの許可は必ず要ります。`Info.plist` に、マイクを使う理由（`NSMicrophoneUsageDescription`）を書く必要があります。キー入力の送信や、他のアプリの画面の読み取りには、アクセシビリティの許可が要る見込みです。ショートカットの方式によっては、入力監視の許可も要ります。
- **配布:** 配り方によって、必要なことが変わります。
  - **自分の Mac で使うだけ:** 自分でビルドして使うなら、Apple の開発者登録は要りません。
  - **ソースを公開して、各自にビルドしてもらう:** これも登録なしでできます。
  - **ビルド済みのアプリを他の人に配る:** 署名と公証（notarization）をしないと、受け取った人の Mac で Gatekeeper の警告が出ます。受け取った人が自分で許可すれば開けますが、手間がかかり、慣れない人には難しい手順です。警告なく開けるようにするには、Apple Developer Program（有料）への登録と、署名・公証が必要です。
  - 許可の画面（マイク、アクセシビリティ）は、アプリの署名と結びついて記録されます。署名がない、または変わると、作り直すたびに許可をやり直すことになる場合があります。
- **確認:** ビルドだけなら GitHub Actions の macOS 環境でもできます。マイク、ショートカット、貼り付け、許可の画面、ロック・スリープは、Mac の実機で確かめる必要があります。
- **速さ:** Windows 版の速さは、Windows PC での実測にもとづいています。Mac 版の速さは改めて測ってください。Metal を使う whisper.cpp や MLX などの別の認識部品も候補ですが、速さも組み込みの手間も未確認です。
- **配布の形:** `src-tauri/tauri.conf.json` は Windows のインストーラー（NSIS）向けの設定が中心です。`icons/icon.icns` はあります。Mac 用の `.app` / `.dmg` の設定と、認識部品を同梱する場所（`resources/stt/`）の確認が必要です。
