//! For each standard-input line "transcript<TAB>draft", prints the dictionary suggestions
//! the app would offer (default glossary as vocabulary), for checking the suggestion
//! rule on evaluation output. Nothing is stored.
use std::io::{BufRead, Write};
use voiceweave_lib::candidates::find;
use voiceweave_lib::glossary::{terms, DEFAULT_TEXT};

fn main() {
    let glossary = terms(DEFAULT_TEXT);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("utf-8 input");
        let Some((original, draft)) = line.split_once('\t') else {
            writeln!(out).expect("write");
            continue;
        };
        let found: Vec<String> = find(original, draft, &glossary)
            .into_iter()
            .map(|candidate| format!("{}→{}", candidate.alias, candidate.term))
            .collect();
        writeln!(out, "{}", found.join(" / ")).expect("write");
    }
}
