// Screenshot harness for the user manual: renders the real settings screen in a browser
// with sample (default) settings instead of the Tauri backend. Never part of the app.
import React from "react";
import ReactDOM from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";

const params = new URLSearchParams(location.search);
const scene = params.get("scene") ?? "ready";
const overlay = params.get("overlay");
mockWindows(overlay ? "recording-overlay" : "main");

const fresh = scene === "fresh";
const settings = {
  schema_version: 1, revision: 1,
  shortcuts: { record: "F8", repaste: "Ctrl+Alt+V", english_enabled: false, english: "Shift+F8" },
  recognition: { preset: "standard", mode: "local", cloud_hints: false },
  input_device: { kind: "system_default" }, autostart: false,
  dictionary: {
    entries: [
      { id: "1", term: "Claude", aliases: ["クロード"], enabled: true, note: "", hint: true },
      { id: "2", term: "VoiceWeave", aliases: ["ボイスウィーブ"], enabled: true, note: "", hint: true },
    ],
    glossary: "VoiceWeave、Whisper、Groq、Ollama、ChatGPT\n議事録、見積書、請求書、稟議書、打ち合わせ\n",
  },
  remove_fillers: true, tidy_numbers: true, fix_restatements: true,
  polish: {
    enabled: scene === "polish", shortcut: "F9", model: "qwen2.5:7b", endpoint: "http://127.0.0.1:11434",
    append_original: true, provider: "local", openai_model: "gpt-6-luna", anthropic_model: "claude-haiku-5-5",
    profile: "", fallback: true, compose_enabled: true, compose_shortcut: "Shift+F9", compose_clipboard: false,
  },
  punctuation: { mode: "rules", model: "gemma3:4b", auto_rules_when_low_memory: true },
  paragraphs: true, screen_context: false, streaming: true, live_preview: false,
};

const replies: Record<string, unknown> = {
  "plugin:app|version": "0.3.14",
  probe_worker: { version: "0.3.14" },
  get_settings: { settings, recovered: false },
  hotkey_status: { record: true, repaste: true, polish: scene === "polish", compose: scene === "polish", english: false },
  development_build: false, input_probe_enabled: false, capture_probe_enabled: true,
  engine_status: fresh ? { enabled: true, phase: "failed", active_preset: null, external_override: false, device: null }
    : { enabled: true, phase: "ready", active_preset: "standard", external_override: false, device: "cuda" },
  cuda_status: { installed: !fresh, installing: false, supported: true },
  model_inventory: [
    { preset: "standard", files_present: !fresh, location: fresh ? null : "managed" },
    { preset: "accurate", files_present: false, location: null },
    { preset: "parakeet-ja", files_present: false, location: null },
  ],
  autostart_status: { supported: true, registered: false },
  list_input_devices: [{ id: "mic", name: "マイク (USB Audio Device)", is_default: true }],
  model_setup_status: { installing: false },
  diagnostics_snapshot: { revision: 0, latest: null, last_error: null },
  nvidia_gpu_present: true,
  api_key_status: { openai: false, anthropic: false, groq: false },
  ollama_status: { state: "ready", models: ["qwen2.5:7b", "gemma3:4b"] },
  polish_models: ["qwen2.5:7b", "gemma3:4b"],
  dictionary_candidates: [], screen_word_candidates: [],
  preview_dictionary: { text: "", replacements: 0, hints: [] },
};
mockIPC((cmd, payload) => {
  if (cmd === "save_settings") return (payload as { settings: unknown }).settings;
  return replies[cmd] ?? null;
}, { shouldMockEvents: true });

if (overlay) {
  document.documentElement.classList.add("overlay-page");
  const style = document.createElement("style");
  style.textContent = "*,*::before,*::after{box-sizing:border-box} html.overlay-page{background:#e8ecf2 !important} .overlay-page #root{position:fixed;left:20px;top:20px;width:188px !important;height:54px !important}";
  document.head.appendChild(style);
}
const { default: Root } = overlay ? await import("../../src/RecordingOverlay") : await import("../../src/App");
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<React.StrictMode><Root /></React.StrictMode>);
if (overlay) {
  setTimeout(() => {
    void emit("overlay_mode", "local");
    void emit("overlay_phase", overlay);
    if (overlay === "notice") void emit("overlay_notice", "入力先が変わったため、コピーだけしました");
    const tick = () => { void emit("audio_level", 0.5 + Math.random() * 0.4); };
    setInterval(tick, 120);
  }, 300);
}
const tab = params.get("tab");
if (tab) setTimeout(() => (document.getElementById(`tab-${tab}`) as HTMLButtonElement | null)?.click(), 800);
