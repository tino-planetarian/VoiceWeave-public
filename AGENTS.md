# VoiceWeave 開発の手引き

人とAI（Codex、Claude Code など）の両方が、開発を始める前に読む文書です。使い方は [README.md](README.md)。

## 全体の仕組み

Windows専用の常駐アプリ。Tauri 2（Rust）＋ React / TypeScript の画面＋ 常駐するPythonの認識ワーカー。

```
ショートカット → 録音（Rust） → 認識（Pythonワーカー / Groq） → 後処理（Rust） → 入力（Rust）
```

- `src/`（画面、React）: `App.tsx`（設定画面の本体）、`PolishPanel.tsx`（整形）、`RecognitionMode.tsx`（認識の方式）、`ModelPanel.tsx`・`GpuPanel.tsx`（モデルとGPU部品の導入）、`DictionaryEditor.tsx`、`DiagnosticsPanel.tsx`、`OllamaGuide.tsx`。
- `src-tauri/src/`（Rust）:
  - `pipeline.rs`: 録音→認識→入力の中核。状態は `coordinator.rs`。
  - `recording.rs`（録音）、`streaming.rs`（話しながらの認識）、`worker.rs`（ワーカーとの通信）、`engine.rs`（モデルの切り替えと巻き戻し）。
  - 後処理: `postprocess.rs`（暴走・決まり文句・言いよどみ）、`dictionary.rs`、`glossary.rs`、`spoken.rs`（言い直し・数字）、句読点は規則か `polish.rs`（Ollama）。
  - `polish.rs`（整形の指示とOllama通信）、`cloud.rs`（OpenAI・Anthropic・Groq）、`ollama.rs`・`ollama_install.rs`。
  - `input.rs`（貼り付けとクリップボード）、`hotkeys.rs`、`screen.rs`（画面の言葉の読み取り）、`session_guard.rs`（ロック・スリープ）、`memory_gate.rs`（メモリ不足時の切り替え）、`diagnostics.rs`、`settings.rs`、`lib.rs`（画面から呼ぶコマンド）。
- `worker/voiceweave_worker/`（Python）: `__main__.py`（通信の入口）、`sherpa_engine.py`（Parakeet）、`short_window.py`・`streaming.py`（Whisperの高速認識）、`model_install.py`・`cuda_install.py`（導入）。モデルの取得元は `*_catalog.json` に固定revisionで書く。
- `scripts/`: ビルド・起動・試験・評価の道具。

## 開発環境

Windows 11 x64、Node.js / npm、Rust（MSVC toolchain）、Visual Studio C++ Build Tools、Python 3.12 x64。

```powershell
npm.cmd ci
powershell -NoProfile -File .\scripts\bootstrap.ps1      # Pythonの .venv と依存を準備
```

## 動かす・試す

```powershell
# 開発版（マイク・録音を含む）をビルドして起動。インストール版のVoiceWeaveは先に終了する
npm.cmd run build
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-capture-dev.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-capture-dev.ps1

# 試験一式
powershell -NoProfile -File .\scripts\verify.ps1
cd src-tauri; cargo test --all-targets
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features experimental-audio -- -D warnings
```

インストーラー: `scripts/build-worker.ps1`（ワーカーをPyInstallerで固める）のあと `npm.cmd run tauri -- build --bundles nsis`。出力は `src-tauri/target/release/bundle/nsis/`。Windowsのユーザー名がビルドに埋め込まれないよう、`RUSTFLAGS='--remap-path-prefix=C:\Users\<名前>=C:\Users\user'` を付けるとよい。

フォルダーを移動したあと、古いビルドの残り（`src-tauri/target/`）が原因でビルドが失敗したら、その中の該当フォルダーを消して作り直す。

## 守る約束

- **音声・認識した文章・辞書・ウィンドウタイトルを、ログ・診断・Gitに残さない。** 診断には時間・件数・状態だけを入れる。
- **入力の安全条件を緩めない。** 録音開始時と入力先が変わったら貼り付けず、コピーだけにする。ロック・スリープでも同じ。
- **APIキーはWindowsの資格情報マネージャーに保存する。** ファイル・ログ・画面に出さない。
- **外部へ送るものは、画面の説明と一致させる。** 何がどこへ送られるかを変えたら、説明文も直す。
- **試していないことを「できた」と書かない。** 実際のマイクでの確認は人が行う。合成音声の結果を、実際の声の結果として扱わない。
- 画面の文言は日本語。コードのコメントと識別子は英語。
- 1つのフォルダーで複数のAIが同時に編集・ビルド・Git操作をしない。

## 用語

- 整形（F9）: 話した内容を書き言葉に整える。作文（Shift+F9）: 話した依頼から文章を作る。
- 話しながらの認識: 録音中に区切りごとに先に認識しておき、停止後の待ちを短くする仕組み。
- 用語集: 分野の言葉の一覧（整形AIへの指示と聞き取りのヒント）。辞書: 聞き違いの書き換え。
