import { useState } from "react";
import { progressPercent, type CudaOutcome, type CudaProgress } from "./GpuPanel";

export type ModelAvailability = { preset: string; files_present: boolean; location: "managed" | "development" | null };

const phaseLabels: Record<string, string> = {
  starting: "準備しています",
  downloading: "ダウンロード中（約1.6GB。回線によっては数分〜数十分かかります）",
  copying: "モデルをコピー中",
  verifying: "ファイルを検査中",
};

type Props = {
  standard: ModelAvailability | undefined;
  installing: boolean;
  progress: CudaProgress | null;
  outcome: CudaOutcome;
  onDownload: () => void;
  onImport: (folder: string) => void;
  onCancel: () => void;
};

export function ModelPanel({ standard, installing, progress, outcome, onDownload, onImport, onCancel }: Props) {
  const [folder, setFolder] = useState("");
  let body;
  if (installing) {
    const percent = progressPercent(progress);
    const label = phaseLabels[progress?.phase ?? "starting"] ?? "処理中";
    body = <>
      <p role="status">{label}{percent === null ? "…" : `（${percent}%）`}</p>
      <progress max={100} value={percent ?? undefined} aria-label="モデル導入の進み具合" />
      <p className="detail">完了すると、自動で認識エンジンを準備します。</p>
      <p className="detail">取り消しても、ダウンロード中の通信はすぐには止まりません。ダウンロードが終わった時点で取り消しが効き、モデルは導入されません。</p>
      <button type="button" onClick={onCancel}>導入を取り消す</button>
    </>;
  } else if (standard?.files_present && standard.location === "managed") {
    body = <p role="status" className="status ok">標準モデル（turbo）は導入済みです。</p>;
  } else {
    body = <>
      {standard?.files_present
        ? <p>いまは開発用フォルダーのモデルを使っています。アプリ専用の保存先に導入すると、開発用フォルダーがなくても使えます。</p>
        : <p>音声認識には標準モデル（turbo、約1.6GB）が必要です。配布元から取得するか、同じモデルが入ったフォルダーからコピーして導入します。どちらの場合も、ファイルを検査してから使います。</p>}
      <button type="button" onClick={onDownload}>ダウンロードして導入する</button>
      <label className="folder-import">
        コピー元のフォルダー
        <input type="text" value={folder} placeholder="例: D:\models\standard\…" spellCheck={false}
          onChange={(event) => setFolder(event.target.value)} />
      </label>
      <button type="button" disabled={!folder.trim()} onClick={() => onImport(folder.trim())}>このフォルダーからコピーして導入する</button>
    </>;
  }
  return (
    <section className="panel" aria-labelledby="model-title">
      <h2 id="model-title">認識モデル</h2>
      {body}
      {outcome === "failed" && <p role="alert" className="status">導入できませんでした。通信状況・空き容量・コピー元のフォルダーを確認して、もう一度お試しください。</p>}
      {outcome === "cancelled" && <p className="detail">導入を取り消しました。何も変更していません。</p>}
      {outcome === "installed" && <p className="detail">導入しました。認識エンジンを準備しています。</p>}
    </section>
  );
}
