//! Applies the app's rule-based punctuation to each line of standard input, for
//! scripts/eval/eval-asr.py. Reads and writes UTF-8; nothing is stored.
use std::io::{BufRead, Write};
use voiceweave_lib::postprocess::punctuate;

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("utf-8 input");
        writeln!(out, "{}", punctuate(&line).text).expect("write");
    }
}
