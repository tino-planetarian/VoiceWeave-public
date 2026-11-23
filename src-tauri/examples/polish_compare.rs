//! Sends each transcript of a JSON-lines file to both cloud services with the app's own
//! drafting instructions, and writes JSON lines {id, service, model, seconds, ok, text}.
//! For scripts/eval/polish-compare.py; uses the API keys saved by the app (never shown).
//! Input lines: {"id": "...", "text": "..."}, plus "compose": true and an optional
//! "reference" (copied text) and "previous" (the text to revise) for compose mode. Context: the default glossary and profile.
//! `--check` only reports whether each key is saved. Models: arguments "openai=MODEL" and
//! "anthropic=MODEL" (any number, run in parallel); without them, the app's defaults.
use std::io::{BufRead, Write};
use std::time::Duration;
use voiceweave_lib::cloud::{has_key, timed, Provider};
use voiceweave_lib::polish::{cleanup, system_prompt_for, user_message, Context};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--check") {
        for provider in [Provider::OpenAi, Provider::Anthropic] {
            println!("{}: {}", provider.id(), has_key(provider));
        }
        return;
    }
    let mut services: Vec<(Provider, String)> = args[1..]
        .iter()
        .filter_map(|arg| {
            let (service, model) = arg.split_once('=')?;
            Some((Provider::parse(service)?, model.to_owned()))
        })
        .collect();
    if services.is_empty() {
        let defaults = voiceweave_lib::settings::PolishSettings::default();
        services = vec![
            (Provider::OpenAi, defaults.openai_model),
            (Provider::Anthropic, defaults.anthropic_model),
        ];
    }
    let context = Context {
        profile: voiceweave_lib::settings::PolishSettings::default().profile,
        terms: Vec::new(),
        notes: Vec::new(),
        screen: Vec::new(),
        screen_text: String::new(),
        glossary: voiceweave_lib::glossary::terms(voiceweave_lib::glossary::DEFAULT_TEXT),
        ..Context::default()
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("utf-8 input");
        if line.trim().is_empty() {
            continue;
        }
        let item: serde_json::Value = serde_json::from_str(&line).expect("json line");
        let id = item["id"].as_str().unwrap_or_default().to_owned();
        let context = Context {
            compose: item["compose"].as_bool().unwrap_or(false),
            reference: item["reference"].as_str().unwrap_or_default().to_owned(),
            previous: item["previous"].as_str().unwrap_or_default().to_owned(),
            ..context.clone()
        };
        let system = system_prompt_for(&context);
        let user = user_message(item["text"].as_str().unwrap_or_default(), &context);
        let jobs: Vec<_> = services
            .iter()
            .cloned()
            .map(|(provider, model)| {
                let user = user.clone();
                std::thread::spawn(move || {
                    timed(provider, &model, system, &user, Duration::from_secs(60))
                })
            })
            .collect();
        for job in jobs {
            let draft = job.join().expect("thread");
            let (ok, text) = match &draft.result {
                Ok(text) => (true, cleanup(text)),
                Err(error) => (false, error.status().to_owned()),
            };
            let record = serde_json::json!({
                "id": id, "service": draft.provider.id(), "model": draft.model,
                "seconds": (draft.seconds * 1000.0).round() / 1000.0, "ok": ok, "text": text,
            });
            writeln!(out, "{record}").expect("write");
            out.flush().expect("flush");
        }
    }
}
