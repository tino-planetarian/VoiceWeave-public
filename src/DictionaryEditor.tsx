import { useState } from "react";

export type DictionaryEntry = { id: string; term: string; aliases: string[]; enabled: boolean; note?: string; hint?: boolean };
export type DictionaryPreview = { text: string; replacements: number; hints: string[] };

type Props = {
  entries: DictionaryEntry[];
  disabled: boolean;
  onChange: (entries: DictionaryEntry[]) => void;
  preview: (text: string, entries: DictionaryEntry[]) => Promise<DictionaryPreview>;
  newId?: () => string;
};

export const MAX_ENTRIES = 200;

/// Aliases typed in one go may be separated by 、 or commas.
export function splitAliases(text: string): string[] {
  return text.split(/[、,，]/).map((alias) => alias.trim()).filter((alias) => alias.length > 0);
}

/// Add aliases to a list, skipping blanks, duplicates and the term itself.
export function addAliases(current: string[], typed: string, term: string): string[] {
  const next = [...current];
  for (const alias of splitAliases(typed)) {
    if (alias !== term.trim() && !next.includes(alias)) next.push(alias);
  }
  return next;
}

type Draft = { id: string | null; term: string; aliases: string[]; aliasInput: string; note: string; hint: boolean };
const emptyDraft: Draft = { id: null, term: "", aliases: [], aliasInput: "", note: "", hint: true };

export function DictionaryEditor({ entries, disabled, onChange, preview, newId }: Props) {
  const [draft, setDraft] = useState<Draft>(emptyDraft);
  const [sample, setSample] = useState("");
  const [result, setResult] = useState<DictionaryPreview | null>(null);
  const editing = draft.id !== null;
  const pendingAliases = addAliases(draft.aliases, draft.aliasInput, draft.term);
  const duplicateTerm = !editing && entries.some((entry) => entry.term === draft.term.trim());
  const canSave = draft.term.trim().length > 0 && !duplicateTerm
    && (editing || entries.length < MAX_ENTRIES);

  const commit = () => {
    if (!canSave) return;
    const entry = {
      id: draft.id ?? (newId ? newId() : crypto.randomUUID()),
      term: draft.term.trim(),
      aliases: pendingAliases,
      enabled: editing ? entries.find((e) => e.id === draft.id)?.enabled ?? true : true,
      note: draft.note.trim(),
      hint: draft.hint,
    };
    onChange(editing ? entries.map((e) => e.id === entry.id ? entry : e) : [...entries, entry]);
    setDraft(emptyDraft);
  };

  return <div className="dictionary">
    <p className="detail">「語」には、正しく入力してほしい表記（固有名詞など）を書きます。認識の手がかりとして使います（目安として合計200文字まで）。「よくある誤認識」には、登録しないと誤って出てしまう書き方を書くと、その語に置き換えます。1つの語にいくつでも登録できます。</p>

    <div className="dictionary-form" aria-label={editing ? "辞書の語を編集" : "辞書に語を登録"}>
      <label>
        語
        <input type="text" value={draft.term} disabled={disabled} maxLength={100} placeholder="例: Claude"
          onChange={(event) => setDraft({ ...draft, term: event.target.value })} />
      </label>
      <label>
        よくある誤認識
        <span className="alias-row">
          <input type="text" value={draft.aliasInput} disabled={disabled} maxLength={100} placeholder="例: クロード"
            onChange={(event) => setDraft({ ...draft, aliasInput: event.target.value })}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                setDraft({ ...draft, aliases: pendingAliases, aliasInput: "" });
              }
            }} />
          <button type="button" disabled={disabled || !draft.aliasInput.trim()}
            onClick={() => setDraft({ ...draft, aliases: pendingAliases, aliasInput: "" })}>追加</button>
        </span>
      </label>
      <label>
        使う場面（任意・整形モードのAIに伝えます）
        <input type="text" value={draft.note} disabled={disabled} maxLength={100}
          placeholder="例: 稟議 → 社内で決裁を求める手続き。書類の名前と一緒に使う"
          onChange={(event) => setDraft({ ...draft, note: event.target.value })} />
      </label>
      <label className="hint-toggle">
        <input type="checkbox" checked={draft.hint} disabled={disabled}
          onChange={(event) => setDraft({ ...draft, hint: event.target.checked })} />
        聞き取りのヒントにも使う
      </label>
      <p className="detail">オフにすると、聞き取りの前のヒントには使わず、置き換えと整形のAIへの説明だけに使います。話していないのに書かれる言葉（珍しい作品名など）や、ふつうの言葉と取り違えやすい言葉（Claude とクラウドなど）はオフがおすすめです。</p>
      {draft.aliases.length > 0 && <ul className="alias-chips" aria-label="追加した誤認識">
        {draft.aliases.map((alias) => <li key={alias}>
          {alias}
          <button type="button" aria-label={`${alias}を外す`} disabled={disabled}
            onClick={() => setDraft({ ...draft, aliases: draft.aliases.filter((a) => a !== alias) })}>×</button>
        </li>)}
      </ul>}
      {duplicateTerm && <p className="detail">この語はすでに登録されています。一覧の「編集」から誤認識を追加してください。</p>}
      <div className="dictionary-actions">
        <button type="button" disabled={disabled || !canSave} onClick={commit}>{editing ? "変更を反映" : "登録"}</button>
        {editing && <button type="button" disabled={disabled} onClick={() => setDraft(emptyDraft)}>編集をやめる</button>}
      </div>
    </div>

    {entries.length === 0
      ? <p className="detail">登録はまだありません。</p>
      : <table className="dictionary-list">
        <thead><tr><th scope="col">語</th><th scope="col">よくある誤認識</th><th scope="col">ヒント</th><th scope="col">使う</th><th scope="col"><span className="visually-hidden">操作</span></th></tr></thead>
        <tbody>{entries.map((entry) => <tr key={entry.id} className={entry.enabled ? undefined : "disabled-entry"}>
          <td>{entry.term}{entry.note && <span className="detail entry-note">{entry.note}</span>}</td>
          <td>{entry.aliases.length ? entry.aliases.join("、") : <span className="detail">（置き換えなし）</span>}</td>
          <td><input type="checkbox" checked={entry.hint ?? true} disabled={disabled || !entry.enabled} aria-label={`${entry.term}を聞き取りのヒントにも使う`}
            onChange={(event) => onChange(entries.map((e) => e.id === entry.id ? { ...e, hint: event.target.checked } : e))} /></td>
          <td><input type="checkbox" checked={entry.enabled} disabled={disabled} aria-label={`${entry.term}を使う`}
            onChange={(event) => onChange(entries.map((e) => e.id === entry.id ? { ...e, enabled: event.target.checked } : e))} /></td>
          <td className="row-actions">
            <button type="button" disabled={disabled}
              onClick={() => setDraft({ id: entry.id, term: entry.term, aliases: entry.aliases, aliasInput: "", note: entry.note ?? "", hint: entry.hint ?? true })}>編集</button>
            <button type="button" disabled={disabled}
              onClick={() => onChange(entries.filter((e) => e.id !== entry.id))}>削除</button>
          </td>
        </tr>)}</tbody>
      </table>}
    <p className="detail">{entries.length} / {MAX_ENTRIES} 件</p>

    <div className="dictionary-preview">
      <label>
        置き換えを試す（保存前の内容で確認できます。この文は保存しません）
        <span className="alias-row">
          <input type="text" value={sample} maxLength={500} onChange={(event) => setSample(event.target.value)} />
          <button type="button" disabled={!sample.trim()} onClick={() => {
            void preview(sample, entries).then(setResult).catch(() => setResult(null));
          }}>試す</button>
        </span>
      </label>
      {result && <p className="detail">結果: {result.text}（置き換え {result.replacements} か所） · ヒント: {result.hints.length ? result.hints.join("、") : "なし"}</p>}
    </div>
  </div>;
}
