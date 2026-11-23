//! Sends one synthetic sentence through the polishing path to the local Ollama and prints
//! the draft and timing. Usage: cargo run --example polish_probe [model]
//! Needs Ollama running on 127.0.0.1:11434. No personal text is used.
use std::time::Instant;
use voiceweave_lib::polish::{
    compose, polish, warm_up, DEFAULT_ENDPOINT, DEFAULT_MODEL, POLISH_TIMEOUT,
};

fn main() {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    let original = "明日の会議の件なんですけど、四時からに変更になりましたので、田中さんにも伝えておいてもらえますか。";
    let started = Instant::now();
    println!(
        "warm-up: {:?} in {:.2?}",
        warm_up(DEFAULT_ENDPOINT, &model),
        started.elapsed()
    );
    let started = Instant::now();
    match polish(
        DEFAULT_ENDPOINT,
        &model,
        original,
        &Default::default(),
        POLISH_TIMEOUT,
    ) {
        Ok(draft) => println!(
            "{:.2?}\n{}",
            started.elapsed(),
            compose(&draft, original, true)
        ),
        Err(error) => println!(
            "{:.2?} failed: {error:?} / {}",
            started.elapsed(),
            error.notice()
        ),
    }
}
