//! Parse and check user-chosen global shortcuts such as "F8" or "Ctrl+Alt+V".
//! A plain letter, digit or Space must carry Ctrl, Alt or Win: otherwise registering it
//! would swallow ordinary typing system-wide.

pub const DEFAULT_RECORD: &str = "F8";
pub const DEFAULT_REPASTE: &str = "Ctrl+Alt+V";
pub const DEFAULT_ENGLISH: &str = "Shift+F8";

// Win32 modifier flags (MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN).
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub modifiers: u32,
    pub virtual_key: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutError {
    Empty,
    UnknownPart(String),
    MissingKey,
    TwoKeys,
    NeedsModifier,
    Same,
    SamePolish,
    SameCompose,
    SameEnglish,
}

impl ShortcutError {
    /// Plain message for the settings screen.
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "ショートカットが空です".into(),
            Self::UnknownPart(part) => format!("「{part}」は使えないキーです"),
            Self::MissingKey => {
                "修飾キー（Ctrl・Alt・Shift・Win）以外のキーを1つ含めてください".into()
            }
            Self::TwoKeys => "修飾キー以外のキーは1つだけにしてください".into(),
            Self::NeedsModifier => {
                "英字・数字・Spaceには Ctrl・Alt・Win のどれかを組み合わせてください".into()
            }
            Self::Same => "録音と再入力に同じショートカットは使えません".into(),
            Self::SamePolish => {
                "整形のショートカットは、録音・再入力と別のキーにしてください".into()
            }
            Self::SameCompose => {
                "作文のショートカットは、録音・再入力・整形と別のキーにしてください".into()
            }
            Self::SameEnglish => {
                "英語のショートカットは、ほかのショートカットと別のキーにしてください".into()
            }
        }
    }
}

fn key_code(name: &str) -> Option<(u32, bool)> {
    // Returns the virtual-key code and whether the key types a character.
    let upper = name.to_ascii_uppercase();
    if let Some(number) = upper.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24)
            .contains(&number)
            .then_some((0x70 + number - 1, false));
    }
    let mut chars = upper.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some((c as u32, true));
        }
    }
    match upper.as_str() {
        "SPACE" => Some((0x20, true)),
        "PAUSE" => Some((0x13, false)),
        "INSERT" => Some((0x2D, false)),
        _ => None,
    }
}

pub fn parse(text: &str) -> Result<Shortcut, ShortcutError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ShortcutError::Empty);
    }
    let mut modifiers = 0;
    let mut key: Option<(u32, bool)> = None;
    for part in text.split('+').map(str::trim) {
        let modifier = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(MOD_CONTROL),
            "alt" => Some(MOD_ALT),
            "shift" => Some(MOD_SHIFT),
            "win" | "windows" => Some(MOD_WIN),
            _ => None,
        };
        if let Some(modifier) = modifier {
            modifiers |= modifier;
        } else if let Some(code) = key_code(part) {
            if key.replace(code).is_some() {
                return Err(ShortcutError::TwoKeys);
            }
        } else {
            return Err(ShortcutError::UnknownPart(part.to_owned()));
        }
    }
    let (virtual_key, types_character) = key.ok_or(ShortcutError::MissingKey)?;
    if types_character && modifiers & (MOD_CONTROL | MOD_ALT | MOD_WIN) == 0 {
        return Err(ShortcutError::NeedsModifier);
    }
    Ok(Shortcut {
        modifiers,
        virtual_key,
    })
}

/// Both shortcuts parsed and distinct.
pub fn parse_pair(record: &str, repaste: &str) -> Result<(Shortcut, Shortcut), ShortcutError> {
    let record = parse(record)?;
    let repaste = parse(repaste)?;
    if record == repaste {
        return Err(ShortcutError::Same);
    }
    Ok((record, repaste))
}

/// Every global shortcut the app holds. `polish` is present only while the polishing
/// mode is turned on, `compose` only while compose mode is on too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bindings {
    pub record: Shortcut,
    pub repaste: Shortcut,
    pub polish: Option<Shortcut>,
    pub compose: Option<Shortcut>,
    /// Present only while English recording is turned on.
    pub english: Option<Shortcut>,
}

/// All shortcuts parsed and distinct from each other.
pub fn parse_bindings(
    record: &str,
    repaste: &str,
    polish: Option<&str>,
) -> Result<Bindings, ShortcutError> {
    let (record, repaste) = parse_pair(record, repaste)?;
    let polish = polish.map(parse).transpose()?;
    if polish.is_some_and(|polish| polish == record || polish == repaste) {
        return Err(ShortcutError::SamePolish);
    }
    Ok(Bindings {
        record,
        repaste,
        polish,
        compose: None,
        english: None,
    })
}

/// `bindings` with the compose-mode shortcut, distinct from all the others.
pub fn with_compose(bindings: Bindings, compose: Option<&str>) -> Result<Bindings, ShortcutError> {
    let compose = compose.map(parse).transpose()?;
    if compose.is_some_and(|compose| {
        compose == bindings.record
            || compose == bindings.repaste
            || Some(compose) == bindings.polish
    }) {
        return Err(ShortcutError::SameCompose);
    }
    Ok(Bindings {
        compose,
        ..bindings
    })
}

/// `bindings` with the English recording shortcut, distinct from all the others.
pub fn with_english(bindings: Bindings, english: Option<&str>) -> Result<Bindings, ShortcutError> {
    let english = english.map(parse).transpose()?;
    if english.is_some_and(|english| {
        english == bindings.record
            || english == bindings.repaste
            || Some(english) == bindings.polish
            || Some(english) == bindings.compose
    }) {
        return Err(ShortcutError::SameEnglish);
    }
    Ok(Bindings {
        english,
        ..bindings
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_common_forms_parse() {
        assert_eq!(
            parse("F8"),
            Ok(Shortcut {
                modifiers: 0,
                virtual_key: 0x77
            })
        );
        assert_eq!(
            parse("Ctrl+Alt+V"),
            Ok(Shortcut {
                modifiers: MOD_CONTROL | MOD_ALT,
                virtual_key: b'V' as u32
            })
        );
        assert_eq!(parse(" ctrl + shift + space "), parse("Ctrl+Shift+Space"));
        assert_eq!(parse("Win+F12").unwrap().virtual_key, 0x7B);
        assert_eq!(parse("Alt+1").unwrap().virtual_key, b'1' as u32);
        assert!(parse_pair(DEFAULT_RECORD, DEFAULT_REPASTE).is_ok());
    }

    #[test]
    fn the_polish_shortcut_is_optional_and_must_differ() {
        let without = parse_bindings("F8", "Ctrl+Alt+V", None).unwrap();
        assert_eq!(without.polish, None);
        assert_eq!(
            parse_bindings("F8", "Ctrl+Alt+V", Some("F9"))
                .unwrap()
                .polish,
            parse("F9").ok()
        );
        assert_eq!(
            parse_bindings("F8", "Ctrl+Alt+V", Some("f8")),
            Err(ShortcutError::SamePolish)
        );
        assert_eq!(
            parse_bindings("F8", "Ctrl+Alt+V", Some("Ctrl+Alt+V")),
            Err(ShortcutError::SamePolish)
        );
        assert_eq!(
            parse_bindings("F8", "Ctrl+Alt+V", Some("Q")),
            Err(ShortcutError::NeedsModifier)
        );
    }

    #[test]
    fn the_compose_shortcut_starts_with_shift_and_must_differ() {
        let base = parse_bindings("F8", "Ctrl+Alt+V", Some("F9")).unwrap();
        let both = with_compose(base, Some("Shift+F9")).unwrap();
        assert_eq!(both.compose, parse("Shift+F9").ok());
        assert_ne!(both.compose, both.polish);
        assert_eq!(with_compose(base, None).unwrap().compose, None);
        assert_eq!(
            with_compose(base, Some("F9")),
            Err(ShortcutError::SameCompose)
        );
        assert_eq!(
            with_compose(base, Some("f8")),
            Err(ShortcutError::SameCompose)
        );
    }

    #[test]
    fn the_english_shortcut_is_optional_and_must_differ() {
        let base = with_compose(
            parse_bindings("F8", "Ctrl+Alt+V", Some("F9")).unwrap(),
            Some("Shift+F9"),
        )
        .unwrap();
        assert_eq!(
            with_english(base, Some(DEFAULT_ENGLISH)).unwrap().english,
            parse("Shift+F8").ok()
        );
        assert_eq!(with_english(base, None).unwrap().english, None);
        for taken in ["F8", "Ctrl+Alt+V", "F9", "Shift+F9"] {
            assert_eq!(
                with_english(base, Some(taken)),
                Err(ShortcutError::SameEnglish)
            );
        }
    }

    #[test]
    fn shortcuts_that_would_swallow_typing_or_are_malformed_are_rejected() {
        assert_eq!(parse(""), Err(ShortcutError::Empty));
        assert_eq!(parse("V"), Err(ShortcutError::NeedsModifier));
        assert_eq!(parse("Shift+V"), Err(ShortcutError::NeedsModifier));
        assert_eq!(parse("Space"), Err(ShortcutError::NeedsModifier));
        assert_eq!(parse("Ctrl+Alt"), Err(ShortcutError::MissingKey));
        assert_eq!(parse("Ctrl+A+B"), Err(ShortcutError::TwoKeys));
        assert_eq!(parse("F25"), Err(ShortcutError::UnknownPart("F25".into())));
        assert_eq!(parse("Ctrl+@"), Err(ShortcutError::UnknownPart("@".into())));
        assert_eq!(parse_pair("F8", "f8"), Err(ShortcutError::Same));
        for error in [ShortcutError::Empty, ShortcutError::Same] {
            assert!(!error.message().is_empty());
        }
    }
}
