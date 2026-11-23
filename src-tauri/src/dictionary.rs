//! The user's dictionary (design §9). A term is a recognition hint; aliases are
//! user-declared misrecognitions replaced by the term after recognition. Replacement is
//! one left-to-right pass, longest alias first, never applied again to its own output.
//! ASCII aliases match whole words only, so "AI" never rewrites part of "MAIL".
use crate::settings::DictionaryEntry;

/// Upper bound for the hint text handed to the recognizer. Approximates the design's
/// 200-token budget: Japanese text is roughly one token per character or more.
pub const HINT_CHAR_LIMIT: usize = 200;

/// Enabled terms meant as hints, in order, without duplicates, while their total length
/// fits the limit. Terms beyond the limit, or with the hint turned off, are left out of
/// the hint (they still apply as replacements and in drafts).
pub fn hints(entries: &[DictionaryEntry]) -> Vec<String> {
    let mut used = 0;
    let mut hints: Vec<String> = Vec::new();
    for entry in entries.iter().filter(|entry| entry.enabled && entry.hint) {
        let term = entry.term.trim();
        let length = term.chars().count();
        if term.is_empty() || hints.iter().any(|hint| hint == term) {
            continue;
        }
        if used + length > HINT_CHAR_LIMIT {
            break;
        }
        used += length;
        hints.push(term.to_owned());
    }
    hints
}

/// All enabled terms, for the drafting model's spelling list (hint or not).
pub fn terms(entries: &[DictionaryEntry]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for entry in entries.iter().filter(|entry| entry.enabled) {
        let term = entry.term.trim();
        if !term.is_empty() && !terms.iter().any(|known| known == term) {
            terms.push(term.to_owned());
        }
    }
    terms
}

/// Upper bound for the dictionary notes sent with every draft (about 0.01 yen per draft
/// with a small cloud model). Notes beyond it are left out.
const NOTES_LIMIT: usize = 600;

/// "term：note" for enabled entries with a note, for the drafting model. All of them are
/// sent (within the limit): the term itself is often exactly what was misheard.
pub fn notes(entries: &[DictionaryEntry]) -> Vec<String> {
    let mut total = 0;
    let mut out = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.enabled && !entry.note.trim().is_empty())
    {
        let line = format!("{}：{}", entry.term.trim(), entry.note.trim());
        total += line.chars().count();
        if total > NOTES_LIMIT {
            break;
        }
        out.push(line);
    }
    out
}

/// Reject an alias that would mean two different terms. Returns the conflicting alias.
pub fn conflicting_alias(entries: &[DictionaryEntry]) -> Option<String> {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for entry in entries {
        for alias in &entry.aliases {
            match seen.iter().find(|(known, _)| *known == alias.as_str()) {
                Some((_, term)) if *term != entry.term.as_str() => return Some(alias.clone()),
                Some(_) => {}
                None => seen.push((alias.as_str(), entry.term.as_str())),
            }
        }
    }
    None
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// Replace declared aliases with their terms. Returns the text and the replacement count.
pub fn apply_aliases(text: &str, entries: &[DictionaryEntry]) -> (String, usize) {
    let mut pairs: Vec<(Vec<char>, &str)> = entries
        .iter()
        .filter(|entry| entry.enabled)
        .flat_map(|entry| {
            entry
                .aliases
                .iter()
                .filter(|alias| !alias.is_empty() && **alias != entry.term)
                .map(move |alias| (alias.chars().collect::<Vec<char>>(), entry.term.as_str()))
        })
        .collect();
    // Longest first, so "音声入力アプリ" wins over "音声入力".
    pairs.sort_by_key(|(alias, _)| std::cmp::Reverse(alias.len()));
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut replaced = 0;
    let mut i = 0;
    while i < chars.len() {
        let matched = pairs.iter().find(|(alias, _)| {
            let end = i + alias.len();
            if end > chars.len() || chars[i..end] != alias[..] {
                return false;
            }
            // Whole-word rule for aliases that start or end with an ASCII letter/digit.
            let starts_word = alias.first().copied().is_some_and(is_word_char);
            let ends_word = alias.last().copied().is_some_and(is_word_char);
            let before_ok = !starts_word || i == 0 || !is_word_char(chars[i - 1]);
            let after_ok = !ends_word || end == chars.len() || !is_word_char(chars[end]);
            before_ok && after_ok
        });
        match matched {
            Some((alias, term)) => {
                output.push_str(term);
                i += alias.len();
                replaced += 1;
            }
            None => {
                output.push(chars[i]);
                i += 1;
            }
        }
    }
    (output, replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_term_without_hint_still_spells_drafts_but_is_not_a_hint() {
        let mut title = entry("チ。-地球の運動について-", &["地球の運動について"], true);
        title.hint = false;
        let entries = vec![entry("VoiceWeave", &[], true), title];
        assert_eq!(hints(&entries), vec!["VoiceWeave".to_owned()]);
        assert_eq!(terms(&entries).len(), 2);
        // Replacement still applies to a term that is not a hint.
        let (text, count) = apply_aliases("地球の運動についての番組", &entries);
        assert_eq!(
            (text.as_str(), count),
            ("チ。-地球の運動について-の番組", 1)
        );
    }

    fn entry(term: &str, aliases: &[&str], enabled: bool) -> DictionaryEntry {
        DictionaryEntry {
            note: String::new(),
            hint: true,
            id: format!("id-{term}"),
            term: term.into(),
            aliases: aliases.iter().map(|alias| (*alias).into()).collect(),
            enabled,
        }
    }

    #[test]
    fn aliases_become_terms_longest_first_in_one_pass() {
        let entries = [
            entry("VoiceWeave", &["ボイスウィーブ", "ボイス ウィーブ"], true),
            entry("音声入力アプリ", &["音声入力あぷり"], true),
            entry("入力", &["にゅうりょく"], true),
        ];
        let (text, count) = apply_aliases("ボイスウィーブは音声入力あぷりです", &entries);
        assert_eq!(text, "VoiceWeaveは音声入力アプリです");
        assert_eq!(count, 2);
        // The output of one replacement is never replaced again.
        let looping = [entry("AB", &["A"], true), entry("C", &["AB"], true)];
        assert_eq!(apply_aliases("A", &looping).0, "AB");
    }

    #[test]
    fn ascii_aliases_match_whole_words_and_disabled_entries_do_nothing() {
        let entries = [
            entry("Claude", &["AI"], true),
            entry("無効", &["テスト"], false),
        ];
        assert_eq!(
            apply_aliases("AI と MAIL と AI2", &entries).0,
            "Claude と MAIL と AI2"
        );
        assert_eq!(apply_aliases("これはテスト", &entries).0, "これはテスト");
        // Japanese aliases match inside running text, as declared.
        let japanese = [entry("議事録", &["ぎじろく"], true)];
        assert_eq!(
            apply_aliases("会議のぎじろくを", &japanese).0,
            "会議の議事録を"
        );
    }

    #[test]
    fn hints_are_enabled_unique_terms_within_the_limit() {
        let long = "長".repeat(150);
        let entries = [
            entry("VoiceWeave", &[], true),
            entry("無効", &[], false),
            entry("VoiceWeave", &[], true),
            entry(&long, &[], true),
            entry(&"後".repeat(60), &[], true),
            entry("次", &[], true),
        ];
        let hints = hints(&entries);
        assert_eq!(hints, vec!["VoiceWeave".to_owned(), long]);
        assert!(hints.iter().map(|hint| hint.chars().count()).sum::<usize>() <= HINT_CHAR_LIMIT);
    }

    #[test]
    fn one_alias_cannot_mean_two_terms() {
        let ok = [entry("A", &["x"], true), entry("A", &["x"], true)];
        assert_eq!(conflicting_alias(&ok), None);
        let bad = [entry("A", &["x"], true), entry("B", &["x"], true)];
        assert_eq!(conflicting_alias(&bad), Some("x".into()));
    }
}
