//! Writes each line of standard input with other spellings of glossary names in the
//! default glossary's spelling, for scripts/eval/eval-terms.py. Nothing is stored.
use std::io::{BufRead, Write};
use voiceweave_lib::glossary::{align, terms, DEFAULT_TEXT};

fn main() {
    let glossary = terms(DEFAULT_TEXT);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("utf-8 input");
        writeln!(out, "{}", align(&line, &glossary).0).expect("write");
    }
}
