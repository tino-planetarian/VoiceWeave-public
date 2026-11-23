//! Dictionary suggestions from the drafting model's corrections. When a draft writes a
//! glossary or dictionary term where the transcript had something else at the same place
//! (四分木座流星群 → しぶんぎ座流星群), that pair is offered as a dictionary entry: the term
//! with the transcript's spelling as an alias, so plain dictation (F8) writes it right next
//! time. Nothing is registered by itself: the user presses a button for each one. The
//! suggestions live only in memory (never written to disk or logs) and end with the app.

use serde::Serialize;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub term: String,
    pub alias: String,
}

/// Texts longer than this (characters) are not compared: the alignment table grows with
/// the product of the lengths.
const MAX_CHARS: usize = 2_500;
const MAX_KEPT: usize = 20;
const BREAKS: &[char] = &[
    '、', '。', '，', '．', '！', '？', '!', '?', '\n', '\r', '「', '」', '・', ' ', '　',
];

/// For each draft character, the transcript character it was aligned with (longest
/// common subsequence), if any.
fn alignment(original: &[char], draft: &[char]) -> Vec<Option<usize>> {
    let (n, m) = (original.len(), draft.len());
    let width = m + 1;
    let mut table = vec![0u16; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * width + j] = if original[i] == draft[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let mut out = vec![None; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if original[i] == draft[j] {
            out[j] = Some(i);
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The suggestions in one draft: vocabulary terms the draft wrote in place of another
/// spelling in the transcript. Only local corrections count: the characters just before
/// and after the term are the same in both texts, the transcript's spelling has no
/// punctuation, and the term appears nowhere in the transcript.
pub fn find(original: &str, draft: &str, vocabulary: &[String]) -> Vec<Candidate> {
    let o: Vec<char> = original.chars().collect();
    let d: Vec<char> = draft.chars().collect();
    if o.is_empty() || d.is_empty() || o.len() > MAX_CHARS || d.len() > MAX_CHARS {
        return Vec::new();
    }
    let map = alignment(&o, &d);
    let mut out: Vec<Candidate> = Vec::new();
    let mut terms: Vec<&String> = vocabulary.iter().collect();
    // Longest first, so a longer term claims its place before a term inside it.
    terms.sort_by_key(|term| std::cmp::Reverse(term.chars().count()));
    let mut claimed = vec![false; d.len()];
    for term in terms {
        let t: Vec<char> = term.chars().collect();
        if t.len() < 2 || original.contains(term.as_str()) {
            continue;
        }
        let mut start = 0;
        while start + t.len() <= d.len() {
            if d[start..start + t.len()] != t[..] {
                start += 1;
                continue;
            }
            let (s, e) = (start, start + t.len());
            start = e;
            if claimed[s..e].iter().any(|taken| *taken) {
                continue;
            }
            // Punctuation the draft added next to the term does not break the anchor.
            let before = d[..s].iter().rposition(|c| !BREAKS.contains(c));
            let after = d[e..]
                .iter()
                .position(|c| !BREAKS.contains(c))
                .map(|k| e + k);
            let anchored_before = before.is_none_or(|k| map[k].is_some());
            let anchored_after = after.is_none_or(|k| map[k].is_some());
            if !anchored_before || !anchored_after {
                continue;
            }
            let from = match map[s] {
                Some(index) => index,
                None => before.and_then(|k| map[k]).map_or(0, |k| k + 1),
            };
            let to = match map[e - 1] {
                Some(index) => index + 1,
                None => after.and_then(|k| map[k]).unwrap_or(o.len()),
            };
            let (mut from, mut to) = (from, to);
            while from < to && BREAKS.contains(&o[from]) {
                from += 1;
            }
            while from < to && BREAKS.contains(&o[to - 1]) {
                to -= 1;
            }
            if from >= to {
                continue;
            }
            let alias: String = o[from..to].iter().collect();
            let alias_len = to - from;
            if alias == *term
                || alias.contains(BREAKS)
                || alias_len > t.len() * 2 + 2
                || out.iter().any(|known| known.alias == alias)
            {
                continue;
            }
            claimed[s..e].iter_mut().for_each(|taken| *taken = true);
            out.push(Candidate {
                term: term.clone(),
                alias,
            });
        }
    }
    out
}

/// The suggestions waiting for the user, newest first, and the ones they dismissed (not
/// offered again while the app runs).
#[derive(Default)]
pub struct CandidateStore {
    inner: Mutex<(Vec<Candidate>, Vec<Candidate>)>,
}

impl CandidateStore {
    /// Adds new suggestions, skipping aliases the dictionary already has and dismissed
    /// ones. Returns whether anything was added.
    pub fn add(&self, found: Vec<Candidate>, known_aliases: &[String]) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let (waiting, dismissed) = &mut *inner;
        let mut added = false;
        for candidate in found {
            if known_aliases.contains(&candidate.alias)
                || dismissed.contains(&candidate)
                || waiting.contains(&candidate)
            {
                continue;
            }
            waiting.insert(0, candidate);
            added = true;
        }
        waiting.truncate(MAX_KEPT);
        added
    }

    pub fn list(&self) -> Vec<Candidate> {
        self.inner
            .lock()
            .map(|inner| inner.0.clone())
            .unwrap_or_default()
    }

    /// Removes a suggestion (registered or dismissed); a dismissed one is not offered again.
    pub fn remove(&self, candidate: &Candidate, dismissed: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.0.retain(|known| known != candidate);
            if dismissed && !inner.1.contains(candidate) {
                inner.1.push(candidate.clone());
            }
        }
    }
}

/// Recordings in which an unknown name must be on the screen before it is suggested.
pub const SCREEN_SEEN: u32 = 3;
const MAX_COUNTED: usize = 5_000;

/// Names often on the screen while the user dictates (screen reading on), offered for the
/// field glossary. Counts, suggestions and dismissals live only in memory.
#[derive(Default)]
pub struct ScreenWordStore {
    inner: Mutex<ScreenWords>,
}

#[derive(Default)]
struct ScreenWords {
    counts: std::collections::HashMap<String, u32>,
    waiting: Vec<String>,
    dismissed: Vec<String>,
}

impl ScreenWordStore {
    /// Counts the names of one reading (each once). Returns whether a name became a
    /// suggestion.
    pub fn saw(&self, words: &[String]) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.counts.len() > MAX_COUNTED {
            inner.counts.clear();
        }
        let mut added = false;
        for word in words {
            let count = inner.counts.entry(word.clone()).or_insert(0);
            *count += 1;
            if *count == SCREEN_SEEN
                && !inner.dismissed.contains(word)
                && !inner.waiting.contains(word)
            {
                inner.waiting.insert(0, word.clone());
                added = true;
            }
        }
        inner.waiting.truncate(MAX_KEPT);
        added
    }

    pub fn list(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|inner| inner.waiting.clone())
            .unwrap_or_default()
    }

    /// Removes a suggestion (added or dismissed); a dismissed one is not offered again.
    pub fn remove(&self, word: &str, dismissed: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.waiting.retain(|known| known != word);
            if dismissed && !inner.dismissed.iter().any(|known| known == word) {
                inner.dismissed.push(word.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn a_misheard_term_the_draft_corrected_is_suggested() {
        let found = find(
            "一月の初めには四分木座流星群が極大を迎えるよ",
            "1月の初めには、しぶんぎ座流星群が極大を迎えます。",
            &words(&["しぶんぎ座流星群", "極大", "流星群"]),
        );
        assert_eq!(
            found,
            vec![Candidate {
                term: "しぶんぎ座流星群".into(),
                alias: "四分木座流星群".into()
            }]
        );
    }

    #[test]
    fn terms_already_in_the_transcript_or_rewritten_passages_are_not_suggested() {
        let vocabulary = words(&["衝", "木星", "掩蔽"]);
        // Already right in the transcript.
        assert!(find("木星が見えます", "木星が見えます。", &vocabulary).is_empty());
        // A whole sentence rewritten: no anchor on both sides of the term.
        assert!(find("ええとなんだっけあれ", "掩蔽について", &vocabulary).is_empty());
        // The transcript's side crosses punctuation.
        assert!(find("塩、平が", "掩蔽が", &vocabulary).is_empty());
    }

    #[test]
    fn several_corrections_in_one_draft() {
        let found = find(
            "月が星を隠す塩平と、設計と石油の話",
            "月が星を隠す掩蔽と、赤経と赤緯の話",
            &words(&["掩蔽", "赤経", "赤緯"]),
        );
        let pairs: Vec<(&str, &str)> = found
            .iter()
            .map(|c| (c.term.as_str(), c.alias.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![("掩蔽", "塩平"), ("赤経", "設計"), ("赤緯", "石油")]
        );
    }

    #[test]
    fn a_name_seen_in_three_recordings_is_suggested_once() {
        let store = ScreenWordStore::default();
        let seen = words(&["リュウグウ", "Typeless"]);
        assert!(!store.saw(&seen));
        assert!(!store.saw(&words(&["リュウグウ"])));
        assert!(store.saw(&seen));
        assert_eq!(store.list(), words(&["リュウグウ"]));
        assert!(store.saw(&seen));
        assert_eq!(store.list(), words(&["Typeless", "リュウグウ"]));
        store.remove("Typeless", true);
        assert!(!store.saw(&seen));
        assert_eq!(store.list(), words(&["リュウグウ"]));
    }

    #[test]
    fn the_store_skips_known_and_dismissed_suggestions() {
        let store = CandidateStore::default();
        let one = Candidate {
            term: "掩蔽".into(),
            alias: "塩平".into(),
        };
        let two = Candidate {
            term: "赤経".into(),
            alias: "設計".into(),
        };
        assert!(store.add(vec![one.clone(), two.clone()], &words(&["設計"])));
        assert_eq!(store.list(), vec![one.clone()]);
        store.remove(&one, true);
        assert!(!store.add(vec![one.clone()], &[]));
        assert!(store.list().is_empty());
    }
}
