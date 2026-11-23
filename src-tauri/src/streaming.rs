//! Recognition while the user is still speaking (design: docs/STREAMING_DESIGN.md).
//!
//! During a recording the pipeline periodically copies the new audio, sends the part
//! recorded since the last cut to the worker, and the worker recognizes it up to its
//! last finished pause. At stop only the remainder is recognized, so the wait after stop
//! no longer grows with the length of the recording. Nothing is shown or delivered until
//! stop; any failure or doubt makes the whole recording go through the usual path.
use crate::audio::{resample_mono, AudioTap};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Send a part only once this much audio is waiting (minimum part plus the pause the
/// worker needs to hear finish), so most requests find a cut.
pub const MIN_PENDING_SECONDS: f32 = 3.6;
/// Context passed to the worker: the end of the text recognized so far.
const CONTEXT_CHARS: usize = 120;
/// A remainder shorter than this at stop holds no speech worth a request.
pub const MIN_TAIL_SAMPLES: usize = 16_000 / 2;

pub struct StreamState {
    tap: Option<AudioTap>,
    rate: u32,
    /// Mono audio at the device rate, everything read so far.
    raw: Vec<f32>,
    /// Frames of `raw` already recognized.
    committed: usize,
    texts: Vec<String>,
    /// Local-AI punctuation of each part, started in the background when the part was
    /// recognized (None: not needed or not chosen). Its answer replaces the part's text
    /// only if it arrives in time; otherwise the text stays as recognized.
    punctuation: Vec<Option<JoinHandle<Option<String>>>>,
    /// Set on any failure: the recording then goes the usual way.
    pub failed: bool,
    /// Why recognition while speaking was given up (a fixed code, never text).
    pub failure: Option<&'static str>,
    /// Parts the quick recognition doubted, recognized again the careful way.
    pub rescued: usize,
    /// Parts recognized again with the glossary topic's terms.
    pub topic_passes: usize,
}

impl StreamState {
    pub fn new(tap: AudioTap) -> Self {
        let rate = tap.sample_rate;
        Self {
            tap: Some(tap),
            rate,
            raw: Vec::new(),
            committed: 0,
            texts: Vec::new(),
            punctuation: Vec::new(),
            failed: false,
            failure: None,
            rescued: 0,
            topic_passes: 0,
        }
    }

    #[cfg(test)]
    fn from_samples(rate: u32, raw: Vec<f32>) -> Self {
        Self {
            tap: None,
            rate,
            raw,
            committed: 0,
            texts: Vec::new(),
            punctuation: Vec::new(),
            failed: false,
            failure: None,
            rescued: 0,
            topic_passes: 0,
        }
    }

    /// Give up recognition while speaking for this recording, recording why.
    pub fn give_up(&mut self, reason: &'static str) {
        if !self.failed {
            self.failed = true;
            self.failure = Some(reason);
        }
    }

    /// Take the audio recorded since the last read.
    pub fn pull(&mut self) {
        if let Some(tap) = &self.tap {
            match tap.drain() {
                Some(new) => self.raw.extend(new),
                None => self.give_up("audio_copy"),
            }
        }
    }

    pub fn pending_seconds(&self) -> f32 {
        (self.raw.len() - self.committed) as f32 / self.rate as f32
    }

    /// The audio not yet recognized, at 16 kHz.
    pub fn pending_16k(&self) -> Option<Vec<f32>> {
        resample_mono(self.raw[self.committed..].to_vec(), self.rate).ok()
    }

    /// The worker recognized `cut` samples (16 kHz) of the pending audio as `text`.
    pub fn commit(&mut self, cut: usize, text: String) {
        let frames = (cut as u64 * u64::from(self.rate) / 16_000) as usize;
        self.committed = (self.committed + frames).min(self.raw.len());
        self.texts.push(text);
        self.punctuation.push(None);
    }

    /// The text of the part recognized last.
    pub fn last_text(&self) -> Option<&str> {
        self.texts.last().map(String::as_str)
    }

    /// Whether a part's punctuation is still running. The GPU (6 GB) cannot run the
    /// recognizer and the local model side by side without slowing both down badly or
    /// running out of memory, so the next part waits until it is done.
    pub fn punctuation_busy(&self) -> bool {
        self.punctuation
            .iter()
            .flatten()
            .any(|handle| !handle.is_finished())
    }

    /// Punctuate the part recognized last in the background with `job`.
    pub fn punctuate_last(&mut self, job: JoinHandle<Option<String>>) {
        if let Some(slot) = self.punctuation.last_mut() {
            *slot = Some(job);
        }
    }

    /// The text so far, with each part's punctuation where it finished by `deadline`.
    /// Returns the text and how many parts were punctuated.
    pub async fn finished_text(&mut self, deadline: Instant) -> (String, usize) {
        let mut text = String::new();
        let mut punctuated = 0;
        for (part, job) in self.texts.iter().zip(self.punctuation.iter_mut()) {
            if let Some(handle) = job.as_ref() {
                while !handle.is_finished() && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            // A job still running is left to finish on its own; its answer is unused.
            let answer = match job.take() {
                Some(handle) if handle.is_finished() => handle.join().ok().flatten(),
                other => {
                    *job = other;
                    None
                }
            };
            match answer {
                Some(answer) => {
                    punctuated += 1;
                    text.push_str(&answer);
                }
                None => text.push_str(part),
            }
        }
        (text, punctuated)
    }

    pub fn parts(&self) -> usize {
        self.texts.len()
    }

    /// Whether this recording can be finished from the parts recognized so far.
    pub fn usable(&self) -> bool {
        !self.failed && !self.texts.is_empty()
    }

    /// Samples (16 kHz) of the final recording already recognized.
    pub fn committed_16k(&self) -> usize {
        (self.committed as u64 * 16_000 / u64::from(self.rate)) as usize
    }

    pub fn text(&self) -> String {
        self.texts.concat()
    }

    pub fn context(&self) -> String {
        let text = self.text();
        let chars: Vec<char> = text.chars().collect();
        chars[chars.len().saturating_sub(CONTEXT_CHARS)..]
            .iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_parts_map_between_device_and_16k_rates() {
        let mut state = StreamState::from_samples(48_000, vec![0.0; 48_000 * 10]);
        assert!((state.pending_seconds() - 10.0).abs() < 1e-3);
        assert!(!state.usable());
        // 4 s recognized at 16 kHz = 4 s at the device rate.
        state.commit(16_000 * 4, "前半".into());
        assert!((state.pending_seconds() - 6.0).abs() < 1e-3);
        assert_eq!(state.committed_16k(), 16_000 * 4);
        state.commit(16_000 * 3, "後半".into());
        assert_eq!(state.text(), "前半後半");
        assert_eq!(state.parts(), 2);
        assert!(state.usable());
        // A cut past the audio read so far never runs beyond it.
        state.commit(16_000 * 100, String::new());
        assert_eq!(state.pending_seconds(), 0.0);
        state.failed = true;
        assert!(!state.usable());
    }

    #[tokio::test]
    async fn punctuation_answers_replace_parts_only_when_they_arrive_in_time() {
        let mut state = StreamState::from_samples(16_000, vec![0.0; 16_000 * 12]);
        state.commit(16_000 * 4, "一つ目の文二つ目の文".into());
        state.punctuate_last(std::thread::spawn(|| Some("一つ目の文。二つ目の文".into())));
        state.commit(16_000 * 4, "三つ目".into());
        state.commit(16_000 * 4, "四つ目の文".into());
        // A failed answer and a late one both keep the recognized text.
        state.punctuate_last(std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(400));
            Some("遅すぎた答え".into())
        }));
        let deadline = Instant::now() + Duration::from_millis(100);
        assert!(state.punctuation_busy());
        let (text, punctuated) = state.finished_text(deadline).await;
        assert_eq!(text, "一つ目の文。二つ目の文三つ目四つ目の文");
        assert_eq!(punctuated, 1);
        assert_eq!(state.last_text(), Some("四つ目の文"));
    }

    #[test]
    fn pending_audio_is_resampled_and_context_is_the_text_end() {
        let mut state = StreamState::from_samples(48_000, vec![0.1; 48_000 * 2]);
        let pending = state.pending_16k().unwrap();
        assert!(
            (pending.len() as i64 - 32_000).abs() < 64,
            "{}",
            pending.len()
        );
        state.commit(16_000, "あ".repeat(200));
        assert_eq!(state.context().chars().count(), CONTEXT_CHARS);
    }
}
