import { useEffect, useState } from "react";
import { ShortcutInput } from "./ShortcutInput";

export type PolishSettings = {
  enabled: boolean;
  shortcut: string;
  model: string;
  endpoint: string;
  append_original: boolean;
  /// "local" (Ollama on this PC), "openai", "anthropic", or "compare" (both cloud services).
  provider?: string;
  openai_model?: string;
  anthropic_model?: string;
  /// What the speaker usually talks about, for the drafting model.
  profile?: string;
  /// Ask the other cloud service when the chosen one is slow (first answer is used).
  fallback?: boolean;
  /// Compose mode: write a text from a spoken request.
  compose_enabled?: boolean;
  compose_shortcut?: string;
  /// Use copied text (an email to answer) as reference in compose mode.
  compose_clipboard?: boolean;
};

type CloudProvider = "openai" | "anthropic";

/// Cloud-service operations; keys go straight to the Windows Credential Manager and are
/// never shown again.
export type CloudApi = {
  keyStatus: () => Promise<Record<CloudProvider, boolean>>;
  saveKey: (provider: CloudProvider, key: string) => Promise<void>;
  deleteKey: (provider: CloudProvider) => Promise<void>;
  listModels: (provider: CloudProvider) => Promise<string[]>;
};

type Props = {
  value: PolishSettings;
  disabled: boolean;
  /// The polish key equals the record or re-paste key.
  clash: boolean;
  /// Whether Windows registered the saved polish key; null when the mode is saved off.
  registered: boolean | null;
  onChange: (value: PolishSettings) => void;
  onCapture: (active: boolean) => void;
  /// The compose key equals another shortcut.
  composeClash?: boolean;
  /// Whether Windows registered the saved compose key; null when compose is saved off.
  composeRegistered?: boolean | null;
  onCaptureCompose?: (active: boolean) => void;
  listModels: (endpoint: string) => Promise<string[]>;
  cloud?: CloudApi;
};

type ModelList = { state: "idle" } | { state: "checking" } | { state: "ok"; models: string[] } | { state: "error"; message: string };

const services: Record<CloudProvider, { name: string; model: "openai_model" | "anthropic_model"; fallback: string; hint: string }> = {
  openai: { name: "OpenAI（GPT-6 Luna など）", model: "openai_model", fallback: "gpt-6-luna", hint: "sk-" },
  anthropic: { name: "Anthropic（Claude Haiku 5.5 など）", model: "anthropic_model", fallback: "claude-haiku-5-5", hint: "sk-ant-" },
};

/// One cloud service: its API key (saved, never shown) and the model to use.
function CloudService({ provider, value, disabled, cloud, saved, onSaved, onChange }: {
  provider: CloudProvider; value: PolishSettings; disabled: boolean; cloud: CloudApi; saved: boolean;
  onSaved: () => void; onChange: (value: PolishSettings) => void;
}) {
  const service = services[provider];
  const [key, setKey] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [list, setList] = useState<ModelList>({ state: "idle" });
  const model = value[service.model] ?? service.fallback;
  const known = list.state === "ok" ? list.models : [];
  const choices = known.includes(model) ? known : [model, ...known];
  const check = () => {
    setList({ state: "checking" });
    cloud.listModels(provider)
      .then((models) => setList({ state: "ok", models }))
      .catch((error) => setList({ state: "error", message: String(error) }));
  };
  return <div className="setup-item">
    <h3>{service.name}</h3>
    <p className="detail">APIキー: {saved ? "登録済み（Windowsの資格情報に保存。画面には表示しません）" : "未登録"}</p>
    <div className="alias-row">
      <input type="password" autoComplete="off" spellCheck={false} aria-label={`${service.name}のAPIキー`}
        placeholder={saved ? "新しいキーに替えるときだけ貼り付け" : `${service.hint} で始まるキーを貼り付け`}
        value={key} disabled={disabled} onChange={(event) => setKey(event.target.value)} />
      <button type="button" disabled={disabled || !key.trim()} onClick={() => {
        cloud.saveKey(provider, key)
          .then(() => { setKey(""); setMessage("保存しました"); onSaved(); })
          .catch((error) => setMessage(String(error)));
      }}>保存</button>
      {saved && <button type="button" disabled={disabled} onClick={() => {
        cloud.deleteKey(provider).then(() => { setMessage("削除しました"); onSaved(); });
      }}>削除</button>}
    </div>
    {message && <p className="detail" role="status">{message}</p>}
    <div className="alias-row">
      <select aria-label={`${service.name}で使うモデル`} value={model} disabled={disabled}
        onChange={(event) => onChange({ ...value, [service.model]: event.target.value })}>
        {choices.map((name) => <option key={name} value={name}>{name}</option>)}
      </select>
      <button type="button" disabled={disabled || !saved || list.state === "checking"} onClick={check}>
        {list.state === "checking" ? "確認中…" : "接続を確認"}
      </button>
    </div>
    {list.state === "ok" && <p className="detail" role="status">接続できました。使えるモデル: {list.models.length} 個（一覧から選べます）</p>}
    {list.state === "error" && <p className="field-error" role="status">{list.message}</p>}
  </div>;
}

/// Settings for the polishing mode: a model turns the transcript into an email-style
/// draft, and the faithful transcript can be kept below it.
export function PolishPanel({ value, disabled, clash, registered, onChange, onCapture, composeClash = false, composeRegistered = null, onCaptureCompose = () => undefined, listModels, cloud }: Props) {
  const [list, setList] = useState<ModelList>({ state: "idle" });
  const [keys, setKeys] = useState<Record<CloudProvider, boolean>>({ openai: false, anthropic: false });
  const refreshKeys = () => { cloud?.keyStatus().then(setKeys).catch(() => undefined); };
  useEffect(refreshKeys, [cloud]);
  const provider = value.provider ?? "local";
  const check = () => {
    setList({ state: "checking" });
    listModels(value.endpoint)
      .then((models) => setList({ state: "ok", models }))
      .catch((error) => setList({ state: "error", message: String(error) }));
  };
  const known = list.state === "ok" ? list.models : [];
  const choices = known.includes(value.model) ? known : [value.model, ...known];
  const off = disabled || !value.enabled;

  return <section className="panel" aria-labelledby="polish-title">
    <div className="tag">試験中</div>
    <h2 id="polish-title">整形モード（メール向けの下書き）</h2>
    <p>別のショートカットで録音すると、書き起こした文をAIがメール向けの文章に整えてから入力します。いつものショートカットで録音したときは、これまでどおり話した言葉に忠実に入力します。</p>
    <p className="detail">整えた文は「下書き」として、送る前に必ず読み直してください。整形できなかったときは、元の文をそのまま入力します。</p>

    <label>
      <input type="checkbox" checked={value.enabled} disabled={disabled}
        onChange={(event) => onChange({ ...value, enabled: event.target.checked })} />
      整形モードを使う
    </label>

    <fieldset className="shortcut-fields" disabled={!value.enabled}>
      <legend>整形モードで録音するショートカット</legend>
      <ShortcutInput label="録音の開始・停止（整形して入力）" value={value.shortcut} disabled={off}
        onCapture={onCapture} onChange={(shortcut) => onChange({ ...value, shortcut })} />
      {registered !== null && <p className="detail">いま登録中の {value.shortcut}: {registered ? "登録済み" : "登録競合・利用不可"}</p>}
      {clash && <p className="field-error" role="alert">録音・再入力と同じキーは使えません。別のキーにしてください。</p>}
      <p className="detail">録音の止め方は通常の録音と同じで、どちらのショートカットでも止められます。</p>
    </fieldset>

    <fieldset className="shortcut-fields" disabled={!value.enabled}>
      <legend>作文モード（話した依頼から文章を作る）</legend>
      <p className="detail">書いてほしい内容を、順番や言い回しを気にせず話すと、AIがそのまま送れる文章（メール・チャット・お知らせなど）に書き上げます。「山田さんにメールで、会議を水曜に変えたいと、丁寧めに」のように、相手や文の種類も話の中で伝えられます。話していない日時・数字・約束は作らず、足りない情報は「【要確認：…】」と残します。使うAIは上の「整形に使うAI」と同じです。</p>
      <label>
        <input type="checkbox" checked={value.compose_enabled ?? true} disabled={off}
          onChange={(event) => onChange({ ...value, compose_enabled: event.target.checked })} />
        作文モードを使う
      </label>
      <ShortcutInput label="作文モードの録音を始める" value={value.compose_shortcut ?? "Shift+F9"} disabled={off || !(value.compose_enabled ?? true)}
        onCapture={onCaptureCompose} onChange={(compose_shortcut) => onChange({ ...value, compose_shortcut })} />
      {composeRegistered !== null && <p className="detail">いま登録中の {value.compose_shortcut ?? "Shift+F9"}: {composeRegistered ? "登録済み" : "登録競合・利用不可"}</p>}
      {composeClash && <p className="field-error" role="alert">録音・再入力・整形と同じキーは使えません。別のキーにしてください。</p>}
      <p className="detail">止めるときは、録音・整形のショートカット（F8・F9など）のどれを押しても止まり、作文が始まります。Escで取り消せます。</p>
      <p className="detail">作ってから10分以内にもう一度このキーで録音して「もう少し短く」「敬語を強めに」「〜も入れて」のように話すと、直前の文章を直します。前の文章を選択してから頼むと、直した文章で置き換わります（選択していないとカーソルの位置に追加されます）。</p>
      <label>
        <input type="checkbox" checked={value.compose_clipboard ?? true} disabled={off || !(value.compose_enabled ?? true)}
          onChange={(event) => onChange({ ...value, compose_clipboard: event.target.checked })} />
        コピーした文章（返信したいメールなど）を参考にする
      </label>
      {(value.compose_clipboard ?? true) && <p className="detail">作文モードの録音を始めたときにクリップボードの文章（6,000字まで）を読み、AIへ一緒に送ります。前回このアプリが入力した文章や、パスワード・キーのような短い英数字は使いません。秘密の情報をコピーしたままにしないでください。</p>}
    </fieldset>

    <div className="setup-item">
      <h3>整形に使うAI</h3>
      <select aria-label="整形に使うAI" value={provider} disabled={off}
        onChange={(event) => onChange({ ...value, provider: event.target.value })}>
        <option value="local">このPC内のAI（無料・文章はPCの外に出ない）</option>
        <option value="openai">OpenAI（外部・有料）</option>
        <option value="anthropic">Anthropic（外部・有料）</option>
        <option value="compare">比べる：OpenAI と Anthropic の両方で整形して並べる（試験用）</option>
      </select>
      {provider !== "local" && <p className="detail">外部のAIを選ぶと、整形モードで録音した文章（文字だけ。音声は送りません）がそのサービスへ送られ、使った分だけ料金がかかります（1回あたり約0.06円、比べるときは2回分）。いつものショートカットの録音は、この整形には送りません（ただし、認識を「クラウド（Groq）」にしている場合は、声がGroqへ送られます。辞書・画面の登録語は、設定でオンにしたときだけ送られます）。</p>}
      {(provider === "openai" || provider === "anthropic") && <label>
        <input type="checkbox" checked={value.fallback ?? true} disabled={off}
          onChange={(event) => onChange({ ...value, fallback: event.target.checked })} />
        応答が遅いときは、もう一方のAIにも頼み、先に届いた方を使う
      </label>}
      {(provider === "openai" || provider === "anthropic") && (value.fallback ?? true) && <p className="detail">選んだAIが一定時間（短い文で6秒、長い文ほど長く、最大20秒）答えないときだけ、もう一方のAIにも同じ文章を送ります（キーを保存している場合）。もう一方が書いたときは画面でお知らせします。その回は料金が2回分かかることがあります。</p>}
    </div>

    {provider === "local" && <div className="setup-item">
      <h3>このPC内のAIのモデル</h3>
      <div className="alias-row">
        <select aria-label="整形に使うモデル" value={value.model} disabled={off}
          onChange={(event) => onChange({ ...value, model: event.target.value })}>
          {choices.map((model) => <option key={model} value={model}>{model}</option>)}
        </select>
        <button type="button" disabled={disabled || list.state === "checking"} onClick={check}>
          {list.state === "checking" ? "確認中…" : "Ollama を確認"}
        </button>
      </div>
      {list.state === "ok" && <p className="detail" role="status">{list.models.length
        ? `Ollama に接続できました。導入済みのモデル: ${list.models.length} 個`
        : "Ollama に接続できましたが、モデルがありません。Ollama でモデルを取得してください"}</p>}
      {list.state === "error" && <p className="field-error" role="status">{list.message}</p>}
      <p className="detail">ふだん使いは qwen2.5:7b（速い）。qwen2.5:14b はより正確ですが、1回に数十秒かかり、メモリを多く使います。初回は読み込みに数十秒かかることがあります。</p>
    </div>}

    {provider !== "local" && cloud && (["openai", "anthropic"] as const)
      .filter((service) => provider === "compare" || provider === service)
      .map((service) => <CloudService key={service} provider={service} value={value} disabled={off}
        cloud={cloud} saved={keys[service]} onSaved={refreshKeys} onChange={onChange} />)}

    <div className="setup-item">
      <h3>話し手の分野</h3>
      <textarea aria-label="話し手の分野" rows={2} maxLength={300} disabled={off} value={value.profile ?? ""}
        onChange={(event) => onChange({ ...value, profile: event.target.value })} />
      <p className="detail">よく話す分野や、よく使う種類の言葉を書いておくと、整形のAIが専門用語を正しい字で書きやすくなります（例: 「衝」）。辞書の「使う場面」とあわせて、整形のたびにAIへ伝えます。</p>
    </div>

    <div className="setup-item">
      <h3>入力のしかた</h3>
      <label>
        <input type="checkbox" checked={value.append_original} disabled={off}
          onChange={(event) => onChange({ ...value, append_original: event.target.checked })} />
        整えた文の下に、元の文（話した言葉に忠実な書き起こし）も入力する
      </label>
      <p className="detail">見比べて確かめたあとで、元の文の部分（「――― 元の文 ―――」から下）を消してください。</p>
    </div>
  </section>;
}
