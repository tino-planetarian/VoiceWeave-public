export type CudaStatus = { installed: boolean; installing: boolean; supported: boolean };
export type CudaProgress = { phase: string; completed: number; total: number };
export type CudaOutcome = "installed" | "cancelled" | "failed" | null;

const phaseLabels: Record<string, string> = {
  starting: "準備しています",
  downloading: "ダウンロード中",
  "checking-wheel": "ダウンロードしたファイルを検査中",
  extracting: "必要なファイルを取り出し中",
  verifying: "取り出したファイルを検査中",
};

export function progressPercent(progress: CudaProgress | null): number | null {
  if (!progress || progress.total <= 0) return null;
  return Math.min(100, Math.floor((progress.completed * 100) / progress.total));
}

type Props = {
  status: CudaStatus | null;
  device: string | null;
  /** The recognition engine is prepared (so `device` says where it runs). */
  engineReady?: boolean;
  /** The selected engine runs on the CPU by design (Parakeet), so a CPU device is no fault. */
  cpuEngine?: boolean;
  progress: CudaProgress | null;
  outcome: CudaOutcome;
  onInstall: () => void;
  onCancel: () => void;
};

export function GpuPanel({ status, device, engineReady = false, cpuEngine = false, progress, outcome, onInstall, onCancel }: Props) {
  let body;
  if (!status) {
    body = <p role="status">確認中…</p>;
  } else if (status.installing) {
    const percent = progressPercent(progress);
    const label = phaseLabels[progress?.phase ?? "starting"] ?? "処理中";
    body = (
      <>
        <p role="status">{label}{percent === null ? "…" : `（${percent}%）`}</p>
        <progress max={100} value={percent ?? undefined} aria-label="GPU高速化の導入の進み具合" />
        <p className="detail">ウィンドウを閉じても導入は続きます。完了すると自動でエンジンを再準備します。</p>
        <button type="button" onClick={onCancel}>導入を取り消す</button>
      </>
    );
  } else if (device === "cuda") {
    body = <p role="status" className="status ok">有効です。GPUで音声認識しています。</p>;
  } else if (cpuEngine && engineReady) {
    body = (
      <p role="status">
        いま選んでいる認識（Parakeet）は、CPUで動く作りです。GPUは使いません（遅いわけではありません）。
        標準（turbo）・高精度（large-v3）を選ぶと、GPUで認識します。
      </p>
    );
  } else if (status.installed && engineReady) {
    body = (
      <p role="status">
        GPU用のライブラリは導入済みですが、今はGPUを使えずCPUで認識しています（認識が遅くなります）。
        GPUのメモリをほかのソフト（Ollamaなど）が使っていないか確認して、エンジンを再準備してください。
      </p>
    );
  } else if (status.installed) {
    body = (
      <p role="status">
        GPU用のライブラリは導入済みです。認識エンジンが準備中か止まっているため、今は使っていません。
        エンジンの準備ができるとGPUで認識します（GPUのメモリが足りない場合はCPUで動作し、認識が遅くなります）。
      </p>
    );
  } else {
    body = (
      <>
        <p>
          GPUを使うと、話し終えてから入力されるまでの時間が大きく短くなります（CPUでは十数秒、GPUでは約2秒の実測）。
          NVIDIAの公式部品（約550MB）を配布元から取得して導入します。取得後はファイルを検査してから使います。
        </p>
        <button type="button" disabled={!status.supported} onClick={onInstall}>GPU高速化を導入する</button>
        {!status.supported && <p className="detail">この版ではまだ導入に対応していません。</p>}
      </>
    );
  }
  return (
    <section className="panel" aria-labelledby="gpu-title">
      <h2 id="gpu-title">GPU高速化</h2>
      {body}
      {outcome === "failed" && <p role="alert" className="status">導入できませんでした。通信状況と空き容量を確認して、もう一度お試しください。</p>}
      {outcome === "cancelled" && <p className="detail">導入を取り消しました。何も変更していません。</p>}
    </section>
  );
}
