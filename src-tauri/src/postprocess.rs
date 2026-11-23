//! Conservative punctuation for fast, unbroken speech.
//!
//! Whisper punctuates from pauses, so a fast speaker gets long runs with no marks.
//! This inserts only `、` and `。` at high-confidence meaning boundaries inside such
//! runs. It never changes, removes or reorders a character, never touches text that
//! already carries punctuation, and leaves anything ambiguous alone.
//! Invariant (tested): removing every `、` and `。` from the result gives the input
//! with the same marks removed.

/// Only runs this long without a mark are candidates; short runs read fine as they are.
const MIN_RUN_CHARS: usize = 24;
/// No fragment shorter than this is created before a mark, and enough text must follow.
const MIN_BEFORE: usize = 8;
const MIN_AFTER: usize = 6;

/// Endings that can close a sentence, followed (without a pause) by a sentence opener.
const SENTENCE_ENDINGS: [&str; 12] = [
    "ください",
    "お願いします",
    "ましょう",
    "でしょう",
    "ですね",
    "ますね",
    "ですよ",
    "ますよ",
    "ません",
    "ました",
    "でした",
    "です",
];
const SENTENCE_ENDINGS_EXTRA: [&str; 1] = ["ます"];

/// Words that open a new sentence, with the follow-on that must not be glued to them.
const OPENERS: [(&str, &[char]); 15] = [
    ("まずは", &[]),
    ("まず", &['は', 'い']),
    ("それから", &[]),
    ("そして", &[]),
    ("次に", &[]),
    ("さらに", &[]),
    ("しかし", &[]),
    ("ただし", &[]),
    ("ところで", &[]),
    ("ちなみに", &[]),
    ("最後に", &[]),
    ("最初に", &[]),
    ("その後", &['ろ']),
    ("つまり", &[]),
    ("また", &['は', 'も', 'ず', 'た']),
];

/// Clause connectives that take a comma, with next characters that show the word is not
/// really a connective here (sentence-final particles, `のです`, `のでは`, ...).
const CONNECTIVES: [(&str, &[char]); 12] = [
    ("のですが", &['ね', 'よ', 'な', 'わ', 'の', 'っ']),
    ("ですが", &['ね', 'よ', 'な', 'わ', 'の', 'っ']),
    ("ますが", &['ね', 'よ', 'な', 'わ', 'の', 'っ']),
    ("でしたが", &['ね', 'よ', 'な', 'わ', 'の', 'っ']),
    ("ましたが", &['ね', 'よ', 'な', 'わ', 'の', 'っ']),
    ("ですけど", &['ね', 'よ', 'な', 'わ', 'も', 'の']),
    ("ますけど", &['ね', 'よ', 'な', 'わ', 'も', 'の']),
    ("ですから", &['ね', 'よ', 'な', 'こ', 'そ']),
    ("ますから", &['ね', 'よ', 'な', 'こ', 'そ']),
    ("ですし", &['ょ', 'ね', 'よ']),
    ("ますし", &['ょ', 'ね', 'よ']),
    ("ので", &['す', 'は', 'も']),
];

const MARKS: [char; 6] = ['、', '。', '？', '！', '?', '!'];

#[derive(Debug, PartialEq, Eq)]
pub struct ProcessedText {
    pub raw: String,
    pub text: String,
    /// Fixed rule identifiers that fired, never any content.
    pub rules: Vec<&'static str>,
}

fn ends_with_at(chars: &[char], end: usize, word: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    end >= word.len() && chars[end - word.len()..end] == word[..]
}

fn starts_with_at(chars: &[char], start: usize, word: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    start + word.len() <= chars.len() && chars[start..start + word.len()] == word[..]
}

/// Punctuate one mark-free run. Returns the run with marks inserted.
fn punctuate_run(chars: &[char], rules: &mut Vec<&'static str>) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(chars.len() + 8);
    // Where the current fragment began, and whether it begins a sentence (a run start or
    // right after an inserted full stop), which is the only place an opener takes a comma.
    let mut fragment_start = 0_usize;
    let mut sentence_start = true;
    for i in 0..=chars.len() {
        if i > 0 && i < chars.len() {
            let before = i - fragment_start;
            let after = chars.len() - i;
            let next = chars[i];
            if sentence_start {
                if let Some((word, blocked)) = OPENERS.iter().find(|(word, _)| {
                    starts_with_at(chars, fragment_start, word)
                        && fragment_start + word.chars().count() == i
                }) {
                    let _ = word;
                    if !blocked.contains(&next) && after >= 2 {
                        out.push('、');
                        rules.push("punct.opening_comma");
                        sentence_start = false;
                    }
                }
            }
            if before >= MIN_BEFORE {
                let closes_sentence = SENTENCE_ENDINGS
                    .iter()
                    .chain(SENTENCE_ENDINGS_EXTRA.iter())
                    .any(|ending| ends_with_at(chars, i, ending))
                    && OPENERS.iter().any(|(word, blocked)| {
                        let len = word.chars().count();
                        starts_with_at(chars, i, word)
                            && chars.get(i + len).is_some_and(|c| !blocked.contains(c))
                            && after >= len + 2
                    });
                if closes_sentence {
                    out.push('。');
                    rules.push("punct.sentence_break");
                    fragment_start = i;
                    sentence_start = true;
                } else if after >= MIN_AFTER
                    && CONNECTIVES.iter().any(|(word, blocked)| {
                        ends_with_at(chars, i, word) && !blocked.contains(&next)
                    })
                {
                    out.push('、');
                    rules.push("punct.clause_comma");
                    fragment_start = i;
                    sentence_start = false;
                }
            }
        }
        if i < chars.len() {
            out.push(chars[i]);
        }
    }
    out
}

/// Insert punctuation into long unmarked runs. Text that Whisper already punctuated is
/// kept exactly; only the long mark-free stretches between its marks are considered.
pub fn punctuate(raw: &str) -> ProcessedText {
    let mut rules = Vec::new();
    let mut result = String::with_capacity(raw.len() + 16);
    let mut run: Vec<char> = Vec::new();
    let flush = |run: &mut Vec<char>, result: &mut String, rules: &mut Vec<&'static str>| {
        if run.len() >= MIN_RUN_CHARS && !run.iter().any(|c| c.is_whitespace() || c.is_ascii()) {
            result.extend(punctuate_run(run, rules));
        } else {
            result.extend(run.iter());
        }
        run.clear();
    };
    for c in raw.chars() {
        if MARKS.contains(&c) || c == '\n' {
            flush(&mut run, &mut result, &mut rules);
            result.push(c);
        } else {
            run.push(c);
        }
    }
    flush(&mut run, &mut result, &mut rules);
    ProcessedText {
        raw: raw.to_owned(),
        text: result,
        rules,
    }
}

/// Stock closing lines Whisper writes over trailing silence (learned from video
/// subtitles). Nobody dictates them, so one standing at the very end is removed.
/// Longest first. Only these exact lines; nothing inside the text is touched.
const SUBTITLE_TAILS: [&str; 7] = [
    "最後までご視聴いただきありがとうございました",
    "最後までご視聴ありがとうございました",
    "ご視聴いただきありがとうございました",
    "チャンネル登録よろしくお願いします",
    "ご視聴ありがとうございました",
    "チャンネル登録お願いします",
    "ご清聴ありがとうございました",
];

/// Texts shorter than this, or with fewer sentences, stay one paragraph. (150 / 4 / 5
/// until 2026-10-23: the user found the paragraphs too long.)
const PARAGRAPH_MIN_CHARS: usize = 100;
const PARAGRAPH_MIN_SENTENCES: usize = 3;
/// A paragraph without any topic word still ends after this many sentences, or at the
/// first sentence end past this many characters (long spoken sentences).
const PARAGRAPH_MAX_SENTENCES: usize = 3;
const PARAGRAPH_MAX_CHARS: usize = 120;
/// Sentence openings that start a new topic. A small local model placed breaks worse
/// than these words did on the same texts (2026-10-10), and rules add no wait.
const TOPIC_STARTS: [&str; 22] = [
    "まず",
    "最初に",
    "はじめに",
    "次に",
    "続いて",
    "それから",
    "あと、",
    "あとは",
    "また、",
    "さらに",
    "ところで",
    "さて",
    "一方",
    "では、",
    "なお",
    "最後に",
    "二つ目",
    "三つ目",
    "2つ目",
    "3つ目",
    "もう一つ",
    "以上",
];
/// Closing sentences of a message, set apart as their own paragraph.
const CLOSINGS: [&str; 5] = [
    "何かあれば",
    "よろしくお願い",
    "以上です",
    "お手数ですが",
    "引き続き",
];

/// Split a text into sentences after 。！？ (the marks stay with their sentence).
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        current.push(c);
        if matches!(c, '。' | '！' | '？' | '!' | '?') {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// Marks no sentence starts with, and doubled marks where recognized parts meet: a 、 or 。
/// at the very start or at the start of a line is dropped (the local punctuation model
/// sometimes put 。 before a part, and that part was often the first one); 、。 / 。、 / 。。
/// become 。 and 、、 becomes 、. Nothing else changes.
pub fn tidy_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let line = line.trim_start_matches(['、', '。', '，', '．']);
        let mut last: Option<char> = None;
        for c in line.chars() {
            let mark = matches!(c, '、' | '。');
            match (last, mark) {
                (Some('、'), true) if c == '。' => {
                    out.pop();
                    out.push('。');
                }
                (Some(previous), true) if previous == '。' || previous == c => {}
                _ => out.push(c),
            }
            last = Some(if mark { c } else { ' ' });
        }
    }
    out
}

/// A transcript ending in Japanese without a mark (Parakeet often writes none there,
/// 2026-10-23), or with a 、 left by a part's pause, ends with 。. Whisper already ends with
/// one; text ending in a Latin word, a bracket or another mark is left as it is.
pub fn close_final(text: &str) -> String {
    let trimmed = text.trim_end();
    match trimmed.chars().last() {
        Some('、') => format!("{}。", &trimmed[..trimmed.len() - '、'.len_utf8()]),
        Some('\u{3041}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}') => {
            format!("{trimmed}。")
        }
        _ => text.to_owned(),
    }
}

/// Split a long transcript into paragraphs before sentences that open a new topic
/// ("まず", "次に", "ところで", "最後に" …) or close the message ("何かあれば" …), and
/// after at most `PARAGRAPH_MAX_SENTENCES` sentences. Only blank lines are added; the
/// characters never change. Returns the text and the number of breaks added.
pub fn paragraphs(text: &str) -> (String, usize) {
    let parts = sentences(text);
    if text.chars().count() < PARAGRAPH_MIN_CHARS
        || parts.len() < PARAGRAPH_MIN_SENTENCES
        || text.contains('\n')
    {
        return (text.to_owned(), 0);
    }
    let mut out = String::new();
    let mut breaks = 0;
    let mut in_paragraph = 0;
    let mut paragraph_chars = 0;
    for (index, sentence) in parts.iter().enumerate() {
        let opening = sentence.trim_start();
        let new_topic = TOPIC_STARTS
            .iter()
            .chain(CLOSINGS.iter())
            .any(|word| opening.starts_with(word));
        let full =
            in_paragraph >= PARAGRAPH_MAX_SENTENCES || paragraph_chars >= PARAGRAPH_MAX_CHARS;
        if index > 0 && (new_topic || full) {
            out = out.trim_end().to_owned();
            out.push_str("\n\n");
            out.push_str(opening);
            breaks += 1;
            in_paragraph = 1;
            paragraph_chars = opening.chars().count();
        } else {
            out.push_str(sentence);
            in_paragraph += 1;
            paragraph_chars += sentence.chars().count();
        }
    }
    (out, breaks)
}

/// A run is collapsed only when its unit repeats at least this many times in a row.
const MIN_REPEATS: usize = 4;
/// Longest repeated unit looked for, in characters.
const MAX_REPEAT_UNIT: usize = 30;

/// Collapse a phrase that Whisper wrote over and over in a row (a known decoding loop:
/// "卒がなく、卒がなく、…" fifty times for one spoken phrase, 2026-10-10) into a single
/// occurrence, and say whether anything was collapsed. Only runs of at least
/// `MIN_REPEATS` copies of a unit of 2 or more characters are touched, so ordinary
/// repetition ("はいはい", "どんどん") stays.
pub fn collapse_repeats(text: &str) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut collapsed = false;
    let mut i = 0;
    while i < chars.len() {
        // The unit that covers the most characters with MIN_REPEATS or more copies.
        let mut best: Option<(usize, usize)> = None;
        for unit in 2..=MAX_REPEAT_UNIT.min((chars.len() - i) / MIN_REPEATS) {
            let mut copies = 1;
            while i + (copies + 1) * unit <= chars.len()
                && chars[i + copies * unit..i + (copies + 1) * unit] == chars[i..i + unit]
            {
                copies += 1;
            }
            if copies >= MIN_REPEATS && best.is_none_or(|(u, c)| unit * copies > u * c) {
                best = Some((unit, copies));
            }
        }
        match best {
            Some((unit, copies)) => {
                out.extend(&chars[i..i + unit]);
                i += unit * copies;
                collapsed = true;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    (out, collapsed)
}

/// Remove a stock subtitle line from the end of the text (with its trailing marks), and
/// say whether one was removed. Fast speech gives no 。 before it, so it is matched at
/// the end regardless of what precedes it. A text that is only such a line becomes empty
/// (it was silence). Design §9 exception, decided with the user on 2026-10-06.
pub fn remove_subtitle_tail(text: &str) -> (String, bool) {
    let trimmed = text.trim_end_matches(|c: char| c.is_whitespace() || MARKS.contains(&c));
    for tail in SUBTITLE_TAILS {
        if let Some(rest) = trimmed.strip_suffix(tail) {
            return (rest.trim_end().to_owned(), true);
        }
    }
    (text.to_owned(), false)
}

/// Hesitations removed by `remove_fillers`. Only unmistakable forms: "ええ" (yes),
/// "あの" (that), "まあ" (well/fairly) and "あー" (realisation) carry meaning and are
/// never listed. Longest first so "えーっと" is not read as "えー" + "っと".
const FILLERS: [&str; 9] = [
    "えーっと",
    "ええっと",
    "えーと",
    "ええと",
    "えっと",
    "あのー",
    "あのう",
    "うーん",
    "えー",
];

/// Forms that are hesitations even when Whisper writes no comma after them (fast
/// speech). Excluded on purpose: "ええと" ("ええとても" = yes + very), "あのう"
/// ("あのうさぎ" = that rabbit) and "えー" (surprise: "えー本当に").
const GLUED_FILLERS: [&str; 5] = ["えーっと", "ええっと", "えーと", "えっと", "あのー"];

const FILLER_COMMAS: [char; 3] = ['、', '，', ','];
const SENTENCE_BOUNDARIES: [char; 7] = ['、', '。', '？', '！', '?', '!', '\n'];

/// Remove hesitations that stand alone as their own phrase: at the start of the text
/// or right after punctuation, followed by their own comma ("えーと、明日は" → "明日は").
/// Fast speech often has no comma, so the unmistakable long forms in `GLUED_FILLERS`
/// also go when glued to the next word at a phrase start ("えーと明日は" → "明日は").
/// Position, not string match alone, decides (design §9); quotes and words containing
/// the same letters are kept. Off unless the user enables it.
pub fn remove_fillers(text: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut removed = 0;
    let mut i = 0;
    while i < chars.len() {
        let at_phrase_start = output
            .trim_end_matches([' ', '　'])
            .chars()
            .next_back()
            .is_none_or(|previous| SENTENCE_BOUNDARIES.contains(&previous));
        let filler = at_phrase_start
            .then(|| {
                FILLERS
                    .iter()
                    .map(|f| f.chars().collect::<Vec<char>>())
                    .find_map(|filler| {
                        let end = i + filler.len();
                        if end >= chars.len() || chars[i..end] != filler[..] {
                            return None;
                        }
                        if FILLER_COMMAS.contains(&chars[end]) {
                            return Some(filler.len() + 1); // with its comma
                        }
                        let glued = GLUED_FILLERS
                            .iter()
                            .any(|g| g.chars().eq(filler.iter().copied()));
                        let next = chars[end];
                        (glued && !SENTENCE_BOUNDARIES.contains(&next) && next != '」')
                            .then_some(filler.len())
                    })
            })
            .flatten();
        match filler {
            Some(length) => {
                // Drop the hesitation (and its comma) and any spaces after it.
                i += length;
                while i < chars.len() && matches!(chars[i], ' ' | '　') {
                    i += 1;
                }
                removed += 1;
            }
            None => {
                output.push(chars[i]);
                i += 1;
            }
        }
    }
    (output, removed)
}

#[cfg(test)]
mod paragraph_tests {
    use super::*;

    #[test]
    fn breaks_come_before_topic_words_and_closings() {
        let text = "今日は来週のプラネタリウム投影の準備について話します。まず番組の台本ですが、昨日の時点で八割ほどできています。残りは土星の衝の場面だけです。次に映像の方ですが、フルドーム用の素材がまだ二本足りません。これは外注先に今週中にお願いする予定です。それから当日の受付についてです。予約の人数が多いので、受付を二人に増やしたいと考えています。最後に、終わった後の片付けは全員でやりましょう。";
        let (split, breaks) = paragraphs(text);
        assert_eq!(breaks, 4);
        assert!(split.contains("話します。\n\nまず番組"));
        assert!(split.contains("だけです。\n\n次に映像"));
        assert!(split.contains("予定です。\n\nそれから当日"));
        assert!(split.contains("考えています。\n\n最後に"));
        // Only blank lines are added.
        assert_eq!(split.replace("\n\n", ""), text);
    }

    #[test]
    fn short_texts_and_texts_with_line_breaks_stay_as_they_are() {
        let short = "まず一つ目です。次に二つ目です。最後に三つ目です。";
        assert_eq!(paragraphs(short), (short.to_owned(), 0));
        let broken = format!("{}\n{}", "あ".repeat(100), "まず。次に。それから。最後に。");
        assert_eq!(paragraphs(&broken).1, 0);
    }

    #[test]
    fn a_long_run_without_topic_words_still_breaks() {
        // Three sentences per paragraph.
        let text = "これは長い話の一文目の文章です。".repeat(12);
        let (split, breaks) = paragraphs(&text);
        assert_eq!(breaks, 3);
        assert_eq!(split.replace("\n\n", ""), text);
        // Long sentences: a break at the first sentence end past 120 characters.
        let long = format!("{}です。", "あ".repeat(70)).repeat(4);
        assert_eq!(paragraphs(&long).1, 1);
    }
}

#[cfg(test)]
mod repeat_tests {
    use super::*;

    #[test]
    fn a_decoding_loop_collapses_to_one_occurrence() {
        let looped = format!(
            "完璧とは言わないですが、{}そんなに直していない。",
            "卒がなく、".repeat(50)
        );
        let (text, collapsed) = collapse_repeats(&looped);
        assert_eq!(
            text,
            "完璧とは言わないですが、卒がなく、そんなに直していない。"
        );
        assert!(collapsed);
    }

    #[test]
    fn ordinary_repetition_stays() {
        for spoken in [
            "はいはい、わかりました。",
            "どんどん進めます。",
            "ままま、いいか。",
            "あああ",
        ] {
            assert_eq!(collapse_repeats(spoken), (spoken.to_owned(), false));
        }
        // Three copies are below the threshold.
        let three = "同じ話、同じ話、同じ話、で終わり";
        assert_eq!(collapse_repeats(three), (three.to_owned(), false));
    }
}

#[cfg(test)]
mod subtitle_tail_tests {
    use super::*;

    #[test]
    fn stock_closing_lines_at_the_end_are_removed() {
        assert_eq!(
            remove_subtitle_tail("漢字変換がおかしくなっていましたご視聴ありがとうございました"),
            ("漢字変換がおかしくなっていました".to_owned(), true)
        );
        assert_eq!(
            remove_subtitle_tail("確認してください。ご視聴ありがとうございました。"),
            ("確認してください。".to_owned(), true)
        );
        assert_eq!(
            remove_subtitle_tail("ご視聴ありがとうございました"),
            (String::new(), true)
        );
        assert_eq!(
            remove_subtitle_tail("以上です。チャンネル登録よろしくお願いします！"),
            ("以上です。".to_owned(), true)
        );
    }

    #[test]
    fn the_same_words_elsewhere_or_ordinary_endings_stay() {
        for kept in [
            "ご視聴ありがとうございましたと書かれた字幕を消したい",
            "本日はご参加ありがとうございました。",
            "ありがとうございました。",
            "",
        ] {
            assert_eq!(
                remove_subtitle_tail(kept),
                (kept.to_owned(), false),
                "{kept}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_and_doubled_marks_are_tidied() {
        assert_eq!(tidy_marks("。今日は晴れです。"), "今日は晴れです。");
        assert_eq!(tidy_marks("、。今日は"), "今日は");
        assert_eq!(
            tidy_marks("前半です。。後半です、、それと、。終わり"),
            "前半です。後半です、それと。終わり"
        );
        assert_eq!(tidy_marks("一行目。\n。二行目"), "一行目。\n二行目");
    }

    #[test]
    fn transcript_ends_with_a_full_stop() {
        assert_eq!(close_final("明日は晴れです"), "明日は晴れです。");
        assert_eq!(
            close_final("それで明日の件ですが、"),
            "それで明日の件ですが。"
        );
        assert_eq!(close_final("ジャンプ"), "ジャンプ。");
        assert_eq!(close_final("終わりです。"), "終わりです。");
        assert_eq!(close_final("聞こえますか？"), "聞こえますか？");
        assert_eq!(close_final("GitHub"), "GitHub");
        assert_eq!(close_final("「はい」"), "「はい」");
        assert_eq!(close_final(""), "");
        assert_eq!(tidy_marks("段落\n\n。次の段落"), "段落\n\n次の段落");
        assert_eq!(
            tidy_marks("ふつうの文、そのまま。"),
            "ふつうの文、そのまま。"
        );
        assert_eq!(tidy_marks(""), "");
    }

    fn text(raw: &str) -> String {
        punctuate(raw).text
    }

    #[test]
    fn standalone_hesitations_are_removed_with_their_comma() {
        let cases = [
            (
                "えーと、明日の会議は三時からです。",
                "明日の会議は三時からです。",
            ),
            ("えー、えっと、資料を共有します", "資料を共有します"),
            (
                "確認しました。あのー、一点だけ質問です",
                "確認しました。一点だけ質問です",
            ),
            ("うーん、それは難しいですね", "それは難しいですね"),
            ("資料は、えーっと、明日送ります", "資料は、明日送ります"),
            ("えーと明日の会議は三時からです", "明日の会議は三時からです"),
            (
                "確認しました。えっとそれから資料です",
                "確認しました。それから資料です",
            ),
            ("あのー一点だけ質問です", "一点だけ質問です"),
        ];
        for (raw, expected) in cases {
            assert_eq!(remove_fillers(raw).0, expected, "{raw}");
        }
        assert_eq!(remove_fillers("えー、えっと、資料").1, 2);
    }

    #[test]
    fn meaningful_words_and_glued_fillers_are_kept() {
        // Design §9 regression examples and words that only look like fillers.
        for raw in [
            "あの山を見てください",
            "その案はまあまあ良いと思う",
            "まあ、いいか",
            "ええ、そうです",
            "あー、そういうことか",
            "あの、すみません",
            "ええとても良いですね",
            "あのうさぎを見て",
            "えー本当にそうなんですか",
            "うーん難しいですね",
            "明日はえーと三時です",
            "「えーと、」と彼は言った",
            "「えーと」と彼は言った",
            "こえー、と叫んだ",
            "えー",
        ] {
            assert_eq!(remove_fillers(raw).0, raw, "{raw}");
        }
    }

    #[test]
    fn filler_removal_only_deletes_whole_fillers_and_their_commas() {
        let raw = "えーと、まず、あのー、資料を、えー、確認します。うーん、以上です";
        let (out, removed) = remove_fillers(raw);
        assert_eq!(out, "まず、資料を、確認します。以上です");
        assert_eq!(removed, 4);
    }

    fn without_marks(s: &str) -> String {
        s.chars().filter(|c| !MARKS.contains(c)).collect()
    }

    // Evaluation set. Inputs are synthetic spoken-style Japanese, never recorded speech.
    #[test]
    fn evaluation_cases_insert_only_at_confident_boundaries() {
        let cases = [
            (
                "明日の午後に新しい企画の打ち合わせがありますので資料は事前に共有しておきますから確認をお願いしますそれから会議室の予約も忘れずに入れておいてください",
                "明日の午後に新しい企画の打ち合わせがありますので、資料は事前に共有しておきますから、確認をお願いします。それから、会議室の予約も忘れずに入れておいてください",
            ),
            (
                "このプログラムは音声を録音してからテキストに変換して現在開いているアプリに貼り付ける機能を持っていますがうまく動かない場合はログを確認してください",
                "このプログラムは音声を録音してからテキストに変換して現在開いているアプリに貼り付ける機能を持っていますが、うまく動かない場合はログを確認してください",
            ),
            (
                "まず入力ファイルを読み込んで次に不要な行を削除してから結果を新しいファイルに保存するスクリプトを書いてほしいです",
                "まず、入力ファイルを読み込んで次に不要な行を削除してから結果を新しいファイルに保存するスクリプトを書いてほしいです",
            ),
            (
                "まずはログを確認してそれから設定ファイルを開いて問題の行を探してみてください",
                "まずは、ログを確認してそれから設定ファイルを開いて問題の行を探してみてください",
            ),
            // Already punctuated by Whisper: marks are kept and short runs left alone.
            (
                "今日はいい天気ですね。会議は三時からです。",
                "今日はいい天気ですね。会議は三時からです。",
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(text(raw), expected);
        }
    }

    #[test]
    fn an_opener_at_the_start_gets_a_comma_and_a_follow_on_sentence_is_closed() {
        assert_eq!(
            text("それから会議室の予約も忘れずに入れておいてくださいありがとうございますまた来週の資料も共有してください"),
            "それから、会議室の予約も忘れずに入れておいてくださいありがとうございます。また、来週の資料も共有してください"
        );
    }

    #[test]
    fn ambiguous_words_are_left_alone() {
        let cases = [
            // 短い文: 触らない
            "今日はいい天気ですが明日は雨です",
            // 「のでは」「のです」は接続ではない
            "それは問題ないのではないかと思いますがどうでしょうかね",
            "これは知らなかったのですねそれにしても驚きましたよ本当に",
            // 「または」は接続詞「また」ではない
            "AではなくBまたはCを選んでくださいという意味ではありません",
            // 「まずい」「その後ろ」「なおす」は接続詞ではない
            "まずいことになりましたのでどうにかしてくださいお願いします本当に",
            "その後ろにある棚の上のものを全部下ろしてほしいのですがお願いできますか",
            "なおすところは全部で三つありますのでよく見て確認してください",
            // 文末助詞が続く「ますがね」
            "それもそうですがねやっぱり実際に試してみないと分かりません",
        ];
        for raw in cases {
            let out = text(raw);
            assert!(
                !out.contains("のでは、") && !out.contains("また、は") && !out.contains("がね、"),
                "{out}"
            );
            assert!(
                !out.contains("ので、は") && !out.contains("ので、す"),
                "{out}"
            );
            assert!(
                !out.contains("または、") && !out.contains("また、は"),
                "{out}"
            );
            assert!(
                !out.contains("まず、い") && !out.contains("後、ろ") && !out.contains("なお、す"),
                "{out}"
            );
        }
        assert_eq!(
            text("今日はいい天気ですが明日は雨です"),
            "今日はいい天気ですが明日は雨です"
        );
    }

    #[test]
    fn text_with_ascii_or_spaces_is_never_modified() {
        let raw =
            "このスクリプトはPythonで書いてありますがうまく動かないのでログを確認してください";
        assert_eq!(text(raw), raw);
        let raw = "この スクリプトは ありますがうまく動かないのでログを確認してください ありがとう";
        assert_eq!(text(raw), raw);
    }

    #[test]
    fn only_marks_are_ever_added_and_rules_are_fixed_ids() {
        let inputs = [
            "明日の午後に新しい企画の打ち合わせがありますので資料は事前に共有しておきますから確認をお願いしますそれから会議室の予約も忘れずに入れておいてください",
            "最初に環境を整えますしその後に依存関係を入れてからビルドしますがうまくいかなければログを見てくださいなお原因が分からなければ教えてください",
            "それでは始めますので準備ができたら合図をお願いしますまずは入力を確認します",
            "",
            "あ",
        ];
        for raw in inputs {
            let processed = punctuate(raw);
            assert_eq!(without_marks(&processed.text), without_marks(raw));
            assert_eq!(processed.raw, raw);
            assert!(processed
                .rules
                .iter()
                .all(|rule| rule.starts_with("punct.")));
            // Idempotent: running again on its own output changes nothing.
            assert_eq!(punctuate(&processed.text).text, processed.text);
        }
    }

    #[test]
    fn random_text_through_dictionary_and_punctuation_never_panics_or_rewrites() {
        // Release builds abort on panic, so post-processing must not panic at all
        // (design §9 wants the raw text kept if a rule fails). A deterministic
        // generator mixes the alphabets, marks and edge characters seen in speech.
        use crate::dictionary::apply_aliases;
        use crate::settings::DictionaryEntry;
        const PIECES: [&str; 24] = [
            "です",
            "ます",
            "ので",
            "ですが",
            "それから",
            "まず",
            "また",
            "は",
            "が",
            "、",
            "。",
            "AI",
            "MAIL",
            "x",
            " ",
            "\n",
            "😀",
            "ｱ",
            "の",
            "ください",
            "？",
            "!",
            "\u{0}",
            "\u{301}",
        ];
        let entries = [
            DictionaryEntry {
                note: String::new(),
                hint: true,
                id: "1".into(),
                term: "Claude".into(),
                aliases: vec!["AI".into(), "ですが".into()],
                enabled: true,
            },
            DictionaryEntry {
                note: String::new(),
                hint: true,
                id: "2".into(),
                term: "".into(),
                aliases: vec!["の".into(), "".into()],
                enabled: true,
            },
        ];
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..3000 {
            let mut raw = String::new();
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let length = (state % 60) as usize;
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                raw.push_str(PIECES[(state % PIECES.len() as u64) as usize]);
            }
            let (replaced, _) = apply_aliases(&raw, &entries);
            let processed = punctuate(&replaced);
            // Punctuation only ever adds marks to what the dictionary produced.
            assert_eq!(without_marks(&processed.text), without_marks(&replaced));
        }
    }

    #[test]
    fn existing_marks_survive_exactly() {
        let raw = "えーと、明日の件ですがよろしくお願いします。それから資料は共有済みです？はい！";
        let out = punctuate(raw).text;
        for mark in ['、', '。', '？', '！'] {
            assert!(out.matches(mark).count() >= raw.matches(mark).count());
        }
        assert_eq!(without_marks(&out), without_marks(raw));
    }
}
