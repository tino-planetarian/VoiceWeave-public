//! The user's field glossary: terms of the fields they speak about, editable in the
//! settings. It is sent with every draft
//! (the drafting model prefers these spellings) and, for plain dictation, the terms that
//! are also shown on the screen become recognition hints.

/// The starting glossary: a short example of how to write it (terms separated by 、 or
/// line breaks, one topic per line), meant to be replaced with the user's own terms.
pub const DEFAULT_TEXT: &str = "VoiceWeave、Whisper、Groq、Ollama、ChatGPT
議事録、見積書、請求書、稟議書、打ち合わせ
";

/// Upper bound for the glossary the user can keep (characters).
pub const MAX_CHARS: usize = 4_000;

pub fn default_text() -> String {
    DEFAULT_TEXT.into()
}

/// `current` with the starting glossary's terms it lacks added at the end (the user's own
/// terms and order stay). Used by the settings button that brings in new default terms.
pub fn with_defaults(current: &str) -> String {
    let have = terms(current);
    let missing: Vec<String> = terms(DEFAULT_TEXT)
        .into_iter()
        .filter(|term| !have.contains(term))
        .collect();
    if missing.is_empty() {
        return current.to_owned();
    }
    let mut out = current.trim_end().to_owned();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&missing.join("、"));
    out
}

/// The terms of a glossary text: separated by line breaks, 、, commas or spaces; blanks
/// and duplicates dropped, order kept.
pub fn terms(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for term in text.split(['\n', '\r', '、', ',', '，', '　', ' ']) {
        let term = term.trim();
        if !term.is_empty() && !out.iter().any(|known| known == term) {
            out.push(term.to_owned());
        }
    }
    out
}

/// The glossary by topic: the terms of each line (the starting glossary keeps one topic per
/// line: phenomena, coordinates, constellations, ...), blank lines and duplicates dropped.
/// The worker recognizes a part again with a topic's terms when the part names the topic.
pub fn topics(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(terms)
        .filter(|line| !line.is_empty())
        .take(64)
        .collect()
}

/// Other spellings Whisper writes for glossary names, each with the glossary's spelling.
/// Measured on synthetic speech (2026-10-12, scripts/eval/eval-terms.py): without hints it
/// wrote サソリ座, 白鳥座, 大熊座, 双子座, カシオペア座 and 四分岐座 for the glossary's
/// names. Constellations also match in katakana (サソリ座) and as the head of a longer
/// name (白鳥座流星群).
const SPELLINGS: &[(&str, &[&str])] = &[
    ("おうし座", &["牡牛座"]),
    ("おひつじ座", &["牡羊座"]),
    ("ふたご座", &["双子座"]),
    ("かに座", &["蟹座"]),
    ("しし座", &["獅子座"]),
    ("おとめ座", &["乙女座"]),
    ("てんびん座", &["天秤座"]),
    ("さそり座", &["蠍座"]),
    ("いて座", &["射手座"]),
    ("やぎ座", &["山羊座"]),
    ("みずがめ座", &["水瓶座"]),
    ("うお座", &["魚座"]),
    ("はくちょう座", &["白鳥座"]),
    ("こと座", &["琴座"]),
    ("わし座", &["鷲座"]),
    ("おおぐま座", &["大熊座"]),
    ("こぐま座", &["小熊座", "子熊座"]),
    ("おおいぬ座", &["大犬座"]),
    ("こいぬ座", &["小犬座", "子犬座"]),
    ("りゅう座", &["竜座", "龍座"]),
    ("へびつかい座", &["蛇遣座", "蛇遣い座", "蛇使い座"]),
    ("へび座", &["蛇座"]),
    ("うみへび座", &["海蛇座"]),
    ("みずへび座", &["水蛇座"]),
    ("かんむり座", &["冠座"]),
    ("みなみのかんむり座", &["南の冠座", "南冠座"]),
    ("いるか座", &["海豚座"]),
    ("うさぎ座", &["兎座"]),
    ("からす座", &["烏座"]),
    ("くじら座", &["鯨座"]),
    ("はと座", &["鳩座"]),
    ("つる座", &["鶴座"]),
    ("きりん座", &["麒麟座"]),
    ("ぎょしゃ座", &["馭者座", "御者座"]),
    ("うしかい座", &["牛飼座", "牛飼い座"]),
    ("や座", &["矢座"]),
    ("ほうおう座", &["鳳凰座"]),
    ("くじゃく座", &["孔雀座"]),
    ("とかげ座", &["蜥蜴座"]),
    ("こじし座", &["小獅子座"]),
    ("いっかくじゅう座", &["一角獣座"]),
    ("みなみじゅうじ座", &["南十字座"]),
    ("みなみのうお座", &["南の魚座", "南魚座"]),
    ("さんかく座", &["三角座"]),
    ("みなみのさんかく座", &["南の三角座", "南三角座"]),
    ("おおかみ座", &["狼座"]),
    ("やまねこ座", &["山猫座"]),
    ("こぎつね座", &["小狐座"]),
    ("こうま座", &["小馬座", "子馬座"]),
    ("とびうお座", &["飛魚座"]),
    ("はえ座", &["蝿座", "蠅座"]),
    ("ぼうえんきょう座", &["望遠鏡座"]),
    ("けんびきょう座", &["顕微鏡座"]),
    ("とけい座", &["時計座"]),
    ("じょうぎ座", &["定規座"]),
    ("らしんばん座", &["羅針盤座"]),
    ("さいだん座", &["祭壇座"]),
    ("たて座", &["盾座", "楯座"]),
    ("ちょうこくしつ座", &["彫刻室座"]),
    ("ちょうこくぐ座", &["彫刻具座"]),
    ("がか座", &["画架座"]),
    ("ろくぶんぎ座", &["六分儀座"]),
    ("はちぶんぎ座", &["八分儀座"]),
    ("しぶんぎ座", &["四分儀座", "四分岐座"]),
    ("りょうけん座", &["猟犬座"]),
    ("りゅうこつ座", &["竜骨座"]),
    ("テーブルさん座", &["テーブル山座"]),
    ("ケンタウルス座", &["ケンタウロス座"]),
    ("カシオペヤ座", &["カシオペア座"]),
    ("ペガスス座", &["ペガサス座"]),
    ("ヘルクレス座", &["ヘラクレス座"]),
    (
        "ジェイムズ・ウェッブ宇宙望遠鏡",
        &[
            "ジェームズ・ウェッブ宇宙望遠鏡",
            "ジェームズ・ウェブ宇宙望遠鏡",
            "ジェイムズ・ウェブ宇宙望遠鏡",
            "ジェームズウェッブ宇宙望遠鏡",
            "ジェームズウェブ宇宙望遠鏡",
        ],
    ),
];

fn katakana(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'ぁ'..='ゖ' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// The other spellings that become each glossary name, as dictionary entries (never hints):
/// only names the glossary holds, alone or at the head of a longer term, so a user who
/// writes 白鳥座 in the glossary, or removes a name, keeps what Whisper wrote.
pub fn spelling_entries(terms: &[String]) -> Vec<crate::settings::DictionaryEntry> {
    let holds = |name: &str| terms.iter().any(|term| term.starts_with(name));
    let mut names: Vec<(String, Vec<String>)> = Vec::new();
    for term in terms {
        if term.ends_with('座') && term != "星座" {
            names.push((term.clone(), Vec::new()));
        }
    }
    for (name, others) in SPELLINGS {
        if !holds(name) {
            continue;
        }
        let others = others.iter().map(|other| (*other).to_owned());
        match names.iter_mut().find(|(known, _)| known == name) {
            Some((_, aliases)) => aliases.extend(others),
            None => names.push(((*name).to_owned(), others.collect())),
        }
    }
    names
        .into_iter()
        .map(|(name, mut aliases)| {
            let shifted = katakana(&name);
            if shifted != name {
                aliases.push(shifted);
            }
            crate::settings::DictionaryEntry {
                id: format!("glossary-{name}"),
                term: name,
                aliases,
                enabled: true,
                note: String::new(),
                hint: false,
            }
        })
        .filter(|entry| !entry.aliases.is_empty())
        .collect()
}

/// `text` with other spellings of glossary names written the glossary's way.
pub fn align(text: &str, terms: &[String]) -> (String, usize) {
    crate::dictionary::apply_aliases(text, &spelling_entries(terms))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real field glossary (astronomy, planetariums) to test against.
    const ASTRONOMY: &str = "\
水星、金星、地球、火星、木星、土星、天王星、海王星、準惑星、冥王星、小惑星、彗星、流星、流星群、火球、隕石
しぶんぎ座流星群、こと座流星群、みずがめ座η流星群、ペルセウス座流星群、オリオン座流星群、おうし座流星群、しし座流星群、ふたご座流星群、こぐま座流星群、りゅう座流星群
衝、合、内合、外合、留、順行、逆行、東方最大離角、西方最大離角、最大光度、日食、月食、皆既日食、金環日食、部分日食、皆既月食、部分月食、掩蔽、星食、惑星食、太陽面通過、月齢、満月、新月、上弦、下弦、極大
天球、天頂、天底、子午線、南中、黄道、白道、天の赤道、赤経、赤緯、黄経、黄緯、方位角、高度、天の北極、歳差、春分点、秋分点、夏至、冬至、恒星時、年周視差、光年、天文単位、パーセク、等級、絶対等級、視等級
恒星、連星、変光星、散開星団、球状星団、星雲、散光星雲、惑星状星雲、暗黒星雲、銀河、天の川、天の川銀河、銀河系、アンドロメダ銀河、マゼラン雲、ブラックホール、中性子星、白色矮星、赤色巨星、超新星、星座、一等星、北極星、北斗七星、夏の大三角、冬の大三角、春の大曲線、すばる、プレアデス星団
アンドロメダ座、いっかくじゅう座、いて座、いるか座、インディアン座、うお座、うさぎ座、うしかい座、うみへび座、エリダヌス座、おうし座、おおいぬ座、おおかみ座、おおぐま座、おとめ座、おひつじ座、オリオン座、がか座、カシオペヤ座、かじき座、かに座、かみのけ座、カメレオン座、からす座、かんむり座、きょしちょう座、ぎょしゃ座、きりん座、くじゃく座、くじら座、ケフェウス座、ケンタウルス座、けんびきょう座、こいぬ座、こうま座、こぎつね座、こぐま座、こじし座、コップ座、こと座、コンパス座、さいだん座、さそり座、さんかく座
しし座、じょうぎ座、たて座、ちょうこくぐ座、ちょうこくしつ座、つる座、テーブルさん座、てんびん座、とかげ座、とけい座、とびうお座、とも座、はえ座、はくちょう座、はちぶんぎ座、はと座、ふうちょう座、ふたご座、ペガスス座、へび座、へびつかい座、ヘルクレス座、ペルセウス座、ほ座、ぼうえんきょう座、ほうおう座、ポンプ座、みずがめ座、みずへび座、みなみじゅうじ座、みなみのうお座、みなみのかんむり座、みなみのさんかく座、や座、やぎ座、やまねこ座、らしんばん座、りゅう座、りゅうこつ座、りょうけん座、レチクル座、ろ座、ろくぶんぎ座、わし座
シリウス、ベテルギウス、リゲル、アルデバラン、ベガ、アルタイル、デネブ、アンタレス、スピカ、アークトゥルス、カノープス、ポルックス、カストル、レグルス
望遠鏡、双眼鏡、赤道儀、経緯台、口径、焦点距離、天体写真、光害、シーイング
プラネタリウム、投影機、光学式投影機、デジタル式、ドーム、ドームスクリーン、全天周、全天周映像、フルドーム、ドームマスター、魚眼、投映、星空解説、生解説、解説員、番組
探査機、人工衛星、国際宇宙ステーション、JAXA、NASA、ロケット、はやぶさ2、ジェイムズ・ウェッブ宇宙望遠鏡、ハッブル宇宙望遠鏡
";

    #[test]
    fn other_spellings_of_glossary_names_follow_the_glossary() {
        let terms = terms(ASTRONOMY);
        let (text, count) = align(
            "サソリ座と白鳥座、カシオペア座、四分岐座流星群、ジェームズ・ウェブ宇宙望遠鏡",
            &terms,
        );
        assert_eq!(
            text,
            "さそり座とはくちょう座、カシオペヤ座、しぶんぎ座流星群、ジェイムズ・ウェッブ宇宙望遠鏡"
        );
        assert_eq!(count, 5);
        // The longer name wins: 南の魚座 is みなみのうお座, not 南のうお座.
        assert_eq!(align("南の魚座", &terms).0, "みなみのうお座");
        // Words that only look alike stay: no 座 after them.
        assert_eq!(
            align("サソリと白鳥とスバル", &terms).0,
            "サソリと白鳥とスバル"
        );
    }

    #[test]
    fn names_the_glossary_does_not_hold_are_left_as_written() {
        let own = vec!["白鳥座".to_owned()];
        assert_eq!(align("白鳥座とサソリ座", &own).0, "白鳥座とサソリ座");
        assert!(spelling_entries(&[]).is_empty());
    }

    #[test]
    fn the_default_glossary_is_a_short_general_example() {
        let parsed = terms(DEFAULT_TEXT);
        assert_eq!(parsed.len(), 10);
        assert!(parsed.iter().any(|term| term == "議事録"));
        assert_eq!(topics(DEFAULT_TEXT).len(), 2);
    }

    #[test]
    fn a_field_glossary_parses_into_unique_terms() {
        let parsed = terms(ASTRONOMY);
        assert!(parsed.len() > 150, "{}", parsed.len());
        let mut unique = parsed.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), parsed.len());
        assert!(ASTRONOMY.chars().count() < MAX_CHARS);
    }

    #[test]
    fn missing_default_terms_are_added_without_touching_the_users_own() {
        let merged = with_defaults("自分の番組名、議事録");
        assert!(merged.starts_with(
            "自分の番組名、議事録
"
        ));
        let parsed = terms(&merged);
        assert_eq!(parsed.iter().filter(|term| *term == "議事録").count(), 1);
        assert!(parsed.iter().any(|term| term == "稟議書"));
        assert_eq!(with_defaults(&merged), merged);
    }

    #[test]
    fn topics_are_the_lines_of_the_glossary() {
        let topics = topics(ASTRONOMY);
        assert!(topics.len() >= 10);
        assert!(topics
            .iter()
            .any(|line| line.contains(&"赤経".to_owned()) && line.contains(&"天球".to_owned())));
        assert_eq!(
            super::topics("衝、合\n\n留　南中"),
            vec![vec!["衝", "合"], vec!["留", "南中"]]
        );
    }

    #[test]
    fn separators_and_blanks_are_handled() {
        assert_eq!(
            terms("衝, 合\n\n留　南中 衝"),
            vec!["衝", "合", "留", "南中"]
        );
        assert!(terms("").is_empty());
    }
}
