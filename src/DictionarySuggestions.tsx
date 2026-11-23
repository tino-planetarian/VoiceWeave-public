import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DictionaryEntry } from "./DictionaryEditor";

export type Candidate = { term: string; alias: string };

/// The glossary's limit (characters), as in the settings and `glossary::MAX_CHARS`.
const GLOSSARY_MAX_CHARS = 4000;

/// The entries list with `candidate` registered: its spelling becomes an alias of the
/// term (a new entry when the dictionary lacks the term, not used as a recognition hint,
/// since the alias already fixes it and the field glossary covers hints).
export function withCandidate(entries: DictionaryEntry[], candidate: Candidate, newId: () => string): DictionaryEntry[] {
  const existing = entries.find((entry) => entry.term === candidate.term);
  if (existing) {
    if (existing.aliases.includes(candidate.alias)) return entries;
    return entries.map((entry) => entry === existing ? { ...entry, aliases: [...entry.aliases, candidate.alias] } : entry);
  }
  return [...entries, { id: newId(), term: candidate.term, aliases: [candidate.alias], enabled: true, note: "", hint: false }];
}

/// The glossary text with `word` added at the end (unchanged when it already holds it).
export function withGlossaryWord(glossary: string, word: string): string {
  const terms = glossary.split(/[\n\r、,，　 ]/).map((term) => term.trim());
  if (terms.includes(word)) return glossary;
  const trimmed = glossary.trimEnd();
  return trimmed ? `${trimmed}\n${word}` : word;
}

type Props = {
  entries: DictionaryEntry[];
  glossary: string;
  disabled: boolean;
  onChange: (entries: DictionaryEntry[]) => void;
  onGlossaryChange: (glossary: string) => void;
};

/// Corrections the drafting AI made to glossary or dictionary terms, offered as entries,
/// and names often on the screen, offered for the glossary.
/// Kept by the app only while it runs; nothing is registered without a press.
export function DictionarySuggestions({ entries, glossary, disabled, onChange, onGlossaryChange }: Props) {
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [screenWords, setScreenWords] = useState<string[]>([]);

  useEffect(() => {
    let active = true;
    let off: (() => void) | undefined;
    const refresh = () => {
      void invoke<Candidate[]>("dictionary_candidates")
        .then((list) => { if (active) setCandidates(list); })
        .catch(() => undefined);
      void invoke<string[]>("screen_word_candidates")
        .then((list) => { if (active) setScreenWords(list); })
        .catch(() => undefined);
    };
    refresh();
    void listen("dictionary_candidates", refresh).then((unlisten) => {
      if (active) off = unlisten; else unlisten();
    });
    return () => { active = false; off?.(); };
  }, []);

  const remove = (candidate: Candidate, dismissed: boolean) => {
    setCandidates((list) => list.filter((c) => c.term !== candidate.term || c.alias !== candidate.alias));
    void invoke("remove_dictionary_candidate", { ...candidate, dismissed }).catch(() => undefined);
  };

  const removeWord = (word: string, dismissed: boolean) => {
    setScreenWords((list) => list.filter((w) => w !== word));
    void invoke("remove_screen_word_candidate", { word, dismissed }).catch(() => undefined);
  };

  if (candidates.length === 0 && screenWords.length === 0) return null;
  return <div className="dictionary-suggestions" aria-label="辞書の候補">
    {candidates.length > 0 && <>
    <h3>辞書の候補</h3>
    <p className="detail">整形モードのAIが用語を直した箇所です。登録すると、次からふだんの入力でもこの書き方を用語に置き換えます。左側がふだんの言葉（例:「設計」→「赤経」）のときは、どの話でも置き換わるので登録しないでください。登録したあと「保存して適用」で保存します。候補はアプリを閉じると消えます。</p>
    <ul>
      {candidates.map((candidate) => <li key={`${candidate.term}\u0000${candidate.alias}`}>
        <span>「{candidate.alias}」→「{candidate.term}」</span>
        <button type="button" disabled={disabled} onClick={() => {
          onChange(withCandidate(entries, candidate, () => crypto.randomUUID()));
          remove(candidate, false);
        }}>登録</button>
        <button type="button" disabled={disabled} onClick={() => remove(candidate, true)}>いらない</button>
      </li>)}
    </ul>
    </>}
    {screenWords.length > 0 && <>
    <h3>画面によく出る言葉</h3>
    <p className="detail">録音中に画面に何度も出ていて、用語集にも辞書にもない言葉です。用語集に追加すると、画面に出ているときに聞き取りの手がかりになり、整形のAIもこの表記を使います。追加したあと「保存して適用」で保存します。候補はアプリを閉じると消えます。</p>
    <ul>
      {screenWords.map((word) => <li key={word}>
        <span>「{word}」</span>
        <button type="button" disabled={disabled || withGlossaryWord(glossary, word).length > GLOSSARY_MAX_CHARS} onClick={() => {
          onGlossaryChange(withGlossaryWord(glossary, word));
          removeWord(word, false);
        }}>用語集に追加</button>
        <button type="button" disabled={disabled} onClick={() => removeWord(word, true)}>いらない</button>
      </li>)}
    </ul>
    </>}
  </div>;
}
