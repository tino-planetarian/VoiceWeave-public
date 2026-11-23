//! Synthetic timing of the post-stop 48 kHz -> 16 kHz conversion. No microphone is used.
use std::time::Instant;
use voiceweave_lib::audio::resample_mono;

fn main() {
    for seconds in [1_u32, 6, 30] {
        let samples: Vec<f32> = (0..48_000 * seconds)
            .map(|i| (i as f32 * 0.05).sin() * 0.3)
            .collect();
        let started = Instant::now();
        let output = resample_mono(samples, 48_000).expect("resample");
        println!(
            "{seconds:>2} s of 48 kHz audio -> {} samples in {} ms",
            output.len(),
            started.elapsed().as_millis()
        );
    }
}
