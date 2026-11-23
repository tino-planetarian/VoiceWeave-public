import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// What the settings screen tells the user about Ollama (the local AI runtime) and what it
// offers to do. Mirrors src-tauri/src/ollama.rs.
export type OllamaStatus = {
  state: "ready" | "busy" | "not_running" | "not_installed" | "unexpected";
  models: string[];
};

// A failed check is tried again on its own for a while (Ollama may only be starting).
const AUTO_RETRIES = 6;
const RETRY_MS = 5000;
// After "start": how long to keep waiting. A just-started Ollama is silent for about ten
// seconds, and much longer on a PC short of memory.
const START_LIMIT_MS = 120_000;
const START_PAUSE_MS = 2000;

// "qwen2.5" and "qwen2.5:latest" are the same model.
export const hasModel = (models: string[], wanted: string) => {
  const withTag = (name: string) => (name.includes(":") ? name : `${name}:latest`);
  return models.some((model) => withTag(model) === withTag(wanted));
};

const stateLabels: Record<OllamaStatus["state"], string> = {
  ready: "使える", busy: "動いているが返事がない", not_running: "起動していない",
  not_installed: "見つからない", unexpected: "Ollamaではない応答",
};

// Progress of the installer download and of the model download (src-tauri/src/ollama_install.rs).
type Install = { phase: "downloading" | "verifying" | "launched" | "failed" | "cancelled"; megabytes?: number | null; code?: string | null };
type Pull = { phase: "pulling" | "done" | "failed" | "cancelled"; percent?: number | null; code?: string | null };
const installErrors: Record<string, string> = {
  no_curl: "ダウンロードの道具（Windows標準のcurl）が見つかりません",
  download_failed: "ダウンロードできませんでした。ネットワークを確認して、もう一度お試しください",
  signature_invalid: "ダウンロードしたファイルの署名が、Ollamaの発行元のものではなかったため、起動せずに削除しました。公式サイト（ollama.com）から手動で入れてください",
  launch_failed: "インストーラーを起動できませんでした",
};
const pullErrors: Record<string, string> = {
  not_installed: "Ollamaが見つかりません",
  launch_failed: "ダウンロードを始められませんでした",
  pull_failed: "モデルをダウンロードできませんでした。Ollamaが起動しているか、ネットワークを確認してください",
};
const modelSizes: Record<string, string> = { "qwen2.5:7b": "約4.7GB", "gemma3:4b": "約3.3GB" };
const WAIT_FOR_INSTALL_MS = 15 * 60_000;

export function OllamaGuide({ endpoint, model }: { endpoint: string; model: string }) {
  const [status, setStatus] = useState<OllamaStatus | null>(null);
  const [error, setError] = useState("");
  const [checking, setChecking] = useState(false);
  const [checkedAt, setCheckedAt] = useState<Date | null>(null);
  const [starting, setStarting] = useState(false);
  const [note, setNote] = useState("");
  const [install, setInstall] = useState<Install | null>(null);
  const [pull, setPull] = useState<Pull | null>(null);
  const recheck = useRef<() => void>(() => {});
  const alive = useRef(true);
  const startingNow = useRef(false);
  // Only the newest check may change the screen: a slow old one must not overwrite it.
  const latest = useRef(0);

  useEffect(() => {
    alive.current = true;
    let timer: number | undefined;
    let retries = 0;
    const run = async () => {
      if (startingNow.current) return;
      const mine = ++latest.current;
      setChecking(true);
      try {
        const next = await invoke<OllamaStatus>("ollama_status", { endpoint });
        if (!alive.current || mine !== latest.current) return;
        setStatus(next);
        setError("");
        setCheckedAt(new Date());
        if (next.state !== "ready" && retries++ < AUTO_RETRIES) timer = window.setTimeout(() => void run(), RETRY_MS);
      } catch (reason) {
        if (!alive.current || mine !== latest.current) return;
        setStatus(null);
        setError(String(reason));
        setCheckedAt(new Date());
      } finally {
        if (alive.current && mine === latest.current) setChecking(false);
      }
    };
    recheck.current = () => { retries = 0; window.clearTimeout(timer); void run(); };
    void run();
    // Coming back from installing or starting Ollama: look again.
    const onFocus = () => recheck.current();
    window.addEventListener("focus", onFocus);
    return () => { alive.current = false; window.clearTimeout(timer); window.removeEventListener("focus", onFocus); };
  }, [endpoint]);

  // Progress events from the installer download and the model download.
  useEffect(() => {
    let live = true;
    const offs: Array<() => void> = [];
    const keep = (off: () => void) => { if (live) offs.push(off); else off(); };
    void listen<Install>("ollama_install", (event) => { if (live) setInstall(event.payload); }).then(keep);
    void listen<Pull>("ollama_pull", (event) => {
      if (!live) return;
      setPull(event.payload);
      if (event.payload.phase === "done") recheck.current();
    }).then(keep);
    return () => { live = false; offs.forEach((off) => off()); };
  }, []);

  // After the installer was started the user goes through its window, which takes minutes:
  // keep looking until Ollama answers (or a quarter of an hour has passed).
  const launched = install?.phase === "launched";
  const answering = status?.state === "ready";
  useEffect(() => {
    if (!launched || answering) return;
    const until = Date.now() + WAIT_FOR_INSTALL_MS;
    const timer = window.setInterval(() => {
      if (Date.now() > until) window.clearInterval(timer); else recheck.current();
    }, RETRY_MS);
    return () => window.clearInterval(timer);
  }, [launched, answering]);

  async function startInstall() {
    setInstall({ phase: "downloading", megabytes: 0 });
    try {
      await invoke("ollama_install_start");
    } catch (reason) {
      setInstall({ phase: "failed", code: null });
      setNote(String(reason));
    }
  }

  async function startPull() {
    setPull({ phase: "pulling", percent: null });
    try {
      await invoke("ollama_pull_start", { model });
    } catch (reason) {
      setPull({ phase: "failed", code: null });
      setNote(String(reason));
    }
  }

  async function start() {
    if (startingNow.current) return;
    startingNow.current = true;
    latest.current++; // a check begun before this must not change the screen now
    setStarting(true);
    setChecking(false);
    setNote("Ollamaを起動しています。10〜30秒ほどかかります（PCのメモリが足りないときは、もっとかかります）。");
    const began = Date.now();
    let last: OllamaStatus | null = null;
    try {
      try {
        await invoke("ollama_start");
      } catch (reason) {
        setNote(String(reason));
        return;
      }
      while (alive.current && Date.now() - began < START_LIMIT_MS) {
        await new Promise((resolve) => window.setTimeout(resolve, START_PAUSE_MS));
        if (!alive.current) return;
        try {
          last = await invoke<OllamaStatus>("ollama_status", { endpoint });
        } catch { continue; }
        if (last.state === "ready") { setStatus(last); setError(""); setCheckedAt(new Date()); setNote(""); return; }
        const seconds = Math.round((Date.now() - began) / 1000);
        setNote(last.state === "busy"
          ? `Ollamaは起動しました。準備ができるのを待っています（${seconds}秒）。`
          : `Ollamaの起動を待っています（${seconds}秒）。`);
      }
      if (!alive.current) return;
      if (last) { setStatus(last); setCheckedAt(new Date()); }
      setNote(last?.state === "busy"
        ? "Ollamaは起動しましたが、2分待っても応答がありません。PCのメモリが足りない可能性があります（診断タブのメモリを確認してください）。"
        : "Ollamaを起動しようとしましたが、2分待っても応答がありません。タスクトレイにOllamaのアイコンがあるか確認し、なければOllamaを手動で起動してください。");
    } finally {
      startingNow.current = false;
      if (alive.current) setStarting(false);
    }
  }

  const fallback = "このままでは句読点は「規則」で補います。";
  const again = <button type="button" onClick={() => recheck.current()} disabled={starting}>もう一度確認</button>;
  const startButton = <button type="button" onClick={() => void start()} disabled={starting}>{starting ? "起動しています…" : "Ollamaを起動する"}</button>;
  let message: ReactNode = null;
  if (error) {
    message = <p className="detail warning">Ollamaの確認ができませんでした（{error}）。 {again}</p>;
  } else if (!status) {
    message = <p className="detail">Ollamaを確認しています…</p>;
  } else if (status.state === "ready") {
    const pulling = pull?.phase === "pulling";
    message = hasModel(status.models, model)
      ? <p className="detail">Ollamaは使えます（モデル {model} あり）。 {again}</p>
      : <>
        <p className="detail warning">Ollamaは動いていますが、AIモデル「{model}」が入っていません。{fallback} {again}</p>
        <p className="detail">
          {pulling
            ? <>モデルをダウンロードしています{pull?.percent != null ? `（${pull.percent}%）` : "…"}（{modelSizes[model] ?? "数GB"}。数分〜数十分かかります）。 <button type="button" onClick={() => void invoke("ollama_pull_cancel")}>取り消す</button></>
            : <button type="button" onClick={() => void startPull()}>モデル「{model}」をダウンロードする（{modelSizes[model] ?? "数GB"}）</button>}
          {pull?.phase === "failed" && <> {pullErrors[pull.code ?? ""] ?? "ダウンロードできませんでした"}</>}
          {pull?.phase === "cancelled" && " 取り消しました。"}
        </p>
      </>;
  } else if (status.state === "busy") {
    message = <p className="detail warning">Ollamaは動いていますが、返事がありません（起動した直後か、PCのメモリが足りないときに起こります）。{fallback} {again}</p>;
  } else if (status.state === "not_running") {
    message = <p className="detail warning">Ollamaは入っていますが、起動していません。{fallback} {startButton} {again}</p>;
  } else if (status.state === "not_installed") {
    const installing = install?.phase === "downloading" || install?.phase === "verifying";
    message = <>
      <p className="detail warning">Ollamaが見つかりません（このPCに入っていないようです）。{fallback} {again}</p>
      <p className="detail">
        {installing
          ? <>{install?.phase === "verifying" ? "ダウンロードしたファイルの署名を確認しています…" : `ダウンロードしています（${install?.megabytes ?? 0} MB / 約1GB以上）…`} <button type="button" onClick={() => void invoke("ollama_install_cancel")}>取り消す</button></>
          : launched
            ? "インストーラーを起動しました。表示された画面の案内に沿って進めてください。終わると、自動で検出します。"
            : <button type="button" onClick={() => void startInstall()}>Ollamaをインストールする</button>}
        {install?.phase === "failed" && <> {installErrors[install.code ?? ""] ?? "インストールできませんでした"}</>}
        {install?.phase === "cancelled" && " 取り消しました。"}
      </p>
      {!installing && !launched && <p className="detail">公式サイト（ollama.com）から、インストーラー（約1GB以上）をダウンロードし、発行元（Ollama Inc.）の署名を確認してから起動します。署名が合わないファイルは、開かずに削除します。インストーラーの画面は、ご自身で進めてください。セキュリティソフトの確認が出ることがあります。入れたあと、AIモデル（数GB）も、この画面からダウンロードできます。</p>}
    </>;
  } else {
    message = <p className="detail warning">この接続先は、Ollamaではない別のソフトが使っているようです。{fallback} {again}</p>;
  }
  return <>
    {message}
    {note && <p className="detail" role="status">{note}</p>}
    {status && checkedAt && <p className="detail">最後の確認: {checkedAt.toLocaleTimeString("ja-JP")}（{stateLabels[status.state]}）{checking && " · 確認中…"}</p>}
  </>;
}
