export type Microphone = { id: string; name: string; is_default: boolean };
export type InputDevice = { kind: "system_default" } | { kind: "endpoint"; id: string; display_name: string };

type Props = {
  value: InputDevice;
  microphones: Microphone[] | null;
  disabled: boolean;
  onChange: (value: InputDevice) => void;
  onRefresh: () => void;
};

const DEFAULT = "__system_default__";

/// The saved choice stays selectable even while that microphone is unplugged, so
/// saving other settings never silently drops it. Recording then uses the default.
export function MicrophoneSelect({ value, microphones, disabled, onChange, onRefresh }: Props) {
  const current = microphones?.find((mic) => mic.is_default);
  const selected = value.kind === "endpoint" ? value.id : DEFAULT;
  const missing = value.kind === "endpoint" && microphones !== null
    && !microphones.some((mic) => mic.id === value.id);
  return <div className="microphone-select">
    <label>
      マイク
      <select value={selected} disabled={disabled || microphones === null} onChange={(event) => {
        const id = event.target.value;
        if (id === DEFAULT) { onChange({ kind: "system_default" }); return; }
        const mic = microphones?.find((candidate) => candidate.id === id);
        if (mic) onChange({ kind: "endpoint", id: mic.id, display_name: mic.name });
      }}>
        <option value={DEFAULT}>Windowsの既定{current ? `（現在: ${current.name}）` : ""}</option>
        {microphones?.map((mic) => <option key={mic.id} value={mic.id}>{mic.name}</option>)}
        {missing && value.kind === "endpoint"
          && <option value={value.id}>{value.display_name}（見つかりません）</option>}
      </select>
    </label>
    <button type="button" disabled={disabled} onClick={onRefresh}>一覧を更新</button>
    {microphones === null && <p className="detail">マイクの一覧を確認中…</p>}
    {missing && <p className="detail">選択したマイクが見つかりません。接続されるまでは、Windowsの既定のマイクで録音します。</p>}
  </div>;
}
