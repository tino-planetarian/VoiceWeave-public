import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import { DiagnosticsPanel, newerDiagnostics, type DiagnosticsSnapshot } from "./DiagnosticsPanel";
import { GpuPanel, type CudaOutcome, type CudaProgress, type CudaStatus } from "./GpuPanel";
import { ModelPanel, type ModelAvailability } from "./ModelPanel";
import { MicrophoneSelect, type InputDevice, type Microphone } from "./MicrophoneSelect";
import { DictionaryEditor, type DictionaryEntry, type DictionaryPreview } from "./DictionaryEditor";
import { DictionarySuggestions } from "./DictionarySuggestions";
import { ShortcutInput } from "./ShortcutInput";
import { PolishPanel, type CloudApi, type PolishSettings } from "./PolishPanel";
import { OllamaGuide } from "./OllamaGuide";
import { RecognitionMode, type GroqApi, type RecognitionModeValue } from "./RecognitionMode";

type WorkerProbe = { version: string };
type HotkeyStatus = { record: boolean; repaste: boolean; polish?: boolean; compose?: boolean; english?: boolean };
type Settings = {
  schema_version: number;
  revision: number;
  shortcuts: { record: string; repaste: string; english_enabled?: boolean; english?: string };
  recognition: { preset: string; mode?: RecognitionModeValue; cloud_hints?: boolean };
  input_device: InputDevice;
  autostart: boolean;
  dictionary: { entries: DictionaryEntry[]; glossary?: string };
  remove_fillers?: boolean;
  tidy_numbers?: boolean;
  fix_restatements?: boolean;
  polish?: PolishSettings;
  punctuation?: { mode: "rules" | "local_ai"; model: string; auto_rules_when_low_memory?: boolean };
  paragraphs?: boolean;
  screen_context?: boolean;
  streaming?: boolean;
  live_preview?: boolean;
};
type SettingsResponse = { settings: Settings; recovered: boolean };
type EngineStatus = { enabled: boolean; phase: string; active_preset: string | null; external_override: boolean; device: string | null };
type Tab = "basic" | "dictionary" | "polish" | "setup" | "diagnostics";
const tabs: Array<[Tab, string]> = [["basic", "基本"], ["polish", "整形"], ["dictionary", "辞書"], ["setup", "初期設定"], ["diagnostics", "診断"]];
const presetLabel = (preset: string | null) => preset === "standard" ? "標準（turbo）"
  : preset === "accurate" ? "高精度（large-v3）"
  : preset === "dev-tiny" ? "検証用（tiny）" : preset === "external" ? "開発用指定フォルダー"
  : preset === "parakeet-ja" ? "Parakeet 日本語（試験中）" : "未準備";
/// Engines other than Whisper (sherpa-onnx, CPU only): listed once their files are present.
const experimentalPresets = ["parakeet-ja"];

/// Cloud services for polishing; API keys go straight to the Windows Credential Manager.
const cloudApi: CloudApi = {
  keyStatus: () => invoke<Record<"openai" | "anthropic" | "groq", boolean>>("api_key_status"),
  saveKey: (provider, key) => invoke<void>("save_api_key", { provider, key }),
  deleteKey: (provider) => invoke<void>("delete_api_key", { provider }),
  listModels: (provider) => invoke<string[]>("cloud_models", { provider }),
};

/// Cloud recognition (Groq); the key goes straight to the Windows Credential Manager.
const groqApi: GroqApi = {
  hasKey: () => invoke<Record<string, boolean>>("api_key_status").then((status) => status.groq === true),
  saveKey: (key) => invoke<void>("save_api_key", { provider: "groq", key }),
  deleteKey: () => invoke<void>("delete_api_key", { provider: "groq" }),
  check: () => invoke<string[]>("cloud_models", { provider: "groq" }),
};

function App() {
  // Keep this window above others (for example while testing); not saved between runs.
  const [pinned, setPinned] = useState(false);
  const [worker, setWorker] = useState<WorkerProbe | null>(null);
  const [status, setStatus] = useState("確認中…");
  const [busy, setBusy] = useState(false);
  const [hotkeys, setHotkeys] = useState<HotkeyStatus | null>(null);
  const [notice, setNotice] = useState("");
  const [probeEnabled, setProbeEnabled] = useState(false);
  const [developmentBuild, setDevelopmentBuild] = useState(false);
  const [captureEnabled, setCaptureEnabled] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saved, setSaved] = useState<Settings | null>(null);
  const savedShortcuts = saved?.shortcuts ?? null;
  const unsaved = settings !== null && saved !== null && JSON.stringify(settings) !== JSON.stringify(saved);
  const [tab, setTab] = useState<Tab>("basic");
  const [lastNotice, setLastNotice] = useState("");
  const [appVersion, setAppVersion] = useState<string | null>(null);
  // Which shortcut field waits for keys, and the registered pair, read by the event listener.
  const capturing = useRef<"record" | "repaste" | "polish" | "compose" | "english" | null>(null);
  const registered = useRef<Settings["shortcuts"] | null>(null);
  registered.current = savedShortcuts;
  const savedPolishKey = useRef<string | null>(null);
  savedPolishKey.current = saved?.polish?.enabled ? saved.polish.shortcut : null;
  const savedComposeKey = useRef<string | null>(null);
  savedComposeKey.current = saved?.polish?.enabled && (saved.polish.compose_enabled ?? true)
    ? (saved.polish.compose_shortcut ?? "Shift+F9") : null;
  const savedEnglishKey = useRef<string | null>(null);
  savedEnglishKey.current = saved?.shortcuts.english_enabled ? (saved.shortcuts.english ?? "Shift+F8") : null;
  const captureField = (field: "record" | "repaste" | "polish" | "compose" | "english") => (active: boolean) => {
    capturing.current = active ? field : null;
    void invoke("capture_shortcut", { active }).catch(() => undefined);
  };
  const sameShortcut = settings !== null && settings.shortcuts.record === settings.shortcuts.repaste;
  const polishClash = settings?.polish?.enabled === true
    && [settings.shortcuts.record, settings.shortcuts.repaste].includes(settings.polish.shortcut);
  const composeKey = settings?.polish?.compose_shortcut ?? "Shift+F9";
  const composeClash = settings?.polish?.enabled === true && (settings.polish.compose_enabled ?? true)
    && [settings.shortcuts.record, settings.shortcuts.repaste, settings.polish.shortcut]
      .some((key) => key.toLowerCase() === composeKey.toLowerCase());
  const englishKey = settings?.shortcuts.english ?? "Shift+F8";
  const englishClash = settings?.shortcuts.english_enabled === true
    && [settings.shortcuts.record, settings.shortcuts.repaste,
      ...(settings.polish?.enabled ? [settings.polish.shortcut] : []),
      ...(settings.polish?.enabled && (settings.polish.compose_enabled ?? true) ? [composeKey] : [])]
      .some((key) => key.toLowerCase() === englishKey.toLowerCase());
  const [settingsBusy, setSettingsBusy] = useState(false);
  // The last save's result, in the save bar: the notice at the top of the page is out of
  // view when the page is scrolled down to the button (2026-10-22).
  const [saveResult, setSaveResult] = useState("");
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [models, setModels] = useState<ModelAvailability[]>([]);
  const [diagnostics, setDiagnostics] = useState<DiagnosticsSnapshot | null>(null);
  const [cuda, setCuda] = useState<CudaStatus | null>(null);
  const [cudaProgress, setCudaProgress] = useState<CudaProgress | null>(null);
  const [cudaOutcome, setCudaOutcome] = useState<CudaOutcome>(null);
  const [modelInstalling, setModelInstalling] = useState(false);
  const [autostart, setAutostart] = useState<{ supported: boolean; registered: boolean } | null>(null);
  const refreshAutostart = () => invoke<{ supported: boolean; registered: boolean }>("autostart_status")
    .then(setAutostart).catch(() => setAutostart(null));
  const [microphones, setMicrophones] = useState<Microphone[] | null>(null);
  const refreshMicrophones = () => invoke<Microphone[]>("list_input_devices")
    .then(setMicrophones).catch(() => setMicrophones([]));
  const [modelProgress, setModelProgress] = useState<CudaProgress | null>(null);
  // For the punctuation choice: is there an NVIDIA GPU? (Ollama is checked by OllamaGuide.)
  const [nvidiaGpu, setNvidiaGpu] = useState<boolean | null>(null);
  useEffect(() => {
    void invoke<boolean>("nvidia_gpu_present").then(setNvidiaGpu).catch(() => setNvidiaGpu(null));
  }, []);
  const [modelOutcome, setModelOutcome] = useState<CudaOutcome>(null);

  async function refreshModels() {
    try {
      setModels(await invoke<ModelAvailability[]>("model_inventory"));
    } catch {
      // Keep the previous view; the next refresh or restart shows the real state.
    }
  }

  function startModelInstall(source: string | null) {
    setModelOutcome(null);
    invoke("install_model", { source, preset: null })
      .then(() => setModelInstalling(true))
      .catch((error) => setNotice(`モデルの導入を開始できません: ${String(error)}`));
  }

  async function checkWorker() {
    setBusy(true);
    setStatus("ワーカーを確認中…");
    try {
      const result = await invoke<WorkerProbe>("probe_worker");
      setWorker(result);
      setStatus("起動確認済み");
    } catch (error) {
      setWorker(null);
      setStatus(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function loadSettings() {
    try {
      const response = await invoke<SettingsResponse>("get_settings");
      setSettings(response.settings);
      setSaved(response.settings);
    } catch (error) {
      setNotice(`設定を読み込めません: ${String(error)}`);
    }
  }

  async function saveSettings() {
    if (!settings) return;
    const previousPreset = saved?.recognition.preset;
    setSettingsBusy(true);
    setSaveResult("");
    try {
      const saved = await invoke<Settings>("save_settings", {
        settings,
        expectedRevision: settings.revision,
      });
      setSettings(saved);
      setSaved(saved);
      void refreshAutostart();
      void invoke<HotkeyStatus>("hotkey_status").then(setHotkeys).catch(() => undefined);
      const prepared = captureEnabled && saved.recognition.mode !== "cloud"
        && saved.recognition.preset !== previousPreset;
      const message = prepared ? `設定を保存しました。${presetLabel(saved.recognition.preset)}の準備ができました` : "設定を保存しました";
      setNotice(message);
      setSaveResult(message);
    } catch (error) {
      setNotice(`設定を保存できません: ${String(error)}`);
      setSaveResult(`保存できませんでした: ${String(error)}。編集中の内容は残してあります。もう一度「保存して適用」を押すと、その内容で保存します`);
      // Keep what the user was editing (a long dictionary is not worth losing to a refused
      // save); only learn what is saved now, and its revision so the next save is accepted.
      try {
        const latest = await invoke<SettingsResponse>("get_settings");
        setSaved(latest.settings);
        setSettings((current) => (current ? { ...current, revision: latest.settings.revision } : current));
      } catch { /* the next save will report it */ }
    } finally {
      setSettingsBusy(false);
    }
  }

  async function switchMode(mode: RecognitionModeValue) {
    if (!saved || !settings) return;
    setSettingsBusy(true);
    try {
      const result = await invoke<Settings>("save_settings", {
        settings: { ...saved, recognition: { ...saved.recognition, mode } },
        expectedRevision: saved.revision,
      });
      setSaved(result);
      setSettings({ ...settings, revision: result.revision, recognition: { ...settings.recognition, mode } });
      setNotice(mode === "cloud" ? "「クラウドで認識」に切り替えました" : "「PC内で認識」に切り替えました");
    } catch (error) {
      setNotice(`切り替えできません: ${String(error)}`);
    } finally {
      setSettingsBusy(false);
    }
  }

  useEffect(() => {
    if (!saveResult) return;
    const timer = window.setTimeout(() => setSaveResult(""), 10_000);
    return () => window.clearTimeout(timer);
  }, [saveResult]);

  useEffect(() => {
    if (!notice) return;
    setLastNotice(notice);
    const timer = window.setTimeout(() => setNotice(""), 10_000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  useEffect(() => {
    void getVersion().then(setAppVersion).catch(() => undefined);
    void checkWorker();
    void loadSettings();
    let active = true;
    const unlisten: Array<() => void> = [];
    void (async () => {
      const off = await listen<string>("user_notice", (event) => {
        if (active) setNotice(event.payload);
      });
      if (!active) { off(); return; }
      unlisten.push(off);
      const offCapture = await listen<"record" | "repaste" | "polish" | "compose" | "english">("shortcut_captured", (event) => {
        const field = capturing.current;
        const saved = registered.current;
        const key = event.payload === "polish" ? savedPolishKey.current
          : event.payload === "compose" ? savedComposeKey.current
            : event.payload === "english" ? savedEnglishKey.current : saved?.[event.payload];
        if (!active || !field || !key) return;
        setSettings((current) => {
          if (!current) return current;
          if (field === "polish") return current.polish ? { ...current, polish: { ...current.polish, shortcut: key } } : current;
          if (field === "compose") return current.polish ? { ...current, polish: { ...current.polish, compose_shortcut: key } } : current;
          return { ...current, shortcuts: { ...current.shortcuts, [field]: key } };
        });
      });
      if (!active) { offCapture(); return; }
      unlisten.push(offCapture);
      // Settings changed outside this window (the memory question's answer).
      const offReloaded = await listen("settings_reloaded", () => { if (active) void loadSettings(); });
      if (!active) { offReloaded(); return; }
      unlisten.push(offReloaded);
      const offEngine = await listen<EngineStatus>("engine_status", (event) => {
        if (active) setEngine(event.payload);
      });
      if (!active) { offEngine(); return; }
      unlisten.push(offEngine);
      const offCuda = await listen<CudaProgress>("cuda_install", (event) => {
        if (!active) return;
        const phase = event.payload.phase;
        if (phase === "installed" || phase === "cancelled" || phase === "failed") {
          setCudaOutcome(phase);
          setCudaProgress(null);
          void invoke<CudaStatus>("cuda_status").then((status) => { if (active) setCuda(status); });
        } else {
          setCudaProgress(event.payload);
          setCuda((current) => current && { ...current, installing: true });
        }
      });
      if (!active) { offCuda(); return; }
      unlisten.push(offCuda);
      const offModel = await listen<CudaProgress>("model_install", (event) => {
        if (!active) return;
        const phase = event.payload.phase;
        if (phase === "installed" || phase === "cancelled" || phase === "failed") {
          setModelOutcome(phase);
          setModelProgress(null);
          setModelInstalling(false);
          void refreshModels();
        } else {
          setModelProgress(event.payload);
          setModelInstalling(true);
        }
      });
      if (!active) { offModel(); return; }
      unlisten.push(offModel);
      const applyDiagnostics = (snapshot: DiagnosticsSnapshot) => {
        if (active) setDiagnostics((current) => newerDiagnostics(current, snapshot));
      };
      const offDiagnostics = await listen<DiagnosticsSnapshot>("diagnostics", (event) => applyDiagnostics(event.payload));
      if (!active) { offDiagnostics(); return; }
      unlisten.push(offDiagnostics);
      try {
        const status = await invoke<HotkeyStatus>("hotkey_status");
        if (active) setHotkeys(status);
        const development = await invoke<boolean>("development_build");
        if (active) setDevelopmentBuild(development);
        const probe = await invoke<boolean>("input_probe_enabled");
        if (active) setProbeEnabled(probe);
        const capture = await invoke<boolean>("capture_probe_enabled");
        if (active) setCaptureEnabled(capture);
        const engine = await invoke<EngineStatus>("engine_status");
        if (active) setEngine(engine);
        const cudaStatus = await invoke<CudaStatus>("cuda_status");
        if (active) setCuda(cudaStatus);
        const models = await invoke<ModelAvailability[]>("model_inventory");
        if (active) setModels(models);
        void refreshAutostart();
        void refreshMicrophones();
        const modelSetup = await invoke<{ installing: boolean }>("model_setup_status");
        if (active) setModelInstalling(modelSetup.installing);
        const snapshot = await invoke<DiagnosticsSnapshot>("diagnostics_snapshot");
        applyDiagnostics(snapshot);
      } catch {
        if (active) setNotice("ホットキー状態を確認できません");
      }
    })();
    return () => { active = false; unlisten.forEach((off) => off()); };
  }, []);

  const cloudMode = saved?.recognition.mode === "cloud";
  const installModel = (preset: "accurate" | "parakeet-ja" | "parakeet-en") => {
    setModelOutcome(null);
    invoke("install_model", { source: null, preset })
      .then(() => setModelInstalling(true))
      .catch((error) => setNotice(`モデルの導入を開始できません: ${String(error)}`));
  };
  const installed = (preset: string) => models.some((model) => model.preset === preset && model.files_present);
  const needsModel = captureEnabled && !cloudMode && !modelInstalling && models.length > 0
    && !models.some((model) => model.preset === "standard" && model.files_present);
  const hotkeyLabel = (registered: boolean, probeText: string, readyText: string, pendingText: string) => hotkeys
    ? registered ? probeEnabled ? probeText : captureEnabled ? readyText : pendingText : "登録競合・利用不可"
    : "確認中";
  const engineLabel = !engine?.enabled ? null : engine.phase === "loading" ? "モデルを検査・準備中…"
    : engine.phase === "ready" ? cloudMode ? "利用可能（クラウド・Groq）" : `利用可能（${engine.device === "cuda" ? "GPU" : "CPU"}）`
    : "利用できません。モデルと通知を確認してください";

  return (
    <main className="shell">
      {/* No system title bar: the header moves the window and has its own buttons. */}
      <header data-tauri-drag-region>
        <div className="mark" data-tauri-drag-region>V</div>
        <div className="header-title" data-tauri-drag-region>
          <p className="eyebrow" data-tauri-drag-region>LOCAL VOICE INPUT · 試用版</p>
          <h1 data-tauri-drag-region>VoiceWeave</h1>
        </div>
        <div className="header-status" role="status" data-tauri-drag-region>
          <p className="header-usage" data-tauri-drag-region>{captureEnabled
            ? `${savedShortcuts?.record ?? "F8"}で録音、もう一度${savedShortcuts?.record ?? "F8"}で貼り付け`
            : "この起動では音声入力を無効にしています"}</p>
          {engineLabel && <p className="header-engine" data-tauri-drag-region>エンジン: {engineLabel}</p>}
        </div>
        <button type="button" className={pinned ? "window-pin pinned" : "window-pin"} aria-pressed={pinned}
          title={pinned ? "常に手前に表示しています（押すと戻す）" : "この画面を常に手前に表示する"}
          aria-label="この画面を常に手前に表示する"
          onClick={() => {
            const next = !pinned;
            void getCurrentWindow().setAlwaysOnTop(next).then(() => setPinned(next)).catch(() => undefined);
          }}>
          <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M9 3h6l-1 6 4 4H6l4-4z" fill={pinned ? "currentColor" : "none"} /><path d="M12 13v8" />
          </svg>
        </button>
        <button type="button" className="window-help" title="使い方のマニュアルをブラウザーで開きます"
          onClick={() => void invoke("open_manual").catch((error) => setNotice(String(error)))}>使い方</button>
        <button type="button" className="window-close" title="閉じてもトレイに残ります"
          onClick={() => void getCurrentWindow().close()}>閉じる</button>
      </header>
      <nav className="tabs" role="tablist" aria-label="設定の種類">
        {tabs.map(([id, label]) => <button key={id} type="button" role="tab" id={`tab-${id}`}
          aria-selected={tab === id} aria-controls={`page-${id}`} className={tab === id ? "tab active" : "tab"}
          onClick={() => setTab(id)}>{label}{id === "setup" && needsModel && <span className="tab-badge" aria-label="要対応">!</span>}</button>)}
      </nav>
      <div className="page">
      {notice && <p className="notice" role="status">{notice}</p>}

      {tab === "basic" && <div role="tabpanel" id="page-basic" aria-labelledby="tab-basic">
        {saved && captureEnabled && <RecognitionMode
          value={saved.recognition.mode ?? "local"}
          disabled={settingsBusy || engine?.phase === "loading"}
          onSwitch={switchMode}
          groq={groqApi}
        />}
        {settings && captureEnabled && settings.recognition.mode === "cloud" && <section className="panel" aria-labelledby="cloud-hints-title">
          <h2 id="cloud-hints-title">クラウド認識に添えるヒント</h2>
          <label>
            <input type="checkbox" checked={settings.recognition.cloud_hints ?? false} disabled={settingsBusy}
              onChange={(event) => setSettings({ ...settings, recognition: { ...settings.recognition, cloud_hints: event.target.checked } })} />
            辞書の言葉と、画面で見つけた登録語を、聞き取りのヒントとして声と一緒にGroqへ送る
          </label>
          <p className="detail">オンにすると、固有名詞を聞き取りやすくなります。ただし、話していない言葉（辞書に登録した人名など）もGroqへ送られます。初期はオフです。変更したら「保存して適用」を押してください。</p>
        </section>}
        {needsModel && <section className="panel first-run" aria-labelledby="first-run-title">
          <h2 id="first-run-title">はじめに</h2>
          <p>音声入力には認識モデル（約1.6GB）が必要です。「初期設定」から導入すると、話した言葉を入力できるようになります。NVIDIAのGPUがあるPCでは、続けて「GPU高速化」も導入すると、話し終えてから入力されるまでが大きく速くなります。</p>
          <button type="button" onClick={() => setTab("setup")}>初期設定を開く</button>
        </section>}
        {probeEnabled && <p className="notice" role="alert">開発用入力試験が有効です。F8で固定文を現在の入力先へ貼り付け、Ctrl+Alt+Vで再貼り付けします。</p>}
        {settings && <section className="panel" aria-labelledby="basic-title">
          <h2 id="basic-title">ふだん使う設定</h2>
          <fieldset className="shortcut-fields">
            <legend>ショートカット</legend>
            <div className="shortcut-grid">
              <div>
                <ShortcutInput label="録音の開始・停止" value={settings.shortcuts.record} disabled={settingsBusy}
                  onCapture={captureField("record")}
                  onChange={(record) => setSettings({ ...settings, shortcuts: { ...settings.shortcuts, record } })} />
                <p className="detail">いま登録中の {savedShortcuts?.record ?? "F8"}: {hotkeyLabel(hotkeys?.record ?? false, "登録済み（固定文の入力試験）", "登録済み", "登録済み（録音は準備中）")}</p>
              </div>
              <div>
                <ShortcutInput label="直前の結果を再入力" value={settings.shortcuts.repaste} disabled={settingsBusy}
                  onCapture={captureField("repaste")}
                  onChange={(repaste) => setSettings({ ...settings, shortcuts: { ...settings.shortcuts, repaste } })} />
                <p className="detail">いま登録中の {savedShortcuts?.repaste ?? "Ctrl+Alt+V"}: {hotkeyLabel(hotkeys?.repaste ?? false, "登録済み（固定文の再貼り付け試験）", "登録済み", "登録済み（再貼り付けは準備中）")}</p>
              </div>
            </div>
            {sameShortcut && <p className="field-error" role="alert">録音と再入力に同じショートカットは使えません。どちらかを別のキーにしてください。</p>}
            <label>
              <input type="checkbox" checked={settings.shortcuts.english_enabled ?? false} disabled={settingsBusy}
                onChange={(event) => setSettings({ ...settings, shortcuts: { ...settings.shortcuts, english_enabled: event.target.checked } })} />
              英語で話すときのショートカットを使う（試験中）
            </label>
            <p className="detail">英語のときはこのキーで録音します。{cloudMode ? "クラウドの方式なので、Groqへ英語として送ります。" : "標準（turbo）・高精度（large-v3）は、同じモデルで英語として聞きます（追加のモデルは要りません）。Parakeet 日本語は日本語専用のため、英語用のモデル（Parakeet 英語版・CPUで動作、準備に約15秒、メモリ約1GB）を一緒に使います。"}句読点と大文字は認識が付けます。日本語向けの整形（言いよどみの除去・段落分け・F9の整形）は使いません。</p>
            {settings.shortcuts.english_enabled && !cloudMode && settings.recognition.preset === "parakeet-ja" && !installed("parakeet-en") && <div className="alias-row">
              <p className="detail warning">Parakeet 日本語で英語を使うには、英語用のモデル（約0.7GB）の導入が必要です。</p>
              <button type="button" disabled={modelInstalling} onClick={() => installModel("parakeet-en")}>英語のモデルを導入（約0.7GB）</button>
            </div>}
            {settings.shortcuts.english_enabled && <div>
              <ShortcutInput label="英語で録音の開始・停止" value={englishKey} disabled={settingsBusy}
                onCapture={captureField("english")}
                onChange={(english) => setSettings({ ...settings, shortcuts: { ...settings.shortcuts, english } })} />
              <p className="detail">いま登録中の {savedShortcuts?.english ?? "Shift+F8"}: {saved?.shortcuts.english_enabled ? hotkeyLabel(hotkeys?.english ?? false, "登録済み", "登録済み", "登録済み（録音は準備中）") : "保存すると登録します"}</p>
            </div>}
            {englishClash && <p className="field-error" role="alert">英語のショートカットは、ほかのショートカットと別のキーにしてください。</p>}
            <p className="detail">欄をクリックして、使いたいキーを押すと設定できます（例: Ctrl+5）。英字・数字・Spaceには Ctrl・Alt・Win を組み合わせます。ほかのアプリが使っていて登録できないときは以前のまま残します。録音中に Esc を押すと、録音を取り消して何も入力しません（認識中の取り消しはトレイのメニューから）。</p>
            <button type="button" disabled={settingsBusy}
              onClick={() => setSettings({ ...settings, shortcuts: { ...settings.shortcuts, record: "F8", repaste: "Ctrl+Alt+V" } })}>
              既定（F8・Ctrl+Alt+V）に戻す
            </button>
          </fieldset>
          {captureEnabled && <MicrophoneSelect
            value={settings.input_device}
            microphones={microphones}
            disabled={settingsBusy}
            onChange={(input_device) => setSettings({ ...settings, input_device })}
            onRefresh={() => { setMicrophones(null); void refreshMicrophones(); }}
          />}
          <label>
            <input
              type="checkbox"
              checked={settings.remove_fillers ?? false}
              disabled={settingsBusy}
              onChange={(event) => setSettings({ ...settings, remove_fillers: event.target.checked })}
            />
            言いよどみ（「えーと、」「あのー、」など）を取り除く
          </label>
          <p className="detail">文の切れ目に単独で出たものだけを取り除き、「あの山」のような意味のある言葉は残します。</p>
          <label>
            <input
              type="checkbox"
              checked={settings.fix_restatements ?? true}
              disabled={settingsBusy}
              onChange={(event) => setSettings({ ...settings, fix_restatements: event.target.checked })}
            />
            言い直し（「3時、いや4時」など）を、直した方だけにする
          </label>
          <p className="detail">数字と単位、今日・明日などの日、曜日が、同じ種類で言い直されたときだけ働きます。「いやな予感」のような言葉はそのままです。</p>
          <label>
            <input
              type="checkbox"
              checked={settings.tidy_numbers ?? true}
              disabled={settingsBusy}
              onChange={(event) => setSettings({ ...settings, tidy_numbers: event.target.checked })}
            />
            漢数字＋単位を数字にする（「二千二十六年」→「2026年」）
          </label>
          <p className="detail">年・月・日・時・分・円・個などの単位が後ろに付くときだけ変えます。「一緒」「十分」「三日月」のような言葉はそのままです。試験中の機能です。</p>
          <label>
            <input type="checkbox" checked={settings.paragraphs ?? true} disabled={settingsBusy}
              onChange={(event) => setSettings({ ...settings, paragraphs: event.target.checked })} />
            長い文章を、話題の切れ目（「まず」「次に」「ところで」など）で段落に分ける
          </label>
          <label>
            <input type="checkbox" checked={settings.screen_context ?? false} disabled={settingsBusy}
              onChange={(event) => setSettings({ ...settings, screen_context: event.target.checked })} />
            画面の言葉を、聞き取りと整形の手がかりにする（試験中）
          </label>
          <p className="detail">録音を始めたときに、入力先と、画面に見えているウィンドウ（最大3つ）の表示中の文字を読みます（メニュー・タブ・ボタン・パスワード欄は読みません）。ふだんの入力では、用語集と辞書の言葉のうち画面に出ているものだけを聞き取りのヒントにします（認識をPC内で行うときは、PC内だけで使います。認識を「クラウド（Groq）」にしているときは、「クラウド認識に添えるヒント」をオンにした場合だけ、Groqへ送られます）。整形モードで外部のAIを使うときは、画面の文字の一部（最大2000字）も参考として送ります。読んだ文字は保存しません（拾った用語は診断タブで確認できます）。</p>
          {settings.punctuation && <label>
            句読点の補い方（早口で間がなく、句読点のない長い部分だけに使います）
            <select value={settings.punctuation.mode} disabled={settingsBusy}
              onChange={(event) => setSettings({ ...settings, punctuation: { ...settings.punctuation!, mode: event.target.value as "rules" | "local_ai" } })}>
              <option value="rules">規則（標準・速い）</option>
              <option value="local_ai">PC内のAI（試験中・Ollamaが必要{nvidiaGpu === false ? "・このPCではおすすめしません" : ""}）</option>
            </select>
          </label>}
          {settings.punctuation?.mode === "local_ai" && <>
            <label>
              句読点に使うAIモデル
              <select value={settings.punctuation.model} disabled={settingsBusy}
                onChange={(event) => setSettings({ ...settings, punctuation: { ...settings.punctuation!, model: event.target.value } })}>
                {[...new Set(["gemma3:4b", "qwen2.5:7b", settings.punctuation.model])].map((model) => <option key={model} value={model}>{model}</option>)}
              </select>
            </label>
            <label>
              <input type="checkbox" checked={settings.punctuation.auto_rules_when_low_memory ?? true} disabled={settingsBusy}
                onChange={(event) => setSettings({ ...settings, punctuation: { ...settings.punctuation!, auto_rules_when_low_memory: event.target.checked } })} />
              パソコンのメモリが足りないときは、自動で「規則」にしてAIを止める
            </label>
            <p className="detail">メモリの使用率が90%を超える（または空きが約1GB未満）と、その間だけ規則で句読点を打ち、AIをメモリから降ろします。そのような状態が続くと、録音の右下に、設定を「規則」に切り替えるかどうかの確認が1度だけ出ます。</p>
            {settings.polish?.endpoint && <OllamaGuide endpoint={settings.polish.endpoint} model={settings.punctuation.model} />}
            {nvidiaGpu === false && <p className="detail warning">このPCにはNVIDIAのGPUがないため、AIはCPUで動きます。句読点のたびに数秒かかり、録音が途切れやすくなるため、おすすめしません（「規則」で十分です）。</p>}
            <p className="detail">AIは「、」「。」だけを補い、文字が変わった答えは使いません。入力が約1〜1.5秒遅くなります。おすすめは gemma3:4b（qwen2.5:7b はより正確で、さらに約0.5秒遅い）。</p>
            {settings.recognition.preset === "accurate" && <p className="detail warning">高精度（large-v3）とPC内のAIを一緒に使うと、GPUのメモリ（6GB）が足りなくなり、認識が止まることがあります{settings.punctuation.model.startsWith("qwen2.5:7b") ? "（qwen2.5:7b は約5GBを使います）" : ""}。句読点は「規則」にするか、認識を標準（turbo）にしてください。</p>}
          </>}
        </section>}
      </div>}

      {tab === "dictionary" && <div role="tabpanel" id="page-dictionary" aria-labelledby="tab-dictionary">
        {settings && <section className="panel" aria-labelledby="dictionary-title">
          <h2 id="dictionary-title">辞書</h2>
          <DictionaryEditor
            entries={settings.dictionary.entries}
            disabled={settingsBusy}
            onChange={(entries) => setSettings({ ...settings, dictionary: { ...settings.dictionary, entries } })}
            preview={(text, entries) => invoke<DictionaryPreview>("preview_dictionary", { text, entries })}
          />
          <DictionarySuggestions
            entries={settings.dictionary.entries}
            glossary={settings.dictionary.glossary ?? ""}
            disabled={settingsBusy}
            onChange={(entries) => setSettings({ ...settings, dictionary: { ...settings.dictionary, entries } })}
            onGlossaryChange={(glossary) => setSettings({ ...settings, dictionary: { ...settings.dictionary, glossary } })}
          />
        </section>}
        {settings && <section className="panel setup-item" aria-labelledby="glossary-title">
          <h2 id="glossary-title">分野の用語集</h2>
          <p className="detail">よく話す分野の用語を、「、」か改行で区切って書きます。整形モードのAIには毎回この一覧を伝え、この分野の言葉はこの表記を優先させます。「画面の言葉を手がかりにする」をオンにすると、画面に出ている用語だけを聞き取りのヒントにも使います。</p>
          <textarea aria-label="分野の用語集" rows={8} maxLength={4000} disabled={settingsBusy}
            value={settings.dictionary.glossary ?? ""}
            onChange={(event) => setSettings({ ...settings, dictionary: { ...settings.dictionary, glossary: event.target.value } })} />
          <div className="alias-row">
            <button type="button" disabled={settingsBusy} onClick={() => {
              void invoke<string>("glossary_with_defaults", { current: settings.dictionary.glossary ?? "" })
                .then((glossary) => setSettings({ ...settings, dictionary: { ...settings.dictionary, glossary } }));
            }}>初期の用語集から足りない語を追加</button>
            <p className="detail">{(settings.dictionary.glossary ?? "").length} / 4000 文字（追加したあと「保存して適用」で保存）</p>
          </div>
        </section>}
      </div>}

      {tab === "polish" && <div role="tabpanel" id="page-polish" aria-labelledby="tab-polish">
        {settings?.polish && <PolishPanel
          value={settings.polish}
          disabled={settingsBusy}
          clash={polishClash}
          registered={saved?.polish?.enabled ? (hotkeys?.polish ?? null) : null}
          onChange={(polish) => setSettings({ ...settings, polish })}
          onCapture={captureField("polish")}
          composeClash={composeClash}
          composeRegistered={saved?.polish?.enabled && (saved.polish.compose_enabled ?? true) ? (hotkeys?.compose ?? null) : null}
          onCaptureCompose={captureField("compose")}
          listModels={(endpoint) => invoke<string[]>("polish_models", { endpoint })}
          cloud={cloudApi}
        />}
      </div>}

      {tab === "setup" && <div role="tabpanel" id="page-setup" aria-labelledby="tab-setup">
        <section className="panel setup-group" aria-labelledby="setup-title">
          <h2 id="setup-title">初期設定</h2>
          <p className="detail">最初に一度行えば、ふだんは変更しない項目です。</p>
          {cloudMode && <p className="detail warning">いまは「クラウドで認識」です（切り替えは「基本」タブのいちばん上）。ここのモデルは、ネットが使えないときの予備としてだけ使います。</p>}
          <ModelPanel
            standard={models.find((model) => model.preset === "standard")}
            installing={modelInstalling}
            progress={modelProgress}
            outcome={modelOutcome}
            onDownload={() => startModelInstall(null)}
            onImport={(folder) => startModelInstall(folder)}
            onCancel={() => void invoke("cancel_model_install")}
          />
          <GpuPanel
            status={cuda}
            device={engine?.device ?? null}
            engineReady={engine?.phase === "ready"}
            cpuEngine={experimentalPresets.includes(engine?.active_preset ?? "")}
            progress={cudaProgress}
            outcome={cudaOutcome}
            onInstall={() => {
              setCudaOutcome(null);
              invoke("install_cuda")
                .then(() => setCuda((current) => current && { ...current, installing: true }))
                .catch((error) => setNotice(`GPU高速化を開始できません: ${String(error)}`));
            }}
            onCancel={() => void invoke("cancel_cuda_install")}
          />
          {settings && <div className="setup-item">
            <h3>認識プリセット</h3>
            <p className="detail">{captureEnabled
              ? "待機中に保存すると、モデルを検査・準備して認識プリセットを切り替えます。準備に時間がかかる場合があります。"
              : "音声入力が無効のため、設定の保存だけを行います。"}</p>
            {engine?.external_override && <p className="detail">開発用のモデル指定が有効です。プリセット変更には指定を解除してアプリを再起動してください。</p>}
            <select
              aria-label="認識プリセット"
              value={settings.recognition.preset}
              disabled={settingsBusy || engine?.phase === "loading" || engine?.external_override}
              onChange={(event) => setSettings({ ...settings, recognition: { ...settings.recognition, preset: event.target.value } })}
            >
              <option value="standard">標準（turbo）</option>
              <option value="accurate">高精度（large-v3・試験中）</option>
              {(developmentBuild || settings.recognition.preset === "dev-tiny")
                && <option value="dev-tiny">検証用（tiny）</option>}
              {experimentalPresets.filter((preset) => preset === settings.recognition.preset
                || models.some((model) => model.preset === preset && model.files_present))
                .map((preset) => <option key={preset} value={preset}>{presetLabel(preset)}</option>)}
              {!["standard", "accurate", "dev-tiny", ...experimentalPresets].includes(settings.recognition.preset)
                && <option value={settings.recognition.preset}>未対応の保存済みプリセット</option>}
            </select>
            {models.filter((model) => (developmentBuild || model.preset !== "dev-tiny")
              && model.preset !== "parakeet-en"
              && (!experimentalPresets.includes(model.preset) || model.files_present)).map((model) => <p className="detail" key={model.preset}>
              {presetLabel(model.preset)}: {model.files_present ? "必要ファイルあり（準備時に検査）" : "未導入または必要ファイル不足"}
              {model.files_present && (model.location === "managed" ? " · アプリ専用保存先" : " · 開発用保存先")}
            </p>)}
            {cloudMode && <p className="detail">「クラウドで認識」の間は、次の2つ（話しながらの認識・録音中の文字）は使いません。</p>}
            <label>
              <input type="checkbox" checked={settings.streaming ?? true} disabled={settingsBusy || cloudMode}
                onChange={(event) => setSettings({ ...settings, streaming: event.target.checked })} />
              話しながら裏で認識しておく（長く話しても、話し終えてからの待ち時間が増えない）
            </label>
            <p className="detail">入力は話し終えたときに1回だけ。不審な点があれば録音全体を処理し直します。</p>
            <label>
              <input type="checkbox" checked={settings.live_preview ?? false}
                disabled={settingsBusy || cloudMode || !(settings.streaming ?? true)}
                onChange={(event) => setSettings({ ...settings, live_preview: event.target.checked })} />
              話している途中の文字を、録音中の表示に出す（試験中）
            </label>
            <p className="detail">話しかけの部分も約1秒ごとに仮に認識して薄い色で表示し、話の切れ目で確定した文字に置き換えます。表示のためだけで、入力される文章は変わりません（句読点・整形は入力時に付きます）。話している間のGPUの負担が増えます。</p>
            <p className="detail">高精度（large-v3）は標準より誤りが少なめで、認識に約2倍の時間がかかります（約0.4秒 → 約1.0秒）。</p>
            {settings.recognition.preset === "parakeet-ja" && <p className="detail warning">試験中のモデルです。CPUで動き（GPUは使いません）、画面の言葉による2回目の認識などWhisper向けの工夫は使いません。辞書の手がかりも使いません（置き換えは効きます）。</p>}
            {!installed("accurate") && <div className="alias-row">
              <button type="button" disabled={modelInstalling} onClick={() => installModel("accurate")}>高精度モデルを導入（約3GB）</button>
            </div>}
            {!installed("parakeet-ja") && <div className="alias-row">
              <p className="detail">Parakeet 日本語（試験中）を使うには、モデル（約0.6GB）の導入が必要です。導入すると、上の一覧に出ます。CPUで動くので、GPUのないPCでも使えます。</p>
              <button type="button" disabled={modelInstalling} onClick={() => installModel("parakeet-ja")}>Parakeet 日本語のモデルを導入（約0.6GB）</button>
            </div>}
          </div>}
          {settings && <div className="setup-item">
            <h3>Windows起動時</h3>
            <label>
              <input
                type="checkbox"
                checked={settings.autostart}
                disabled={settingsBusy}
                onChange={(event) => setSettings({ ...settings, autostart: event.target.checked })}
              />
              Windows起動時に開始
            </label>
            {autostart && <p className="detail">{!autostart.supported
              ? "開発版はWindowsに登録しません（インストールした版で有効になります）"
              : autostart.registered ? "Windowsに登録済みです" : "Windowsには登録していません"}</p>}
          </div>}
        </section>
      </div>}

      {tab === "diagnostics" && <div role="tabpanel" id="page-diagnostics" aria-labelledby="tab-diagnostics">
        <p className="page-note">うまく動かないときに確認するページです。ふだんは見る必要はありません。</p>
        <section className="panel" aria-labelledby="worker-title">
          <div className="section-heading">
            <h2 id="worker-title">認識ワーカー</h2>
            <button type="button" disabled={busy} onClick={() => void checkWorker()}>再確認</button>
          </div>
          <p className={worker ? "status ok" : "status"}>{status}</p>
          {worker && <p className="detail">バージョン {worker.version}</p>}
          {engine?.enabled && <>
            <p role="status">音声入力エンジン: {engineLabel}</p>
            {engine.phase === "ready" && <p className="detail">使用中: {presetLabel(engine.active_preset)}</p>}
            {engine.phase !== "ready" && engine.phase !== "loading" && <>
              <p className="detail">モデルが導入済みなのに利用できない場合や、認識中に問題が起きた後は、エンジンを準備し直すと復旧できることがあります。</p>
              <button type="button" onClick={() => void invoke("retry_engine")}>認識エンジンを再準備</button>
            </>}
          </>}
        </section>
        {lastNotice && <section className="panel" aria-labelledby="last-notice-title">
          <h2 id="last-notice-title">最後のお知らせ</h2>
          <p>{lastNotice}</p>
          <p className="detail">画面上部や小画面のお知らせは少し経つと消えます。最後のものをここに残します（アプリ終了で消えます）。</p>
        </section>}
        {captureEnabled && <DiagnosticsPanel snapshot={diagnostics} />}
      </div>}

      {settings && tab !== "diagnostics" && <div className={unsaved ? "save-bar unsaved" : "save-bar"}>
        <p role="status">{settingsBusy ? "保存・準備中…" : unsaved ? "未保存の変更があります" : saveResult || "変更はすべて保存済みです"}</p>
        {unsaved && <button type="button" disabled={settingsBusy} onClick={() => saved && setSettings(saved)}>変更を取り消す</button>}
        <button type="button" className="primary" disabled={settingsBusy || !unsaved || sameShortcut || polishClash || composeClash || englishClash || engine?.phase === "loading"} onClick={() => void saveSettings()}>
          {captureEnabled ? "保存して適用" : "設定を保存"}
        </button>
      </div>}
      </div>
      <footer>閉じるとトレイに残ります。トレイから設定を再度開けます。{appVersion && ` · バージョン ${appVersion}${developmentBuild ? "（開発版）" : ""}`}</footer>
    </main>
  );
}

export default App;
