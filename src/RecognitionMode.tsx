import { useEffect, useState } from "react";

export type RecognitionModeValue = "local" | "cloud";

export type GroqApi = {
  hasKey: () => Promise<boolean>;
  saveKey: (key: string) => Promise<void>;
  deleteKey: () => Promise<void>;
  /// Checks the key and the connection (lists the speech models the key can use).
  check: () => Promise<string[]>;
};

const modes: Array<{ id: RecognitionModeValue; title: string; points: string[] }> = [
  {
    id: "local",
    title: "PC内で認識",
    points: ["声はこのPCから出ません", "無料・ネット不要", "NVIDIAのGPUがあると速い"],
  },
  {
    id: "cloud",
    title: "クラウドで認識",
    points: ["どんなPCでも速い（約1〜2秒）", "ネットが必要・Groqの無料枠", "声をGroq（米国）に送ります"],
  },
];

/// The main switch: where speech is recognized. Chosen once per PC (home PC with a GPU:
/// this PC; an office laptop without one: the cloud), so it sits above every other
/// setting, looks unlike them, asks before switching and applies at once.
export function RecognitionMode({ value, disabled, onSwitch, groq }: {
  value: RecognitionModeValue;
  disabled: boolean;
  onSwitch: (mode: RecognitionModeValue) => Promise<void>;
  groq: GroqApi;
}) {
  const [asking, setAsking] = useState<RecognitionModeValue | null>(null);
  const [switching, setSwitching] = useState(false);
  const [hasKey, setHasKey] = useState<boolean | null>(null);
  const [key, setKey] = useState("");
  const [keyMessage, setKeyMessage] = useState("");

  const refreshKey = () => groq.hasKey().then(setHasKey).catch(() => setHasKey(null));
  useEffect(() => { void refreshKey(); }, []);

  async function confirm() {
    if (!asking) return;
    setSwitching(true);
    try {
      await onSwitch(asking);
      setAsking(null);
    } finally {
      setSwitching(false);
    }
  }

  async function saveKey() {
    setKeyMessage("");
    try {
      await groq.saveKey(key);
      setKey("");
      await refreshKey();
      setKeyMessage("保存しました。接続を確認しています…");
      const models = await groq.check();
      setKeyMessage(models.length ? "接続できました" : "接続できましたが、音声認識のモデルが見つかりません");
    } catch (error) {
      setKeyMessage(String(error));
    }
  }

  async function check() {
    setKeyMessage("確認しています…");
    try {
      const models = await groq.check();
      setKeyMessage(models.length ? "接続できました" : "接続できましたが、音声認識のモデルが見つかりません");
    } catch (error) {
      setKeyMessage(String(error));
    }
  }

  return <section className="panel mode-switch" aria-labelledby="mode-title">
    <p className="mode-eyebrow">大元の切り替え · PCごとに一度決めれば、ふだんは変えません</p>
    <h2 id="mode-title">認識の方式</h2>
    <div className="mode-cards" role="radiogroup" aria-labelledby="mode-title">
      {modes.map((mode) => <button key={mode.id} type="button" role="radio" aria-checked={value === mode.id}
        className={value === mode.id ? "mode-card selected" : "mode-card"}
        disabled={disabled || switching || value === mode.id}
        onClick={() => setAsking(mode.id)}>
        <span className="mode-state">{value === mode.id ? "● 使用中" : "切り替える"}</span>
        <span className="mode-title">{mode.title}</span>
        <span className="mode-points">{mode.points.map((point) => <span key={point}>{point}</span>)}</span>
      </button>)}
    </div>
    {asking && <div className="mode-confirm" role="alertdialog" aria-labelledby="mode-confirm-title">
      <p id="mode-confirm-title" className="mode-confirm-title">「{asking === "cloud" ? "クラウドで認識" : "PC内で認識"}」に切り替えますか？</p>
      {asking === "cloud" ? <ul>
        <li>録音した声を、止めたあとGroq（米国の会社）に送って文字にします。学習には使われない契約ですが、職場で使う場合は職場のルールを確認してください。</li>
        <li>辞書の言葉や画面の登録語は、初期設定では送りません（切り替えたあとの「クラウド認識に添えるヒント」で、送るかどうかを選べます）。</li>
        <li>ネットがつながらないときは、このPCで認識します（GPUのないPCでは時間がかかります）。声は無駄になりません。</li>
        <li>録音中の文字の表示と、話しながらの認識は使いません（止めたあと1〜2秒で入力される見込みです）。</li>
        <li>このPCの認識モデルはメモリから外し、ネットが失敗したときだけ読み込みます。</li>
        {hasKey === false && <li className="mode-warning">GroqのAPIキーがまだありません。切り替えたあと、下の欄で登録してください（未登録の間はこのPCで認識します）。</li>}
      </ul> : <ul>
        <li>声はこのPCの外に出なくなります。</li>
        <li>このPCの認識モデルを読み込みます（GPUのないPCでは数十秒かかります）。</li>
      </ul>}
      <div className="alias-row">
        <button type="button" className="primary" disabled={switching} onClick={() => void confirm()}>
          {switching ? "切り替え中…" : "切り替える"}</button>
        <button type="button" disabled={switching} onClick={() => setAsking(null)}>やめる</button>
      </div>
    </div>}
    {value === "cloud" && <div className="mode-key">
      <p>GroqのAPIキー: {hasKey === null ? "確認中" : hasKey ? "登録済み（Windowsの資格情報に保存）" : "未登録"}</p>
      <div className="alias-row">
        <input type="password" aria-label="GroqのAPIキー" placeholder={hasKey ? "新しいキーで置き換える" : "gsk_ で始まるキーを貼り付け"}
          value={key} autoComplete="off" onChange={(event) => setKey(event.target.value)} />
        <button type="button" disabled={!key.trim()} onClick={() => void saveKey()}>保存</button>
        {hasKey && <button type="button" onClick={() => void check()}>接続を確認</button>}
        {hasKey && <button type="button" onClick={() => {
          void groq.deleteKey().then(refreshKey).then(() => setKeyMessage("削除しました"));
        }}>削除</button>}
      </div>
      {keyMessage && <p className="detail" role="status">{keyMessage}</p>}
      <p className="detail">キーは console.groq.com の「API Keys」で作れます（無料プランはカード登録なし）。キーは画面に戻らず、Windowsの資格情報にだけ保存します。</p>
    </div>}
  </section>;
}
