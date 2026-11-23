//! What is shown on the screen while dictating, used as reference (an opt-in setting).
//!
//! When a recording starts, a thread of its own reads, through Windows UI Automation
//! (the interface screen readers use) and within a time budget, the text currently shown
//! in the input window and in up to three other windows visible on the screen (the
//! material the user is looking at while speaking). From documents only the visible part
//! is read; menus, tabs, toolbars and buttons are skipped; password fields never.
//!
//! Two things come out of it:
//! - for plain dictation (free, on this PC): the glossary and dictionary terms that are
//!   on the screen, as recognition hints;
//! - for polishing (the cloud model): an excerpt of the shown text, as reference for
//!   names and terms.
//!
//! The screen text is used for this recording only and never stored or logged; the
//! matched terms are shown in the diagnostics (in memory).
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    UIA_ButtonControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_SplitButtonControlTypeId, UIA_StatusBarControlTypeId,
    UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_TextPatternId, UIA_TitleBarControlTypeId,
    UIA_ToolBarControlTypeId, UIA_ToolTipControlTypeId, UIA_CONTROLTYPE_ID,
};

/// Time allowed for reading all windows; what was read by then is used.
const READ_BUDGET: Duration = Duration::from_millis(900);
/// Other visible windows read besides the input window.
const OTHER_WINDOWS: usize = 3;
/// Elements visited at most per window.
const MAX_ELEMENTS: usize = 1_500;
/// Characters read per window at most.
const MAX_WINDOW_CHARS: usize = 20_000;
/// The excerpt for the drafting model: characters in all, and per window.
pub const EXCERPT_CHARS: usize = 2_000;
const EXCERPT_WINDOW_CHARS: usize = 800;
/// Matched terms kept, and added to the recognizer's hints at most.
const MAX_TERMS: usize = 20;
pub const HINT_TERMS: usize = 8;

/// Parts of a window that hold its controls rather than its content.
const CHROME: [UIA_CONTROLTYPE_ID; 12] = [
    UIA_MenuBarControlTypeId,
    UIA_MenuControlTypeId,
    UIA_MenuItemControlTypeId,
    UIA_ToolBarControlTypeId,
    UIA_TabControlTypeId,
    UIA_TabItemControlTypeId,
    UIA_TitleBarControlTypeId,
    UIA_ScrollBarControlTypeId,
    UIA_StatusBarControlTypeId,
    UIA_ToolTipControlTypeId,
    UIA_SplitButtonControlTypeId,
    UIA_ButtonControlTypeId,
];

/// What the screen gave for one recording.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenRead {
    /// Glossary and dictionary terms shown on the screen, longest first.
    pub terms: Vec<String>,
    /// The shown text, input window first, for the drafting model.
    pub excerpt: String,
    /// Names shown on the screen that neither the glossary nor the dictionary has
    /// (katakana words, capitalized Latin words), for glossary suggestions.
    pub words: Vec<String>,
}

/// Longest list of unknown words taken from one reading.
const MAX_WORDS: usize = 40;
/// Common words of screens and documents that are never suggested.
const COMMON_WORDS: &[&str] = &[
    "ファイル",
    "フォルダー",
    "フォルダ",
    "ウィンドウ",
    "メニュー",
    "ボタン",
    "ダウンロード",
    "アップロード",
    "メッセージ",
    "アカウント",
    "ユーザー",
    "プロジェクト",
    "コメント",
    "データ",
    "テキスト",
    "ページ",
    "リンク",
    "メール",
    "アドレス",
    "パスワード",
    "ログイン",
    "ログアウト",
    "インストール",
    "アップデート",
    "バージョン",
    "システム",
    "サービス",
    "サーバー",
    "ブラウザー",
    "ブラウザ",
    "アプリ",
    "アプリケーション",
    "ソフト",
    "ソフトウェア",
    "プログラム",
    "コード",
    "エラー",
    "テスト",
    "チェック",
    "クリック",
    "スクロール",
    "コピー",
    "ペースト",
    "キャンセル",
    "オプション",
    "ツール",
    "ヘルプ",
    "ホーム",
    "スタート",
    "タスク",
    "スケジュール",
    "カレンダー",
    "ドキュメント",
    "イメージ",
    "ファイル名",
    "フィードバック",
    "コンテンツ",
    "パソコン",
    "インターネット",
    "ネットワーク",
    "セキュリティ",
    "プライバシー",
    "ポリシー",
    "ニュース",
    "トップ",
    "グループ",
    "チャット",
    "チャンネル",
    "スレッド",
    "リスト",
    "テーブル",
    "カテゴリー",
    "ステータス",
    "レポート",
    "This",
    "That",
    "With",
    "From",
    "Your",
    "What",
    "When",
    "Where",
    "Which",
    "There",
    "These",
    "Those",
    "About",
    "Home",
    "File",
    "Edit",
    "View",
    "Help",
    "Search",
    "Settings",
    "Open",
    "Close",
    "Save",
    "Cancel",
    "Delete",
    "Share",
    "More",
    "Next",
    "Back",
    "New",
    "Copy",
    "Paste",
];

fn is_katakana(c: char) -> bool {
    matches!(c, 'ァ'..='ヺ' | 'ー')
}

/// Katakana words of 4 or more characters and capitalized Latin words of 4 or more
/// letters, once each, that `vocabulary` does not hold.
fn unknown_words(text: &str, vocabulary: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |word: String| {
        let known = vocabulary.iter().any(|term| term.contains(word.as_str()));
        if !known && !COMMON_WORDS.contains(&word.as_str()) && !out.contains(&word) {
            out.push(word);
        }
    };
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        if is_katakana(c) && c != 'ー' {
            while i < chars.len() && is_katakana(chars[i]) {
                i += 1;
            }
            if i - start >= 4 {
                push(chars[start..i].iter().collect());
            }
        } else if c.is_ascii_uppercase()
            && (start == 0 || !chars[start - 1].is_ascii_alphanumeric())
        {
            while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                i += 1;
            }
            if i - start >= 4 {
                push(chars[start..i].iter().collect());
            }
        } else {
            i += 1;
        }
    }
    out.truncate(MAX_WORDS);
    out
}

/// Read the input window `target` and up to `OTHER_WINDOWS` other windows visible on the
/// screen. `vocabulary` (glossary and dictionary terms) decides which terms are matched.
/// Blocking (up to about `READ_BUDGET`); run on a thread of its own.
pub fn collect(target: usize, vocabulary: &[String]) -> ScreenRead {
    if target == 0 {
        return ScreenRead::default();
    }
    let mut windows = vec![target];
    windows.extend(
        visible_windows()
            .into_iter()
            .filter(|&window| window != target)
            .take(OTHER_WINDOWS),
    );
    summarize(&read_windows(&windows), vocabulary)
}

/// Matched terms and the excerpt from the texts of the windows read.
pub fn summarize(texts: &[String], vocabulary: &[String]) -> ScreenRead {
    let all = texts.join("\n");
    let mut terms: Vec<String> = vocabulary
        .iter()
        .filter(|term| term.chars().count() >= 1 && all.contains(term.as_str()))
        .cloned()
        .collect();
    // Longer terms are more specific; a one-character term (衝) matches easily by chance.
    terms.retain(|term| term.chars().count() >= 2);
    terms.sort_by_key(|term| std::cmp::Reverse(term.chars().count()));
    terms.dedup();
    terms.truncate(MAX_TERMS);
    let mut excerpt = String::new();
    for text in texts {
        let left = EXCERPT_CHARS.saturating_sub(excerpt.chars().count());
        if left == 0 {
            break;
        }
        let compact = compact(text);
        let part: String = compact
            .chars()
            .take(EXCERPT_WINDOW_CHARS.min(left))
            .collect();
        if !part.is_empty() {
            if !excerpt.is_empty() {
                excerpt.push('\n');
            }
            excerpt.push_str(&part);
        }
    }
    let words = unknown_words(&all, vocabulary);
    ScreenRead {
        terms,
        excerpt,
        words,
    }
}

/// Collapse runs of blank space and line breaks, so the excerpt carries text, not layout.
fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap: Option<char> = None;
    for c in text.chars() {
        if c == '\n' || c == '\r' {
            gap = Some('\n');
        } else if c.is_whitespace() {
            gap = gap.or(Some(' '));
        } else {
            if let Some(space) = gap.take() {
                if !out.is_empty() {
                    out.push(space);
                }
            }
            out.push(c);
        }
    }
    out
}

/// Add up to `HINT_TERMS` matched screen terms to the dictionary's hints, within the same
/// length limit; dictionary terms come first.
pub fn with_hints(mut hints: Vec<String>, screen: &[String]) -> Vec<String> {
    let mut used: usize = hints.iter().map(|hint| hint.chars().count()).sum();
    let mut added = 0;
    for term in screen {
        if added == HINT_TERMS {
            break;
        }
        let length = term.chars().count();
        if hints.contains(term) {
            continue;
        }
        if used + length > crate::dictionary::HINT_CHAR_LIMIT {
            break;
        }
        used += length;
        added += 1;
        hints.push(term.clone());
    }
    hints
}

/// Top-level windows a person can see, front to back: visible, not minimized, not hidden
/// by the system (cloaked), not a tool window, of a usable size, and not this app's own.
fn visible_windows() -> Vec<usize> {
    use windows_sys::Win32::Foundation::{HWND as SysHwnd, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
        IsIconic, IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
    };

    unsafe extern "system" fn each(hwnd: SysHwnd, found: LPARAM) -> windows_sys::core::BOOL {
        let found = &mut *(found as *mut Vec<usize>);
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return 1;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return 1;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        ) == 0
            && cloaked != 0
        {
            return 1;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect) == 0
            || rect.right - rect.left < 200
            || rect.bottom - rect.top < 150
        {
            return 1;
        }
        let mut process = 0;
        GetWindowThreadProcessId(hwnd, &mut process);
        if process == GetCurrentProcessId() {
            return 1;
        }
        let mut class = [0u16; 64];
        let length = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..length]);
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return 1;
        }
        found.push(hwnd as usize);
        // Enough candidates: stop enumerating.
        i32::from(found.len() < 12)
    }

    let mut found: Vec<usize> = Vec::new();
    // SAFETY: the callback only runs during this call and gets a pointer to `found`.
    unsafe { EnumWindows(Some(each), &mut found as *mut Vec<usize> as LPARAM) };
    found
}

/// The shown text of each window, in the order given (empty where nothing was read).
fn read_windows(windows: &[usize]) -> Vec<String> {
    // SAFETY: COM is initialized for this thread and released before returning; every
    // interface pointer is owned by a windows-rs wrapper that releases it on drop.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let texts = read_with_com(windows).unwrap_or_default();
        if initialized {
            CoUninitialize();
        }
        texts
    }
}

unsafe fn read_with_com(windows: &[usize]) -> Option<Vec<String>> {
    let started = Instant::now();
    let automation: IUIAutomation =
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
    let walker = automation.ControlViewWalker().ok()?;
    let mut texts = Vec::with_capacity(windows.len());
    for (index, &window) in windows.iter().enumerate() {
        let mut text = String::new();
        // Each window gets an equal share of the time that is left.
        let left = READ_BUDGET.saturating_sub(started.elapsed());
        let deadline = Instant::now() + left / (windows.len() - index) as u32;
        let Ok(root) = automation.ElementFromHandle(HWND(window as *mut core::ffi::c_void)) else {
            texts.push(text);
            continue;
        };
        let mut stack: Vec<IUIAutomationElement> = vec![root];
        let mut visited = 0;
        while let Some(element) = stack.pop() {
            if visited >= MAX_ELEMENTS
                || Instant::now() > deadline
                || text.chars().count() > MAX_WINDOW_CHARS
            {
                break;
            }
            visited += 1;
            if is_password(&element) {
                continue;
            }
            let kind = element.CurrentControlType().unwrap_or_default();
            if CHROME.contains(&kind) {
                continue;
            }
            // A document or text box: only the part shown on the screen, and nothing
            // inside it (its pieces would repeat the same text).
            if kind == UIA_DocumentControlTypeId || kind == UIA_EditControlTypeId {
                if let Some(shown) = visible_text(&element) {
                    text.push('\n');
                    text.push_str(&shown);
                    continue;
                }
            }
            if !element.CurrentIsOffscreen().is_ok_and(|off| off.as_bool()) {
                if let Ok(name) = element.CurrentName() {
                    let name = name.to_string();
                    if !name.trim().is_empty() {
                        text.push('\n');
                        text.push_str(&name);
                    }
                }
            }
            // Children in order: push them reversed so the first child is visited next.
            let mut children = Vec::new();
            let mut child = walker.GetFirstChildElement(&element).ok();
            while let Some(current) = child {
                child = walker.GetNextSiblingElement(&current).ok();
                children.push(current);
            }
            stack.extend(children.into_iter().rev());
        }
        texts.push(text);
    }
    Some(texts)
}

/// The text of `element` currently shown on the screen, if it offers that.
unsafe fn visible_text(element: &IUIAutomationElement) -> Option<String> {
    let pattern = element
        .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        .ok()?;
    let ranges = pattern.GetVisibleRanges().ok()?;
    let mut shown = String::new();
    for index in 0..ranges.Length().ok()? {
        if let Ok(range) = ranges.GetElement(index) {
            if let Ok(part) = range.GetText(8_000) {
                shown.push_str(&part.to_string());
                shown.push('\n');
            }
        }
    }
    (!shown.trim().is_empty()).then_some(shown)
}

unsafe fn is_password(element: &IUIAutomationElement) -> bool {
    // Unknown counts as a password: never read a field that may be one.
    element
        .CurrentIsPassword()
        .map_or(true, |password| password.as_bool())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn only_vocabulary_terms_on_the_screen_are_matched_longest_first() {
        let screen = words(&[
            "今夜は土星が衝を迎えます。金星は東方最大離角のころ。",
            "フルドーム番組の制作メモ ／ トラック フレーム イベント",
        ]);
        let vocabulary = words(&["衝", "土星", "東方最大離角", "フルドーム", "散開星団"]);
        let read = summarize(&screen, &vocabulary);
        assert_eq!(read.terms, words(&["東方最大離角", "フルドーム", "土星"]));
    }

    #[test]
    fn the_excerpt_is_compact_and_bounded() {
        let screen = vec![
            "入力先\n\n\n   本文  です".to_owned(),
            "あ".repeat(5_000),
            "い".repeat(5_000),
        ];
        let read = summarize(&screen, &[]);
        assert!(read.excerpt.starts_with("入力先\n本文 です\n"));
        assert!(read.excerpt.chars().count() <= EXCERPT_CHARS + 2);
        assert!(read.excerpt.contains('い'));
    }

    #[test]
    fn matched_terms_follow_the_dictionary_hints_within_limits() {
        let screen: Vec<String> = (0..20).map(|i| format!("用語{i}")).collect();
        let hints = with_hints(words(&["VoiceWeave", "用語1"]), &screen);
        assert_eq!(hints[0], "VoiceWeave");
        assert_eq!(hints.iter().filter(|hint| *hint == "用語1").count(), 1);
        assert_eq!(hints.len(), 2 + HINT_TERMS);
    }

    #[test]
    fn unknown_names_are_collected_once_without_known_or_common_words() {
        let screen = words(&[
            "ドームマスターの書き出しとリュウグウ、Typeless と VoiceWeave、ファイルを開く",
            "リュウグウ The Typelessly typeless Ok GPT",
        ]);
        let read = summarize(&screen, &words(&["ドームマスター", "VoiceWeave"]));
        assert_eq!(read.words, words(&["リュウグウ", "Typeless", "Typelessly"]));
    }

    #[test]
    fn no_window_reads_nothing() {
        assert_eq!(collect(0, &[]), ScreenRead::default());
    }
}

#[cfg(test)]
mod live {
    /// Reads the real screen; prints counts and time, and the matched glossary terms.
    /// Run with `cargo test screen::live -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real screen"]
    fn reads_visible_windows_within_budget() {
        let target =
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as usize;
        let vocabulary = crate::glossary::terms(crate::glossary::DEFAULT_TEXT);
        let started = std::time::Instant::now();
        let read = super::collect(target, &vocabulary);
        println!(
            "visible windows {}, glossary terms on screen {} ({}), excerpt {} chars, {:.2}s",
            super::visible_windows().len(),
            read.terms.len(),
            read.terms.join("、"),
            read.excerpt.chars().count(),
            started.elapsed().as_secs_f64()
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }
}
