use cpal::traits::HostTrait;
use std::thread;
use std::time::{Duration, Instant};
use voiceweave_lib::audio::{AudioCapture, AudioError};

fn main() {
    let index = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<usize>().ok());
    let seconds = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|seconds| (1..=300).contains(seconds))
        .unwrap_or(2);
    eprintln!("opening input {index:?}");
    let capture_result = match index {
        Some(index) => cpal::default_host()
            .input_devices()
            .ok()
            .and_then(|mut devices| devices.nth(index))
            .ok_or(AudioError::NoInputDevice)
            .and_then(AudioCapture::start_device),
        None => AudioCapture::start_default(),
    };
    let capture = match capture_result {
        Ok(capture) => capture,
        Err(error) => {
            eprintln!("Microphone unavailable: {error:?}");
            std::process::exit(1);
        }
    };
    eprintln!("capturing for {seconds} seconds");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline && !capture.max_duration_reached() {
        thread::sleep(Duration::from_millis(20));
    }
    let level = capture.level();
    let (callbacks, frames) = capture.capture_counts();
    let max_duration_reached = capture.max_duration_reached();
    eprintln!("callbacks={callbacks}; captured_frames={frames}; max_duration_reached={max_duration_reached}; last_level={level:.3}");
    if let Some(error) = capture.capture_error() {
        eprintln!("stream_error={error}");
    }
    eprintln!("stopping and resampling");
    match capture.stop() {
        Ok(audio) => {
            let peak = audio
                .samples
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            let rms = (audio
                .samples
                .iter()
                .map(|sample| sample * sample)
                .sum::<f32>()
                / audio.samples.len() as f32)
                .sqrt();
            println!(
                "16k samples: {}; peak: {peak:.6}; rms: {rms:.6}; startup_xrun: {}; max_duration_reached: {}; last level: {level:.3}",
                audio.samples.len(),
                audio.startup_xrun,
                audio.max_duration_reached,
            );
        }
        Err(error) => {
            eprintln!("Capture failed: {error:?}");
            std::process::exit(1);
        }
    }
}
