import { useState } from "react";

export type KeyPress = {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
};

const MODIFIER_CODES = new Set([
  "ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight",
]);

/// The shortcut text ("Ctrl+Alt+R") for a key press, in the same form the app parses.
/// Returns null while only modifiers are held or for keys the app does not support.
export function shortcutFromKey(press: KeyPress): string | null {
  if (MODIFIER_CODES.has(press.code)) return null;
  let key: string | null = null;
  const letter = /^Key([A-Z])$/.exec(press.code);
  const digit = /^Digit([0-9])$/.exec(press.code);
  const fkey = /^F([1-9]|1[0-9]|2[0-4])$/.exec(press.code);
  if (letter) key = letter[1];
  else if (digit) key = digit[1];
  else if (fkey) key = press.code;
  else if (press.code === "Space") key = "Space";
  else if (press.code === "Pause") key = "Pause";
  else if (press.code === "Insert") key = "Insert";
  if (!key) return null;
  const parts: string[] = [];
  if (press.ctrlKey) parts.push("Ctrl");
  if (press.altKey) parts.push("Alt");
  if (press.shiftKey) parts.push("Shift");
  if (press.metaKey) parts.push("Win");
  return [...parts, key].join("+");
}

/// The modifiers held so far ("Ctrl+Alt+"), shown while the user is still pressing keys.
export function heldModifiers(press: Omit<KeyPress, "code">): string {
  const parts: string[] = [];
  if (press.ctrlKey) parts.push("Ctrl");
  if (press.altKey) parts.push("Alt");
  if (press.shiftKey) parts.push("Shift");
  if (press.metaKey) parts.push("Win");
  return parts.length ? `${parts.join("+")}+` : "";
}

type Props = {
  label: string;
  value: string;
  disabled: boolean;
  onChange: (value: string) => void;
  /// Called when the field starts or stops waiting for keys.
  onCapture?: (active: boolean) => void;
};

/// Press the keys to set the shortcut instead of typing their names.
export function ShortcutInput({ label, value, disabled, onChange, onCapture }: Props) {
  const [held, setHeld] = useState("");
  return <label>
    {label}
    <input
      type="text"
      className={held ? "shortcut-pending" : undefined}
      value={held ? `${held}…` : value}
      readOnly
      disabled={disabled}
      placeholder="ここをクリックしてキーを押す"
      aria-label={`${label}（キーを押して設定）`}
      onFocus={() => onCapture?.(true)}
      onBlur={() => { setHeld(""); onCapture?.(false); }}
      onKeyDown={(event) => {
        if (event.key === "Tab") return; // keep keyboard navigation
        event.preventDefault();
        const shortcut = shortcutFromKey(event);
        if (shortcut) {
          setHeld("");
          onChange(shortcut);
        } else {
          setHeld(heldModifiers(event));
        }
      }}
      onKeyUp={(event) => setHeld(heldModifiers(event))}
    />
  </label>;
}
