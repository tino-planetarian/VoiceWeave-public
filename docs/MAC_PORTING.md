# Mac 版に挑戦したい方へ

VoiceWeave は Windows 用のアプリですが、土台の Tauri 2 は macOS にも対応しています。この文書は、Mac 版を自分で作りたい人のための案内です。AI に開発を手伝わせるときは、[AGENTS.md](../AGENTS.md) と一緒にこの文書を読ませてください。

## 進め方の方針

- Mac 版などの派生は、**各自のフォークで自由に進めてください**。
- 本家（このリポジトリ）へ取り込む予定はありません。Pull Request も基本的には受け付けません。自分の版は、自分のリポジトリで完結させてください。
- 複数の人が、それぞれのやり方で別々の版を作ってかまいません。
- ライセンスは [PolyForm Noncommercial 1.0.0](../LICENSE) です。派生版も非営利に限り、`LICENSE` と、README 末尾の **Required Notice**（著作権表示）を残してください。

作者は Mac を持っていないので、この文書の内容は Mac の実機では確かめていません。コードを読んで整理した見取り図として使ってください。

## 現在の状態

**今のままでは、Mac ではビルドが通りません。**

- `src-tauri/Cargo.toml` の `windows`、`windows-sys`、`clipboard-win` が、Windows 専用の指定（`[target.'cfg(windows)'.dependencies]`）になっていません。
- いくつかのファイルが、Windows の機能を条件なしで直接呼んでいます。

最初の作業は、**Mac でビルドが通る状態にすること**です。手順は「おすすめの順番」の 1〜3 に書いています。

## そのまま使える部分

- 画面（`src/` の React）: 設定、辞書、診断、録音中の小窓の表示。
- Rust の処理の大半: 辞書、整形、句読点の後処理、設定の読み書きの中身、ローカル AI（Ollama）とのやり取り。
  - 対象は `dictionary.rs`、`postprocess.rs`、`polish.rs`、`spoken.rs`、`pipeline.rs`、`coordinator.rs` など。
- 録音: `cpal` は macOS にも対応しています。Windows 専用なのは、録音スレッドの優先度を上げる部分だけです。
- 認識部品（`worker/`、Python）: 使っている `sherpa-onnx`（Parakeet）と `faster-whisper` は、どちらも macOS で CPU 動作します。Apple シリコンの CPU なら、Parakeet は実用的な速さが見込めます。

## Mac 用に作り直す部分

`src-tauri/src/` の中で、Windows 専用の機能を使っている部分です。

| ファイル | 今やっていること（Windows） | Mac での置き換えの候補 | 重さ |
| --- | --- | --- | --- |
| `input.rs` | 文字の入力・貼り付け（SendInput、修飾キーの状態） | CGEvent（キー入力）と NSPasteboard。**アクセシビリティの許可**が必要 | 中 |
| `hotkeys.rs` | F8 / F9 の全体ショートカット（キーボードフック） | CGEventTap など。**入力監視の許可**が必要。一部は `shortcut.rs` と Tauri の global-shortcut で代用できる可能性あり | 中 |
| `screen.rs` | 画面の文字を参考情報として読む（UI Automation、任意の設定） | Accessibility API（AXUIElement）。最初は「未対応」にしてもアプリは動く | 重い |
| `overlay.rs` | 録音中の小窓（最前面、フォーカスを奪わない） | Tauri の窓設定と NSWindow のレベル指定 | 中 |
| `cloud.rs` | API キーの保管（資格情報マネージャー）と HTTPS 通信（WinHTTP） | キーは Keychain（`keyring` クレートなど）、通信は `reqwest` など | 中 |
| `session_guard.rs` | 画面ロック・スリープで録音を止める | NSWorkspace の通知（スリープ、画面ロック） | 中 |
| `autostart.rs` | サインイン時の自動起動（レジストリ） | LaunchAgent。Tauri の autostart プラグインも使える | 軽い |
| `job.rs` | アプリ終了時に子プロセス（認識部品）を確実に止める（ジョブオブジェクト） | プロセスグループと終了時の後始末 | 軽い |
| `diagnostics.rs`、`memory_gate.rs` | メモリの使用状況 | `sysctl` / `host_statistics64`（`sysinfo` クレートなど） | 軽い |
| `settings.rs`、`models.rs` | ファイルの扱いの細部（属性、パス） | 標準ライブラリで置き換え | 軽い |
| `audio.rs` | 録音スレッドの優先度（MMCSS） | なしでも動く。必要ならスレッド優先度の指定 | 軽い |
| `lib.rs`、`worker.rs`、`ollama*.rs`、`install.rs` | 子プロセスの窓を隠す設定、ブラウザーを開く（explorer.exe） | 窓の設定は不要。ブラウザーは `open` コマンド | 軽い |
| `gpu.rs`、`cuda.rs`、`install.rs` の CUDA 部分 | NVIDIA GPU の高速化 | Mac には当てはまらない。外すか「対象外」と表示 | 軽い |

AGENTS.md の「守る約束」は、Mac 版でも守ってください。
- 入力先が変わったら貼り付けず、コピーだけにする。
- ロック・スリープで止める。
- 音声・本文・辞書・ウィンドウ名をログに残さない。
- API キーはファイルやログに出さない。Mac では Keychain に保存する。

## おすすめの順番

1. `Cargo.toml` の Windows 専用の部品を `[target.'cfg(windows)'.dependencies]` に移す。
2. 上の表のファイルを、Windows 用と Mac 用に分ける（`#[cfg(windows)]` / `#[cfg(target_os = "macos")]`）。最初は Mac 側を「未対応」と返す仮の中身にする。
3. Mac で `npm run tauri dev` が起動し、設定画面が表示されるところまで進める。
4. 認識部品（`worker/`）を Mac で動かす。CPU の Parakeet から始める。PyInstaller の設定（`voiceweave-worker.spec`）と、Windows 用のビルドスクリプト（`scripts/*.ps1`）は、Mac 用に書き直しが要る。
5. 録音 → 認識 → クリップボードに結果、まで通す。
6. 入力（`input.rs`）とショートカット（`hotkeys.rs`）を Mac 用に作る。ここで、アクセシビリティと入力監視の許可の流れも作る。
7. ロック・スリープ、自動起動、API キー（Keychain）などの残りを作る。
8. 画面の読み取り（`screen.rs`）は、必要なら最後に作る。

## Mac 特有の注意

- **許可:** マイク、アクセシビリティ、入力監視の許可が必要です。`Info.plist` にマイクを使う理由（`NSMicrophoneUsageDescription`）を書く必要があります。
- **配布:** 他の人に配るなら、Apple Developer Program（有料）に登録して、署名と公証（notarization）を行う必要があります。しない場合、受け取った人は Gatekeeper の警告を自分で解除して開くことになります。
- **確認:** ビルドだけなら GitHub Actions の macOS 環境でもできます。ただ、マイク、ショートカット、貼り付け、許可の画面は、Mac の実機で確かめる必要があります。
- **速さ:** Windows 版の速さは、NVIDIA GPU を使った実測にもとづいています。Mac 版の速さは改めて測ってください。Metal を使う whisper.cpp や MLX などの別の認識部品に替えると、さらに速くなる可能性があります。
