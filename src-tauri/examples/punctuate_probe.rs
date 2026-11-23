//! Times the punctuation pass on synthetic long unpunctuated text. No speech involved.
use std::time::Instant;
use voiceweave_lib::postprocess::punctuate;

fn main() {
    let sentence = "明日の午後に新しい企画の打ち合わせがありますので資料は事前に共有しておきますから確認をお願いしますそれから会議室の予約も忘れずに入れておいてください";
    for repeat in [1_usize, 3, 10] {
        let text = sentence.repeat(repeat);
        let started = Instant::now();
        let mut marks = 0;
        for _ in 0..20 {
            marks = punctuate(&text).rules.len();
        }
        println!(
            "{} chars: {:.2} ms per call ({} marks)",
            text.chars().count(),
            started.elapsed().as_secs_f64() * 1000.0 / 20.0,
            marks
        );
    }
}
