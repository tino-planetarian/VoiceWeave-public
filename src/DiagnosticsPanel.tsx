import { useState } from "react";

export type DiagnosticsSnapshot = {
  revision: number;
  latest: {
    attempt: number;
    model: "standard" | "accurate" | "dev_tiny" | "external" | "cloud" | "parakeet" | "parakeet_en" | "unknown";
    outcome: "sent_unverified" | "copied_only" | "empty" | "cancelled" | "failed";
    timings: Partial<Record<TimingKey, number | null>>;
    startup_xruns: number | null;
    capture_callbacks: number | null;
    input_frames: number | null;
    recognition_path?: "short" | "full" | "short_fallback" | null;
    fallback_reason?: keyof typeof reasonLabels | null;
    fillers_removed?: number | null;
    numbers_tidied?: number | null;
    restatements_fixed?: number | null;
    polish?: keyof typeof polishLabels | null;
    punctuation?: keyof typeof polishLabels | null;
    subtitle_tail_removed?: boolean | null;
    repeats_collapsed?: boolean | null;
    paragraph_breaks?: number | null;
    screen_terms?: string[] | null;
    screen_chars?: number | null;
    stream_parts?: number | null;
    stream_rescued?: number | null;
    topic_passes?: number | null;
    compose_reference_chars?: number | null;
    compose_revision?: boolean | null;
    stream_failure?: string | null;
    cloud_drafts?: CloudDraft[] | null;
    device?: string | null;
    gpu?: GpuMemory | null;
    memory_start?: MemoryState | null;
    memory_end?: MemoryState | null;
    local_model?: string | null;
    recovery?: "retried" | "salvaged" | "lost" | null;
    tentative_previews?: number | null;
    tentative_max_ms?: number | null;
    loudness?: Loudness | null;
    cloud_fallback?: string | null;
    cloud_model?: string | null;
  } | null;
  last_empty?: { attempt: number; loudness: Loudness | null; startup_xruns: number | null } | null;
  last_error: {
    attempt: number;
    stage: string;
    code: string;
    backend_kind: string | null;
    fault_position?: { callback: number; audio_ms: number; xruns: number } | null;
    memory?: MemoryState | null;
    gpu?: GpuMemory | null;
  } | null;
  summary?: {
    attempts: number;
    delivered: number;
    failed: number;
    empty?: number;
    fast_path: number;
    fallback: number;
    full_path: number;
    fallback_reasons: Array<{ reason: keyof typeof reasonLabels; count: number }>;
    stop_to_complete: DurationStats | null;
    cloud?: Array<{ service: string; model: string; ok: number; failed: number; time: DurationStats | null }>;
  };
};

type DurationStats = { count: number; median_ms: number; p90_ms: number; max_ms: number };
type GpuMemory = { used_mb: number; total_mb: number; ollama: boolean; other_programs: number };
type MemoryState = { load_percent: number; available_mb: number; commit_percent: number };
// Same limits as the automatic fall-back to the rules (src-tauri/src/memory_gate.rs).
function memoryLine(memory: MemoryState) {
  const tight = memory.load_percent >= 90 || memory.commit_percent >= 92 || memory.available_mb < 1024;
  return `使用率 ${memory.load_percent}% · 空き ${(memory.available_mb / 1024).toFixed(1)} GB · 仮想メモリ ${memory.commit_percent}%`
    + (tight ? "（メモリ不足。認識が遅くなったり、録音が途切れたりするおそれ）" : "");
}
function memoryLines(latest: { memory_start?: MemoryState | null; memory_end?: MemoryState | null }) {
  const lines: string[] = [];
  if (latest.memory_start) lines.push(`録音開始時のメモリ: ${memoryLine(latest.memory_start)}`);
  if (latest.memory_end) lines.push(`処理完了時のメモリ: ${memoryLine(latest.memory_end)}`);
  return lines;
}
const cloudFallbackLabels: Record<string, string> = {
  no_key: "APIキー未登録", unreachable: "接続できない", timeout: "応答が遅い", unauthorized: "APIキーが無効",
  limited: "利用上限", rejected: "要求を受け付けない", bad_response: "応答を読めない", empty: "応答が空",
};
type Loudness = { peak_db: number; voiced_ms: number; audio_ms: number };
function loudnessLine(loudness: Loudness) {
  const verdict = loudness.peak_db <= -50 ? "（ほぼ無音: マイクの選択・音量・ミュートを確認）"
    : loudness.voiced_ms < 300 ? "（声らしい音がほとんどない）" : "";
  return `録音 ${(loudness.audio_ms / 1000).toFixed(1)} 秒 · いちばん大きい音 ${loudness.peak_db} dB · 声らしい音 ${(loudness.voiced_ms / 1000).toFixed(1)} 秒${verdict}`;
}
const LOW_FREE_MB = 700;
function gpuLine(gpu: GpuMemory) {
  const free = gpu.total_mb - gpu.used_mb;
  const others = gpu.ollama ? "Ollama（ローカルAI）が使用中" : gpu.other_programs > 0 ? `ほかの ${gpu.other_programs} 個のソフトが使用中` : "";
  return `使用 ${(gpu.used_mb / 1024).toFixed(1)} GB / ${(gpu.total_mb / 1024).toFixed(1)} GB · 空き ${(free / 1024).toFixed(1)} GB`
    + (others ? `（${others}）` : "")
    + (free < LOW_FREE_MB ? "（空きが少なく、認識が遅くなったり止まったりするおそれ）" : "");
}
const recoveryLabels = {
  retried: "認識に一度失敗したため、認識エンジンを準備し直して録音全体をもう一度認識しました",
  salvaged: "もう一度の認識にも失敗したため、話している間に認識できた部分をクリップボードにコピーしました（最後の部分は含まれません）",
  lost: "もう一度の認識にも失敗し、話している間に認識できた部分もなかったため、入力していません",
};
type CloudDraft = { service: string; model: string; ms: number; ok: boolean; fallback?: boolean };
const serviceLabels: Record<string, string> = { openai: "OpenAI", anthropic: "Anthropic" };
const service = (name: string) => serviceLabels[name] ?? name;
const streamFailureLabels: Record<string, string> = {
  audio_copy: "録音の写しが追いつかなかった",
  rescue_failed: "怪しい区間の認識し直しに失敗",
  rejected: "区間を認識できなかった",
  worker_error: "認識ワーカーの応答エラー",
  resample: "音声の変換に失敗",
};

function SummaryView({ summary }: { summary: NonNullable<DiagnosticsSnapshot["summary"]> }) {
  if (summary.attempts === 0) return null;
  const stats = summary.stop_to_complete;
  const reasons = summary.fallback_reasons
    .map(({ reason, count }) => `${reasonLabels[reason]} ${count}回`).join("、");
  return <div className="diagnostic-summary">
    <p>起動してからの集計: 試行 {summary.attempts} 回（入力・コピー {summary.delivered} 回 · 失敗 {summary.failed} 回{summary.empty ? ` · 文字にならず ${summary.empty} 回` : ""}）</p>
    <p className="detail">高速処理 {summary.fast_path} 回 · 通常処理に切り替え {summary.fallback} 回
      {summary.full_path > 0 && ` · 通常処理のみ ${summary.full_path} 回`}{reasons && `（理由: ${reasons}）`}</p>
    {stats && <p className="detail">停止から完了まで（直近 {stats.count} 回）: 中央値 {seconds(stats.median_ms)} · 遅い方から1割 {seconds(stats.p90_ms)} · 最大 {seconds(stats.max_ms)}</p>}
    {(summary.cloud ?? []).map((entry) => <p className="detail" key={`${entry.service}/${entry.model}`}>
      外部AIの整形 · {service(entry.service)} {entry.model}: 成功 {entry.ok} 回 · 失敗 {entry.failed} 回
      {entry.time && ` · 中央値 ${seconds(entry.time.median_ms)} · 最大 ${seconds(entry.time.max_ms)}`}</p>)}
  </div>;
}

type TimingKey = "capture_start_ms" | "audio_duration_ms" | "stop_queue_ms"
  | "stream_stop_ms" | "conversion_ms" | "worker_roundtrip_ms" | "worker_total_ms"
  | "worker_frontend_ms" | "worker_decode_ms" | "retry_ms" | "processing_ms" | "delivery_ms"
  | "stop_to_complete_ms" | "polish_ms" | "punctuation_ms";

export function newerDiagnostics(current: DiagnosticsSnapshot | null, incoming: DiagnosticsSnapshot): DiagnosticsSnapshot {
  return current && current.revision > incoming.revision ? current : incoming;
}

// Whisper's fast/full 30-second path does not exist for Parakeet; the row would mislead.
const isParakeet = (model: string) => model === "parakeet" || model === "parakeet_en";
const seconds = (ms: number | null | undefined) => ms == null ? "未計測" : `${(ms / 1000).toFixed(3)} 秒`;
const pathLabels = {
  short: "高速処理（録音の長さだけを処理）",
  full: "通常処理（毎回30秒分を処理）",
  short_fallback: "通常処理（30秒分）に切り替え",
};
const reasonLabels = {
  empty: "結果が空", too_long: "長い録音（25秒超）のため最初から通常処理。失敗ではありません", token_limit: "出力が長すぎる",
  compression: "同じ内容の繰り返し", low_confidence: "認識の確信度が低い",
  no_speech: "音声がない可能性", too_dense: "話した長さに対して文字が多すぎる（繰り返しの疑い）", repeat: "同じ言葉の繰り返しを確かめるため、確実な通常処理で文字起こし", exception: "処理中の例外",
};
const polishLabels = {
  drafted: "下書きを作成", cancelled: "取消", not_local: "接続先がこのPCではない",
  unreachable: "Ollamaに接続できない", timeout: "時間切れ", rejected: "Ollamaがエラーを返した",
  bad_response: "応答を読み取れない", empty: "結果が空", changed: "文字が変わったため不採用",
};
const modelLabels = { standard: "標準（turbo）", accurate: "高精度（large-v3）", dev_tiny: "検証用（tiny）", external: "開発用指定", cloud: "クラウド（Groq）", parakeet: "Parakeet 日本語", parakeet_en: "Parakeet 英語（英語ショートカット）", unknown: "不明" };
const outcomeLabels = {
  sent_unverified: "貼り付けキー送信済み（挿入は未確認）",
  copied_only: "コピーのみ", empty: "認識結果なし", cancelled: "取消", failed: "失敗",
};
const stageLabels: Record<string, string> = {
  capture_start: "録音開始", capture_stop: "録音停止", capture_stream: "音声取得・変換",
  overlay: "録音表示", worker: "認識ワーカー", delivery: "コピー・入力",
};
const rows: Array<[TimingKey, string]> = [
  ["capture_start_ms", "録音開始処理"], ["audio_duration_ms", "取得した音声の長さ"],
  ["stop_queue_ms", "停止要求から停止処理の開始まで"],
  ["stream_stop_ms", "マイクストリームの終了"], ["conversion_ms", "音声確定・変換"],
  ["worker_roundtrip_ms", "認識要求の往復（音声送信・応答処理を含む）"],
  ["worker_total_ms", "ワーカー内の認識全体（往復の内訳）"],
  ["worker_frontend_ms", "音声準備（VADなど・認識全体の内訳）"],
  ["worker_decode_ms", "文字生成（認識全体の内訳）"],
  ["retry_ms", "失敗後のやり直し（エンジンの準備し直しと再認識。上の認識の往復には含まない）"],
  ["processing_ms", "結果の処理"], ["punctuation_ms", "AIによる句読点（その設定のときのみ）"], ["polish_ms", "整形（整形モードのみ）"], ["delivery_ms", "コピー・入力処理"],
];

/** The diagnostics as plain text to paste into a chat. Only numbers, labels and error codes:
 * never the recognized text, audio, dictionary, window titles or screen terms. */
export function diagnosticsText(snapshot: DiagnosticsSnapshot | null): string {
  if (!snapshot) return "診断の記録はまだありません。";
  const lines: string[] = ["【VoiceWeave 診断】"];
  const { summary, last_error: failure, last_empty: empty, latest } = snapshot;
  if (summary && summary.attempts > 0) {
    lines.push(`起動後の集計: 試行 ${summary.attempts} 回 / 入力・コピー ${summary.delivered} / 失敗 ${summary.failed}${summary.empty ? ` / 文字にならず ${summary.empty}` : ""}`);
    lines.push(`高速処理 ${summary.fast_path} / 通常への切替 ${summary.fallback} / 通常のみ ${summary.full_path}`);
    const stats = summary.stop_to_complete;
    if (stats) lines.push(`停止から完了（直近 ${stats.count} 回）: 中央値 ${seconds(stats.median_ms)} / 遅い方1割 ${seconds(stats.p90_ms)} / 最大 ${seconds(stats.max_ms)}`);
  }
  if (failure) {
    lines.push("", `最後のエラー（試行 ${failure.attempt}）: ${stageLabels[failure.stage] ?? failure.stage} / ${failure.code}${failure.backend_kind ? ` / ${failure.backend_kind}` : ""}`);
    if (failure.fault_position) lines.push(`  発生位置: 録音開始から ${(failure.fault_position.audio_ms / 1000).toFixed(2)} 秒 / 音声受信 ${failure.fault_position.callback} 回目の後 / 欠落通知 ${failure.fault_position.xruns} 回`);
    if (failure.memory) lines.push(`  メモリ: 使用率 ${failure.memory.load_percent}% / 空き ${(failure.memory.available_mb / 1024).toFixed(1)} GB / 仮想メモリ ${failure.memory.commit_percent}%`);
    if (failure.gpu) lines.push(`  GPUメモリ: ${gpuLine(failure.gpu)}`);
  }
  if (empty && empty.attempt !== latest?.attempt) {
    lines.push("", `最後に文字にならなかった録音（試行 ${empty.attempt}）${empty.loudness ? `: ${loudnessLine(empty.loudness)}` : ""}`);
  }
  if (latest) {
    lines.push("", `直近（試行 ${latest.attempt}）: ${modelLabels[latest.model]} / ${outcomeLabels[latest.outcome]}`);
    if (latest.recovery) lines.push(recoveryLabels[latest.recovery]);
    if (latest.device) lines.push(`認識に使った装置: ${latest.device === "cloud" ? "クラウド" : latest.device === "cuda" ? "GPU" : "CPU"}`);
    if (latest.cloud_fallback) lines.push(`クラウドで認識できず（${cloudFallbackLabels[latest.cloud_fallback] ?? latest.cloud_fallback}）、PCで認識`);
    if (latest.gpu) lines.push(`録音中のGPUメモリ: ${gpuLine(latest.gpu)}`);
    lines.push(...memoryLines(latest));
    if (latest.local_model) lines.push(`PC内のAI: ${latest.local_model}`);
    if (latest.loudness) lines.push(`音の大きさ: ${loudnessLine(latest.loudness)}`);
    if (latest.stream_parts != null) lines.push(`話しながらの認識: ${latest.stream_parts} 区間を先に処理${latest.stream_rescued ? `（認識し直し ${latest.stream_rescued}）` : ""}`);
    if (latest.stream_failure) lines.push(`話しながらの認識をやめた理由: ${streamFailureLabels[latest.stream_failure] ?? latest.stream_failure}`);
    if (latest.tentative_previews) lines.push(`仮の表示: ${latest.tentative_previews} 回 / 最も遅い回 ${seconds(latest.tentative_max_ms)}`);
    if (latest.punctuation) lines.push(`AIによる句読点: ${latest.punctuation === "drafted" ? "使用" : polishLabels[latest.punctuation]}`);
    if (latest.polish) lines.push(`整形モード: ${polishLabels[latest.polish]}`);
    for (const draft of latest.cloud_drafts ?? []) lines.push(`外部AI ${service(draft.service)} ${draft.model}: ${draft.ok ? seconds(draft.ms) : `失敗（${seconds(draft.ms)}）`}`);
    if (latest.recognition_path && !isParakeet(latest.model)) lines.push(`処理方式: ${pathLabels[latest.recognition_path]}${latest.fallback_reason ? `（${reasonLabels[latest.fallback_reason]}）` : ""}`);
    lines.push(`停止要求から処理完了まで: ${seconds(latest.timings.stop_to_complete_ms)}`);
    for (const [key, label] of rows) if (key !== "retry_ms" || latest.timings.retry_ms != null) lines.push(`  ${label}: ${seconds(latest.timings[key])}`);
    if (latest.startup_xruns) lines.push(`開始時の音声欠落通知: ${latest.startup_xruns} 回`);
    if (latest.capture_callbacks != null) lines.push(`音声コールバック ${latest.capture_callbacks} 回 / 取得フレーム ${latest.input_frames ?? "未計測"}`);
  }
  return lines.join("\n");
}

function CopyDiagnostics({ snapshot }: { snapshot: DiagnosticsSnapshot | null }) {
  const [result, setResult] = useState("");
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(diagnosticsText(snapshot));
      setResult("コピーしました。チャット欄に貼り付けてください。");
    } catch {
      setResult("コピーできませんでした。");
    }
  };
  // Sits in the panel's heading, so it works without opening the panel: the click must
  // not also open or close it.
  return <span className="diagnostics-copy">
    {result && <span className="detail" role="status">{result} </span>}
    <button type="button" onClick={(event) => { event.preventDefault(); event.stopPropagation(); void copy(); }}>診断をまとめてコピー</button>
  </span>;
}

export function DiagnosticsPanel({ snapshot }: { snapshot: DiagnosticsSnapshot | null }) {
  const latest = snapshot?.latest;
  const failure = snapshot?.last_error;
  const empty = snapshot?.last_empty;
  return <details className="panel diagnostics">
    <summary><span>速度・エラー診断</span><CopyDiagnostics snapshot={snapshot} /></summary>
    {/* Scrolls inside its own box, so opening it does not lengthen the window. */}
    <div className="diagnostics-body">
    <p className="detail">直近の処理時間、最後のエラー、件数と時間の集計だけをメモリーに保持します。音声・認識本文は含まず、アプリ終了時に消えます。</p>
    {snapshot?.summary && <SummaryView summary={snapshot.summary} />}
    {failure && <div className="diagnostic-error" role="status">
      <p>最後のエラー · 試行 {failure.attempt}: {stageLabels[failure.stage] ?? failure.stage}</p>
      <p><code>{failure.code}{failure.backend_kind ? ` / ${failure.backend_kind}` : ""}</code></p>
      {failure.fault_position && <p className="detail">
        発生位置: 録音開始から {(failure.fault_position.audio_ms / 1000).toFixed(2)} 秒（{failure.fault_position.callback === 0 ? "音声が届く前" : `${failure.fault_position.callback} 回目の音声受信の後`}）
        · この録音での欠落通知: {failure.fault_position.xruns} 回</p>}
      {failure.memory && <p className="detail">
        そのときのメモリ: 使用率 {failure.memory.load_percent}% · 空き {(failure.memory.available_mb / 1024).toFixed(1)} GB · 仮想メモリの使用 {failure.memory.commit_percent}%
        {(failure.memory.load_percent >= 90 || failure.memory.commit_percent >= 90) && "（メモリ不足の可能性が高い）"}</p>}
      {failure.gpu && <p className="detail">そのときのGPUメモリ: {gpuLine(failure.gpu)}</p>}
      <p className="detail">その後に成功しても、この診断は残ります。</p>
    </div>}
    {empty && empty.attempt !== latest?.attempt && <div className="diagnostic-error" role="status">
      <p>最後に文字にならなかった録音 · 試行 {empty.attempt}</p>
      {empty.loudness && <p className="detail">{loudnessLine(empty.loudness)}</p>}
      {empty.startup_xruns ? <p className="detail">録音開始時の欠落: {empty.startup_xruns} 回</p> : null}
    </div>}
    {latest ? <>
      <p>直近 · 試行 {latest.attempt} · {modelLabels[latest.model]}</p>
      <p>{outcomeLabels[latest.outcome]}</p>
      {latest.recovery && <p className="detail">{recoveryLabels[latest.recovery]}</p>}
      {latest.device && <p className="detail">認識に使った装置: {latest.device === "cloud" ? `クラウド（Groq${latest.cloud_model ? ` · ${latest.cloud_model}` : ""}）` : latest.device === "cuda" ? "GPU" : "CPU（GPUを使えず、認識が遅くなります）"}</p>}
      {latest.cloud_fallback && <p className="detail">クラウドで認識できず（{cloudFallbackLabels[latest.cloud_fallback] ?? latest.cloud_fallback}）、このPCで認識しました</p>}
      {latest.gpu && <p className="detail">録音中のGPUメモリ: {gpuLine(latest.gpu)}</p>}
      {memoryLines(latest).map((line) => <p className="detail" key={line.slice(0, 8)}>{line}</p>)}
      {latest.local_model && <p className="detail">PC内のAI（Ollama）: {latest.local_model} を使用（GPUのメモリを認識と分け合います）</p>}
      {latest.loudness && <p className="detail">音の大きさ: {loudnessLine(latest.loudness)}</p>}
      {latest.tentative_previews ? <p className="detail">録音中の仮の表示: {latest.tentative_previews} 回 · 最も遅かった回 {seconds(latest.tentative_max_ms)}</p> : null}
      {latest.fillers_removed != null && <p className="detail">言いよどみの除去: {latest.fillers_removed} 件</p>}
      {latest.restatements_fixed != null && <p className="detail">言い直しの整理: {latest.restatements_fixed} 件 · 数字の整え: {latest.numbers_tidied ?? 0} 件</p>}
      {latest.stream_parts != null && <p className="detail">話しながらの認識: {latest.stream_parts} 区間を先に処理し、止めたあとは残りだけを処理
        {latest.stream_rescued ? `（うち ${latest.stream_rescued} 区間は丁寧な方法で認識し直し）` : ""}</p>}
      {latest.compose_reference_chars != null && <p className="detail">作文モード: {latest.compose_reference_chars > 0 ? `コピーした文章 ${latest.compose_reference_chars} 字を参考に作文` : "参考の文章なし（話した依頼だけで作文）"}{latest.compose_revision ? "・直前の文章も渡して直しに対応" : ""}</p>}
      {latest.topic_passes ? <p className="detail">話題の手がかり: {latest.topic_passes} 区間を、用語集の同じ話題の用語を手がかりに認識し直し</p> : null}
      {latest.stream_failure && <p className="detail">話しながらの認識をやめた理由: {streamFailureLabels[latest.stream_failure] ?? latest.stream_failure}（録音全体をまとめて処理）</p>}
      {latest.subtitle_tail_removed && <p className="detail">文末の決まり文句（「ご視聴ありがとうございました」など、話していない作り話）を取り除きました</p>}
      {latest.repeats_collapsed && <p className="detail">同じ言葉が何度も続けて書かれた部分（認識の暴走）を1回にまとめました</p>}
      {latest.paragraph_breaks != null && <p className="detail">段落分け: {latest.paragraph_breaks} か所</p>}
      {latest.screen_terms && <p className="detail">画面に出ていた用語（用語集・辞書と一致）: {latest.screen_terms.length
        ? `${latest.screen_terms.length} 語（${latest.screen_terms.join("、")}）`
        : "なし（読み取れないアプリか、用語集・辞書の言葉が画面になかった）"}</p>}
      {latest.screen_chars != null && <p className="detail">整形のAIに参考として渡した画面の文字: {latest.screen_chars} 字</p>}
      {latest.punctuation && <p className="detail">AIによる句読点: {latest.punctuation === "drafted" ? "使用" : `${polishLabels[latest.punctuation]}（規則で補った）`}</p>}
      {latest.polish && <p className="detail">整形モード: {polishLabels[latest.polish]}{latest.polish !== "drafted" && latest.polish !== "cancelled" && "（元の文を入力）"}</p>}
      {latest.cloud_drafts?.map((draft) => <p className="detail" key={`${draft.service}/${draft.model}`}>
        外部AI · {service(draft.service)} {draft.model}{draft.fallback ? "（遅いため頼み直し）" : ""}: {draft.ok ? seconds(draft.ms) : `失敗（${seconds(draft.ms)}）`}</p>)}
      {latest.recognition_path && !isParakeet(latest.model) && <p className="detail">処理方式: {pathLabels[latest.recognition_path]}
        {latest.fallback_reason && `（理由: ${reasonLabels[latest.fallback_reason]}）`}</p>}
      <p className="diagnostic-total">停止要求から処理完了まで: <strong>{seconds(latest.timings.stop_to_complete_ms)}</strong></p>
      <table><thead><tr><th scope="col">工程</th><th scope="col">時間</th></tr></thead>
        <tbody>{rows.filter(([key]) => key !== "retry_ms" || latest.timings.retry_ms != null).map(([key, label]) => <tr key={key}><th scope="row">{label}</th><td>{seconds(latest.timings[key])}</td></tr>)}</tbody>
      </table>
      {latest.startup_xruns != null && latest.startup_xruns > 0
        && <p className="detail">開始時の音声欠落通知（Xrun）: {latest.startup_xruns} 回。実際の欠落量は未確認です。</p>}
      {latest.capture_callbacks != null && <p className="detail">音声コールバック: {latest.capture_callbacks} 回 · 取得フレーム: {latest.input_frames ?? "未計測"}</p>}
      <p className="detail">認識の内訳は重複するため、全行を足しても合計にはなりません。自動停止では停止処理の開始から測定します。取消・失敗の途中工程は未計測になる場合があります。画面描画や入力先での文字挿入完了は測定していません。</p>
    </> : <p>まだ処理時間の記録はありません。</p>}
    </div>
  </details>;
}
