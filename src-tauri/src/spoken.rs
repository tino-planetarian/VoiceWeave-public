//! Rules for how speech is written, applied to the recognized Japanese text: kanji numbers
//! with a unit ("二千二十六年" → "2026年") and restatements ("3時、いや4時" → "4時").
//!
//! Both are deliberately narrow. A wrong rewrite of spoken words is silent, so only forms
//! that are unmistakable are touched: a number needs a unit right after it, and a
//! restatement needs the same kind of item (a number with the same unit, a day word, a
//! weekday) on both sides of the marker. Everything else is left exactly as recognized.

const DIGITS: [(char, u64); 10] = [
    ('〇', 0),
    ('一', 1),
    ('二', 2),
    ('三', 3),
    ('四', 4),
    ('五', 5),
    ('六', 6),
    ('七', 7),
    ('八', 8),
    ('九', 9),
];

/// Units that follow a number, longest first. 人 (一人 = ひとり), 番 (一番 = best),
/// 度 and 位 are left out on purpose: their kanji numbers are often words.
const UNITS: [&str; 33] = [
    "パーセント",
    "キロメートル",
    "メートル",
    "センチ",
    "ミリ",
    "キロ",
    "グラム",
    "リットル",
    "時間",
    "週間",
    "年間",
    "日間",
    "分間",
    "か月",
    "ヶ月",
    "ケ月",
    "ドル",
    "ユーロ",
    "%",
    "年",
    "月",
    "日",
    "時",
    "分",
    "秒",
    "円",
    "個",
    "件",
    "回",
    "台",
    "本",
    "枚",
    "歳",
];

/// More units, checked after `UNITS` (kept apart only so the array above stays readable).
const MORE_UNITS: [&str; 3] = ["点", "倍", "割"];

fn digit(c: char) -> Option<u64> {
    DIGITS.iter().find(|(d, _)| *d == c).map(|(_, v)| *v)
}

fn is_multiplier(c: char) -> bool {
    matches!(c, '十' | '百' | '千' | '万' | '億')
}

fn is_kanji_number(c: char) -> bool {
    digit(c).is_some() || is_multiplier(c)
}

fn is_number_char(c: char) -> bool {
    is_kanji_number(c) || c.is_ascii_digit() || ('０'..='９').contains(&c)
}

fn unit_at(chars: &[char]) -> Option<&'static str> {
    UNITS
        .iter()
        .chain(MORE_UNITS.iter())
        .find(|unit| {
            let unit: Vec<char> = unit.chars().collect();
            chars.starts_with(&unit)
        })
        .copied()
}

/// The value of a kanji number written with 十百千万億 ("二千二十六" = 2026).
fn quantity(run: &[char]) -> Option<u64> {
    let (mut total, mut section, mut pending) = (0u64, 0u64, None::<u64>);
    for &c in run {
        if let Some(value) = digit(c) {
            // 〇 is for positional writing; two digits in a row are not a quantity.
            if value == 0 || pending.is_some() {
                return None;
            }
            pending = Some(value);
            continue;
        }
        let place = match c {
            '十' => 10,
            '百' => 100,
            '千' => 1000,
            '万' => 10_000,
            '億' => 100_000_000,
            _ => return None,
        };
        let times = pending.take();
        if place >= 10_000 {
            section = section.checked_add(times.unwrap_or(0))?;
            let group = if section == 0 { 1 } else { section };
            total = total.checked_add(group.checked_mul(place)?)?;
            section = 0;
        } else {
            section = section.checked_add(times.unwrap_or(1).checked_mul(place)?)?;
        }
    }
    total
        .checked_add(section)?
        .checked_add(pending.unwrap_or(0))
}

/// 3万5000, 100万, 1億2000万: tens of thousands and above keep their Japanese unit.
fn counted(value: u64) -> String {
    if value < 10_000 {
        return value.to_string();
    }
    let (oku, man, rest) = (
        value / 100_000_000,
        value % 100_000_000 / 10_000,
        value % 10_000,
    );
    let mut text = String::new();
    if oku > 0 {
        text += &format!("{oku}億");
    }
    if man > 0 {
        text += &format!("{man}万");
    }
    if rest > 0 {
        text += &rest.to_string();
    }
    text
}

fn written(run: &[char]) -> Option<String> {
    if run.len() > 14 {
        return None;
    }
    // 〇〇 is a placeholder (〇〇円 = some amount), not 00.
    if run.len() > 1 && run[0] == '〇' {
        return None;
    }
    if run.iter().all(|c| digit(*c).is_some()) {
        // Positional (二〇二六 = 2026); a single digit is just that digit.
        return Some(
            run.iter()
                .map(|c| char::from(b'0' + digit(*c).unwrap_or(0) as u8))
                .collect(),
        );
    }
    quantity(run).map(counted)
}

/// Kanji numbers directly followed by a unit become digits ("三十分" → "30分",
/// "二月十四日" → "2月14日", "三万五千円" → "3万5000円"). Returns the text and how many
/// numbers were rewritten. Kanji numbers without a unit ("一緒", "三日月", "千葉") and
/// words that only look like numbers with a unit ("十分" = enough, "一時" = for a while,
/// "二分の一") are left alone.
pub fn tidy_numbers(text: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut changed = 0;
    let mut i = 0;
    let is_digit = |c: Option<&char>| matches!(c, Some('0'..='9' | '０'..='９'));
    while i < chars.len() {
        // "1てん5" (a decimal point written out by Parakeet) → "1.5".
        if chars[i] == 'て'
            && chars.get(i + 1) == Some(&'ん')
            && i > 0
            && is_digit(chars.get(i - 1))
            && is_digit(chars.get(i + 2))
        {
            out.push('.');
            changed += 1;
            i += 2;
            continue;
        }
        if !is_kanji_number(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && is_kanji_number(chars[end]) {
            end += 1;
        }
        let run = &chars[i..end];
        let unit = unit_at(&chars[end..]);
        let after_unit = unit.map_or(end, |unit| end + unit.chars().count());
        let next = chars.get(after_unit).copied();
        let vague_count = matches!(chars.get(i.wrapping_sub(1)), Some('数' | '何' | '幾')) && i > 0;
        // "120万円": the 万 belongs to the digits before it, it is not "1万" by itself.
        let after_digits = i > 0 && matches!(chars[i - 1], '0'..='9' | '０'..='９');
        let one = run == ['一'];
        let skip = unit.is_none()
            || vague_count
            || after_digits
            // A bare 万 or 億 is a word (万歳, 万年筆), not "1万".
            || matches!(run.first(), Some('万' | '億'))
            // Words, not numbers: 千歳 (a place), 五月雨, 一回り, 一本化, 一枚岩,
            // 一件落着, 一個人.
            || (run == ['千'] && unit == Some("歳"))
            || (unit == Some("月") && next == Some('雨'))
            || (unit == Some("回") && next == Some('り'))
            || (one && unit == Some("本") && matches!(next, Some('化' | '調' | '道' | '気')))
            || (one && unit == Some("枚") && next == Some('岩'))
            || (one && unit == Some("件") && next == Some('落'))
            || (one && unit == Some("個") && next == Some('人'))
            || (run == ['十'] && unit == Some("分"))
            || (run == ['一'] && unit == Some("時"))
            || (unit == Some("分") && next == Some('の'))
            || (unit == Some("日") && matches!(next, Some('月' | '酔')))
            || (unit == Some("回") && next == Some('忌'));
        match (skip, written(run)) {
            (false, Some(number)) => {
                out += &number;
                out += unit.unwrap_or_default();
                changed += 1;
                i = after_unit;
            }
            _ => {
                out.extend(run);
                i = end;
            }
        }
    }
    (out, changed)
}

const DAY_WORDS: [&str; 20] = [
    "明々後日",
    "一昨日",
    "明後日",
    "再来週",
    "再来月",
    "再来年",
    "おととい",
    "あさって",
    "昨日",
    "今日",
    "明日",
    "先週",
    "今週",
    "来週",
    "先月",
    "今月",
    "来月",
    "去年",
    "今年",
    "来年",
];
const WEEKDAYS: [char; 7] = ['月', '火', '水', '木', '金', '土', '日'];
const MARKERS: [&str; 8] = [
    "、いや、",
    "、いえ、",
    "、じゃなくて、",
    "、ではなくて、",
    "、いや",
    "、いえ",
    "、じゃなくて",
    "、ではなくて",
];

#[derive(PartialEq)]
enum Item {
    Day,
    Weekday,
    Counted(&'static str),
}

fn is_kanji(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

fn ends_with(chars: &[char], word: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    chars.ends_with(&word)
}

/// The item that ends `before`: (where it starts, what kind).
fn item_before(before: &[char]) -> Option<(usize, Item)> {
    let len = before.len();
    for word in DAY_WORDS {
        let size = word.chars().count();
        if ends_with(before, word) && !before[..len - size].last().copied().is_some_and(is_kanji) {
            return Some((len - size, Item::Day));
        }
    }
    for size in [3usize, 2] {
        let day_form = size == 3;
        if len >= size
            && (!day_form || before[len - 1] == '日')
            && before[len - size + 1] == '曜'
            && WEEKDAYS.contains(&before[len - size])
            && (day_form || before[len - 1] == '曜')
        {
            return Some((len - size, Item::Weekday));
        }
    }
    let unit = UNITS
        .iter()
        .chain(MORE_UNITS.iter())
        .copied()
        .find(|unit| ends_with(before, unit))?;
    let mut start = len - unit.chars().count();
    let number_end = start;
    while start > 0 && is_number_char(before[start - 1]) {
        start -= 1;
    }
    // Part of a longer number ("3.5秒", "-3秒", "1,000円", "10〜12時", "三点五秒"): dropping
    // only the tail would leave a different number behind, so the restatement is left as is.
    let continues = start > 0
        && matches!(
            before[start - 1],
            '.' | '．'
                | ','
                | '，'
                | '-'
                | '−'
                | '＋'
                | '+'
                | '/'
                | '／'
                | ':'
                | '：'
                | '〜'
                | '~'
                | '～'
                | '点'
        );
    (start < number_end && !continues).then_some((start, Item::Counted(unit)))
}

/// The item that starts `after`: (how long it is, what kind).
fn item_after(after: &[char]) -> Option<(usize, Item)> {
    for word in DAY_WORDS {
        let chars: Vec<char> = word.chars().collect();
        if after.starts_with(&chars) {
            return Some((chars.len(), Item::Day));
        }
    }
    if after.len() >= 2 && WEEKDAYS.contains(&after[0]) && after[1] == '曜' {
        return Some((
            if after.get(2) == Some(&'日') { 3 } else { 2 },
            Item::Weekday,
        ));
    }
    let mut end = 0;
    while end < after.len() && is_number_char(after[end]) {
        end += 1;
    }
    if end == 0 {
        return None;
    }
    let unit = unit_at(&after[end..])?;
    Some((end + unit.chars().count(), Item::Counted(unit)))
}

/// A restatement within one utterance keeps only the corrected item: "3時、いや4時" →
/// "4時", "明日、いや明後日" → "明後日", "月曜、いや火曜" → "火曜". Only when the same kind of
/// item (a number with the same unit, a day word, a weekday) is on both sides, so
/// "いやな予感" and "その件、いや大変でした" are untouched. Returns the text and the count.
pub fn fix_restatements(text: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let markers: Vec<Vec<char>> = MARKERS.iter().map(|m| m.chars().collect()).collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut fixed = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '、' {
            if let Some(marker) = markers.iter().find(|m| chars[i..].starts_with(m)) {
                let after = i + marker.len();
                if let (Some((start, was)), Some((_, now))) =
                    (item_before(&out), item_after(&chars[after..]))
                {
                    if was == now {
                        out.truncate(start);
                        fixed += 1;
                        i = after;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    (out.into_iter().collect(), fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_out_decimal_point_between_digits_becomes_a_dot() {
        assert_eq!(
            tidy_numbers("費用は1てん5万円です"),
            ("費用は1.5万円です".to_owned(), 1)
        );
        for text in ["1てんです", "てん5", "点数は3てんてん", "ポイント1、てん5"]
        {
            assert_eq!(tidy_numbers(text), (text.to_owned(), 0), "{text}");
        }
    }

    #[test]
    fn a_unit_after_digits_is_not_read_as_a_number_of_its_own() {
        for text in [
            "予算は120万円です",
            "参加者は3人で、予算は120万円です。",
            "1.5万円かかる",
            "売上は３億円",
            "2千円です",
        ] {
            assert_eq!(tidy_numbers(text), (text.to_owned(), 0), "{text}");
        }
    }

    #[test]
    fn kanji_numbers_with_a_unit_become_digits() {
        for (raw, expected) in [
            ("二千二十六年に始まった", "2026年に始まった"),
            ("二〇二六年", "2026年"),
            ("二月十四日の午後三時", "2月14日の午後3時"),
            ("三十分かかる", "30分かかる"),
            ("二十歳になった", "20歳になった"),
            ("千五百円です", "1500円です"),
            ("三万五千円", "3万5000円"),
            ("百万円", "100万円"),
            ("百二十万円", "120万円"),
            ("一千万円", "1000万円"),
            ("一万二千三百四十五円", "1万2345円"),
            ("三百六十五日", "365日"),
            ("十一月二十三日", "11月23日"),
            ("一本だけ", "1本だけ"),
            ("百歳になった", "100歳になった"),
            ("千円です", "1000円です"),
            ("一〇〇円", "100円"),
            ("予算は百二十万円です", "予算は120万円です"),
            ("一億二千万円", "1億2000万円"),
            ("五十パーセント", "50パーセント"),
            ("三か月と二週間", "3か月と2週間"),
            ("一時間半", "1時間半"),
            ("三回目", "3回目"),
        ] {
            assert_eq!(tidy_numbers(raw).0, expected, "{raw}");
        }
        assert_eq!(tidy_numbers("二月十四日").1, 2);
    }

    #[test]
    fn words_that_only_look_like_numbers_are_kept() {
        for raw in [
            "一緒に行こう",
            "十分に休んだ",
            "一時的に止めた",
            "三日月が出ている",
            "二日酔いだ",
            "千葉に行く",
            "二分の一にする",
            "数十分待った",
            "数百円で買えた",
            "何千円もする",
            "一人で行く",
            "一番好きです",
            "唯一の方法",
            "八百屋に寄った",
            "三回忌の法要",
            "2026年に始まった",
            "万歳と叫んだ",
            "万年筆で書く",
            "億万長者",
            "千歳空港に着いた",
            "五月雨のように",
            "一回り大きい",
            "窓口を一本化する",
            "一本調子の話",
            "一枚岩の組織",
            "これで一件落着",
            "一個人として",
            "〇〇円を払う",
            "〇〇月〇〇日",
        ] {
            assert_eq!(tidy_numbers(raw).0, raw, "{raw}");
        }
    }

    #[test]
    fn a_restatement_keeps_only_the_corrected_item() {
        for (raw, expected) in [
            ("3時、いや4時に集合です", "4時に集合です"),
            ("三時、いや、四時にします", "四時にします"),
            ("明日、いや明後日に行く", "明後日に行く"),
            ("会議は月曜、いや火曜です", "会議は火曜です"),
            ("午後3時、いや4時半です", "午後4時半です"),
            ("来週、じゃなくて再来週で", "再来週で"),
            ("3時、いや4時、いや5時にする", "5時にする"),
            ("500円、いや600円です", "600円です"),
        ] {
            assert_eq!(fix_restatements(raw).0, expected, "{raw}");
        }
        assert_eq!(fix_restatements("3時、いや4時に").1, 1);
    }

    #[test]
    fn a_marker_that_is_not_a_restatement_is_kept() {
        for raw in [
            "いやな予感がする",
            "その件、いや大変でした",
            "3時、いやいや間に合わない",
            "明日、いやな顔をした",
            "3時、いや4分だけ",
            "東京駅、いや大阪駅だ",
            "今日はいい天気です、いやほんとに",
            "3時にします",
        ] {
            assert_eq!(fix_restatements(raw).0, raw, "{raw}");
        }
    }

    #[test]
    fn a_restatement_inside_a_longer_number_is_left_alone() {
        // Dropping only the tail would leave a different number ("3.5秒、いや4秒" → "3.4秒").
        for raw in [
            "3.5秒、いや4秒かかった",
            "-3秒、いや4秒",
            "1,000円、いや2000円です",
            "10〜12時、いや13時",
            "三点五秒、いや四秒",
            "12:30分、いや40分",
        ] {
            assert_eq!(fix_restatements(raw), (raw.to_owned(), 0), "{raw}");
        }
        // A whole number next to other words is still fixed.
        assert_eq!(fix_restatements("約3時、いや4時").0, "約4時");
    }
}
