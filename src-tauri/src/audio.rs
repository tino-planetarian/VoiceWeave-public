use crate::diagnostics::{elapsed_ms, BackendKind, FailureCode};
use audioadapter_buffers::owned::InterleavedOwned;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use rubato::{Fft, FixedSync, Resampler};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const OUTPUT_RATE: u32 = 16_000;
pub const MAX_CAPTURE_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioError {
    NoInputDevice,
    UnsupportedFormat,
    StartFailed,
    CaptureFailed,
    Overflow,
    TooShort,
    ConversionFailed,
    InputConfigFailed(cpal::ErrorKind),
    BuildStreamFailed(cpal::ErrorKind),
    PlayStreamFailed(cpal::ErrorKind),
}

impl AudioError {
    pub fn diagnostic(self) -> (FailureCode, Option<BackendKind>) {
        match self {
            Self::NoInputDevice => (FailureCode::NoInputDevice, None),
            Self::UnsupportedFormat => (FailureCode::UnsupportedFormat, None),
            Self::StartFailed => (FailureCode::AudioBuild, None),
            Self::CaptureFailed => (FailureCode::AudioStream, None),
            Self::Overflow => (FailureCode::AudioOverflow, None),
            Self::TooShort => (FailureCode::AudioTooShort, None),
            Self::ConversionFailed => (FailureCode::AudioConversion, None),
            Self::InputConfigFailed(kind) => (FailureCode::AudioConfig, Some(backend_kind(kind))),
            Self::BuildStreamFailed(kind) => (FailureCode::AudioBuild, Some(backend_kind(kind))),
            Self::PlayStreamFailed(kind) => (FailureCode::AudioPlay, Some(backend_kind(kind))),
        }
    }
}

pub fn backend_kind(kind: cpal::ErrorKind) -> BackendKind {
    match kind {
        cpal::ErrorKind::DeviceBusy => BackendKind::DeviceBusy,
        cpal::ErrorKind::DeviceChanged => BackendKind::DeviceChanged,
        cpal::ErrorKind::DeviceNotAvailable => BackendKind::DeviceNotAvailable,
        cpal::ErrorKind::HostUnavailable => BackendKind::HostUnavailable,
        cpal::ErrorKind::InvalidInput => BackendKind::InvalidInput,
        cpal::ErrorKind::PermissionDenied => BackendKind::PermissionDenied,
        cpal::ErrorKind::RealtimeDenied => BackendKind::RealtimeDenied,
        cpal::ErrorKind::ResourceExhausted => BackendKind::ResourceExhausted,
        cpal::ErrorKind::StreamInvalidated => BackendKind::StreamInvalidated,
        cpal::ErrorKind::UnsupportedConfig => BackendKind::UnsupportedConfig,
        cpal::ErrorKind::UnsupportedOperation => BackendKind::UnsupportedOperation,
        cpal::ErrorKind::Xrun => BackendKind::Xrun,
        _ => BackendKind::Other,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureStopTimings {
    pub began: Instant,
    pub stream_stop_ms: u64,
    pub conversion_ms: u64,
}

struct Shared {
    samples: Mutex<RawSamples>,
    max_frames: usize,
    channels: usize,
    level_bits: AtomicU32,
    callback_count: AtomicUsize,
    priority_boosted: AtomicBool,
    frame_count: AtomicUsize,
    failed: AtomicBool,
    startup_xruns: AtomicUsize,
    /// Every Xrun the backend reported, tolerated or not.
    xruns: AtomicUsize,
    /// Callback count and captured frames when the first fatal stream error arrived;
    /// usize::MAX until then. Positions only, never audio.
    fault_callback: AtomicUsize,
    fault_frames: AtomicUsize,
    last_error: Mutex<Option<String>>,
    last_error_kind: Mutex<Option<BackendKind>>,
    overflow: AtomicBool,
    max_duration_reached: AtomicBool,
    /// A mono copy for recognition while speaking, emptied by the reader about once a
    /// second. Separate from `samples`, so reading never makes the callback's
    /// `try_lock` on the recording fail.
    tap: Mutex<Vec<f32>>,
    /// The copy is never allowed to grow past this (no reader): it stops instead.
    tap_limit: usize,
    tap_overflow: AtomicBool,
    /// Captured frames in the first 0.1 s: one Xrun this early is a start-up gap (before
    /// anyone speaks), reported as a warning instead of failing the recording.
    startup_frames: usize,
}

trait ToF32 {
    fn to_f32(self) -> f32;
}
impl ToF32 for f32 {
    fn to_f32(self) -> f32 {
        self
    }
}
impl ToF32 for i16 {
    fn to_f32(self) -> f32 {
        self as f32 / 32768.0
    }
}
impl ToF32 for u16 {
    fn to_f32(self) -> f32 {
        (self as f32 - 32768.0) / 32768.0
    }
}

enum RawSamples {
    F32(Vec<f32>),
    I16(Vec<i16>),
    U16(Vec<u16>),
}

impl RawSamples {
    fn frame_len(&self, channels: usize) -> usize {
        match self {
            Self::F32(samples) => samples.len() / channels,
            Self::I16(samples) => samples.len() / channels,
            Self::U16(samples) => samples.len() / channels,
        }
    }

    fn into_mono(self, channels: usize) -> Vec<f32> {
        fn convert<T: ToF32 + Copy>(samples: Vec<T>, channels: usize) -> Vec<f32> {
            samples
                .chunks_exact(channels)
                .map(|frame| {
                    let mono =
                        frame.iter().map(|sample| sample.to_f32()).sum::<f32>() / channels as f32;
                    if mono.is_finite() {
                        mono.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .collect()
        }
        match self {
            Self::F32(samples) => convert(samples, channels),
            Self::I16(samples) => convert(samples, channels),
            Self::U16(samples) => convert(samples, channels),
        }
    }
}

trait CaptureSample: cpal::SizedSample + ToF32 + Copy + Send + 'static {
    fn append(buffer: &mut RawSamples, data: &[Self]) -> bool;
}

impl CaptureSample for f32 {
    fn append(buffer: &mut RawSamples, data: &[Self]) -> bool {
        if let RawSamples::F32(samples) = buffer {
            samples.extend_from_slice(data);
            true
        } else {
            false
        }
    }
}
impl CaptureSample for i16 {
    fn append(buffer: &mut RawSamples, data: &[Self]) -> bool {
        if let RawSamples::I16(samples) = buffer {
            samples.extend_from_slice(data);
            true
        } else {
            false
        }
    }
}
impl CaptureSample for u16 {
    fn append(buffer: &mut RawSamples, data: &[Self]) -> bool {
        if let RawSamples::U16(samples) = buffer {
            samples.extend_from_slice(data);
            true
        } else {
            false
        }
    }
}

/// CPAL's WASAPI thread is only promoted with its optional `realtime` feature, which is
/// off here, so the capture thread would compete at normal priority and lose packets
/// (reported as Xrun) under load. The data callback runs on that thread: join the
/// Multimedia Class Scheduler "Pro Audio" task, or fall back to a high thread priority.
#[cfg(windows)]
fn boost_capture_thread() -> bool {
    use windows_sys::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    };
    // SAFETY: plain Win32 calls on the current thread with a static NUL-terminated name.
    unsafe {
        let mut task_index = 0_u32;
        if !AvSetMmThreadCharacteristicsW(windows_sys::w!("Pro Audio"), &mut task_index).is_null() {
            return true;
        }
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) != 0
    }
}

#[cfg(not(windows))]
fn boost_capture_thread() -> bool {
    false
}

/// Register with the scheduler once at start-up, on a throwaway thread. The first
/// registration in a process sets up its connection to the scheduler service; done inside
/// the first data callback of the first recording, it may delay that callback enough to
/// lose a packet (hypothesis for the Xrun 0.02 s into the first recording after start).
#[cfg(windows)]
pub fn prepare_capture_priority() {
    use windows_sys::Win32::System::Threading::{
        AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    };
    let _ = std::thread::Builder::new()
        .name("capture-priority-warmup".into())
        .spawn(|| {
            // SAFETY: plain Win32 calls on this thread; the handle is reverted right away.
            unsafe {
                let mut task_index = 0_u32;
                let handle =
                    AvSetMmThreadCharacteristicsW(windows_sys::w!("Pro Audio"), &mut task_index);
                if !handle.is_null() {
                    AvRevertMmThreadCharacteristics(handle);
                }
            }
        });
}

#[cfg(not(windows))]
pub fn prepare_capture_priority() {}

/// `carry` holds copy frames the callback could not hand over yet (the reader held the
/// copy's lock at that instant); they go with the next block.
fn consume<T: CaptureSample>(data: &[T], shared: &Shared, carry: &mut Vec<f32>) {
    if shared.callback_count.fetch_add(1, Ordering::Relaxed) == 0 {
        let boosted = boost_capture_thread();
        shared.priority_boosted.store(boosted, Ordering::Release);
    }
    let Ok(mut samples) = shared.samples.try_lock() else {
        shared.overflow.store(true, Ordering::Release);
        return;
    };
    let frames = data.len() / shared.channels;
    let available = shared.max_frames - samples.frame_len(shared.channels);
    let captured = frames.min(available);
    if captured != frames {
        shared.max_duration_reached.store(true, Ordering::Release);
    }
    if !data.len().is_multiple_of(shared.channels) {
        shared.overflow.store(true, Ordering::Release);
    }
    let data = &data[..captured * shared.channels];
    if !T::append(&mut samples, data) {
        shared.failed.store(true, Ordering::Release);
        return;
    }
    let peak = data
        .iter()
        .map(|sample| sample.to_f32().abs())
        .fold(0.0_f32, f32::max);
    shared.frame_count.fetch_add(captured, Ordering::Release);
    shared.level_bits.store(peak.to_bits(), Ordering::Release);
    if !shared.tap_overflow.load(Ordering::Relaxed) {
        carry.extend(data.chunks_exact(shared.channels).map(|frame| {
            let mono =
                frame.iter().map(|sample| sample.to_f32()).sum::<f32>() / shared.channels as f32;
            if mono.is_finite() {
                mono.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        }));
        if let Ok(mut tap) = shared.tap.try_lock() {
            if tap.len() + carry.len() > shared.tap_limit {
                shared.tap_overflow.store(true, Ordering::Relaxed);
            } else {
                tap.extend_from_slice(carry);
            }
            carry.clear();
        }
    }
}

fn mark_stream_error(shared: &Shared, kind: cpal::ErrorKind) {
    if kind == cpal::ErrorKind::Xrun {
        shared.xruns.fetch_add(1, Ordering::AcqRel);
    }
    let callbacks = shared.callback_count.load(Ordering::Acquire);
    // The stream settling in: before any audio, or within the first 0.1 s of it (seen
    // 2026-10-17 after the 2nd packet of the first recording after start, 0.02 s in).
    let starting =
        callbacks == 0 || shared.frame_count.load(Ordering::Acquire) < shared.startup_frames;
    if kind == cpal::ErrorKind::Xrun
        && starting
        && shared.startup_xruns.fetch_add(1, Ordering::AcqRel) == 0
    {
        // The start may have a gap, so the caller must see this warning.
    } else {
        // Remember where the first fatal error happened, to tell a start-up gap from
        // one in the middle of speech. Later errors keep the first position.
        if shared
            .fault_callback
            .compare_exchange(usize::MAX, callbacks, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            shared.fault_frames.store(
                shared.frame_count.load(Ordering::Acquire),
                Ordering::Release,
            );
        }
        shared.failed.store(true, Ordering::Release);
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: Arc<Shared>,
) -> Result<Stream, AudioError>
where
    T: CaptureSample,
{
    let on_data = shared.clone();
    let on_error = shared;
    // Room for a second of copy frames, so handing them over rarely allocates.
    let mut carry = Vec::with_capacity(config.sample_rate as usize);
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data, _| consume(data, &on_data, &mut carry),
            move |error| {
                if let Ok(mut slot) = on_error.last_error.lock() {
                    *slot = Some(error.to_string());
                }
                if let Ok(mut slot) = on_error.last_error_kind.lock() {
                    *slot = Some(backend_kind(error.kind()));
                }
                mark_stream_error(&on_error, error.kind());
            },
            None,
        )
        .map_err(|error| AudioError::BuildStreamFailed(error.kind()))
}

pub struct AudioCapture {
    stream: Stream,
    shared: Arc<Shared>,
    sample_rate: u32,
}

/// The mono copy of the recording in progress, for recognition while speaking.
#[derive(Clone)]
pub struct AudioTap {
    shared: Arc<Shared>,
    pub sample_rate: u32,
}

impl AudioTap {
    /// The frames recorded since the last call, at the device rate. None if the copy
    /// stopped (nobody read it for too long): the recording then goes the usual way.
    pub fn drain(&self) -> Option<Vec<f32>> {
        if self.shared.tap_overflow.load(Ordering::Acquire) {
            return None;
        }
        // Swapping the vector out keeps the lock for an instant.
        self.shared
            .tap
            .lock()
            .ok()
            .map(|mut tap| std::mem::take(&mut *tap))
    }
}

pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub startup_xrun: bool,
    pub max_duration_reached: bool,
}

/// An input device as the settings screen lists it. The id is CPAL's persistent
/// `host:device` string; the name is for display only.
#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct InputDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Input devices currently present. Devices that cannot report an id are skipped,
/// since they could not be selected again later.
pub fn input_devices() -> Vec<InputDeviceInfo> {
    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|device| {
            let id = device.id().ok()?.to_string();
            let name = device
                .description()
                .map(|description| description.name().to_owned())
                .unwrap_or_else(|_| id.clone());
            Some(InputDeviceInfo {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name,
            })
        })
        .collect()
}

impl AudioCapture {
    pub fn start_default() -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?;
        Self::start_device(device)
    }

    /// Start on the preferred device when it is present, otherwise on the system
    /// default. The flag reports that the preferred device was missing, so the caller
    /// can tell the user; the preference itself is kept for when it returns.
    pub fn start_preferred(preferred: Option<&str>) -> Result<(Self, bool), AudioError> {
        let Some(preferred) = preferred else {
            return Ok((Self::start_default()?, false));
        };
        let host = cpal::default_host();
        let device = preferred
            .parse::<cpal::DeviceId>()
            .ok()
            .and_then(|id| host.device_by_id(&id));
        match device {
            Some(device) => Ok((Self::start_device(device)?, false)),
            None => Ok((Self::start_default()?, true)),
        }
    }

    pub fn start_device(device: cpal::Device) -> Result<Self, AudioError> {
        let supported = device
            .default_input_config()
            .map_err(|error| AudioError::InputConfigFailed(error.kind()))?;
        let sample_rate = supported.sample_rate();
        let channels = usize::from(supported.channels());
        if sample_rate == 0 || sample_rate > 96_000 || channels == 0 {
            return Err(AudioError::UnsupportedFormat);
        }
        let max_frames = sample_rate as usize * MAX_CAPTURE_SECONDS as usize;
        let max_values = max_frames
            .checked_mul(channels)
            .ok_or(AudioError::UnsupportedFormat)?;
        const MAX_RAW_BYTES: usize = 256 * 1024 * 1024;
        if max_values
            .checked_mul(std::mem::size_of::<f32>())
            .is_none_or(|bytes| bytes > MAX_RAW_BYTES)
        {
            return Err(AudioError::UnsupportedFormat);
        }
        let samples = match supported.sample_format() {
            SampleFormat::F32 => RawSamples::F32(Vec::with_capacity(max_values)),
            SampleFormat::I16 => RawSamples::I16(Vec::with_capacity(max_values)),
            SampleFormat::U16 => RawSamples::U16(Vec::with_capacity(max_values)),
            _ => return Err(AudioError::UnsupportedFormat),
        };
        let shared = Arc::new(Shared {
            samples: Mutex::new(samples),
            max_frames,
            channels,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: sample_rate as usize * 60,
            tap_overflow: AtomicBool::new(false),
            startup_frames: sample_rate as usize / 10,
        });
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(&device, config, shared.clone())?,
            SampleFormat::I16 => build_stream::<i16>(&device, config, shared.clone())?,
            SampleFormat::U16 => build_stream::<u16>(&device, config, shared.clone())?,
            _ => return Err(AudioError::UnsupportedFormat),
        };
        stream
            .play()
            .map_err(|error| AudioError::PlayStreamFailed(error.kind()))?;
        Ok(Self {
            stream,
            shared,
            sample_rate,
        })
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level_bits.load(Ordering::Acquire))
    }

    pub fn tap(&self) -> AudioTap {
        AudioTap {
            shared: self.shared.clone(),
            sample_rate: self.sample_rate,
        }
    }

    pub fn capture_counts(&self) -> (usize, usize) {
        (
            self.shared.callback_count.load(Ordering::Acquire),
            self.shared.frame_count.load(Ordering::Acquire),
        )
    }

    pub fn max_duration_reached(&self) -> bool {
        self.shared.max_duration_reached.load(Ordering::Acquire)
    }

    pub fn has_capture_fault(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire) || self.shared.overflow.load(Ordering::Acquire)
    }

    /// Whether the capture thread was promoted. Meaningful once a callback has run.
    pub fn priority_boosted(&self) -> bool {
        self.shared.priority_boosted.load(Ordering::Acquire)
    }

    pub fn startup_xrun_count(&self) -> usize {
        self.shared.startup_xruns.load(Ordering::Acquire)
    }

    pub fn capture_error(&self) -> Option<String> {
        self.shared
            .last_error
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// Callback index, milliseconds of captured audio, and total Xruns at the first
    /// fatal stream error. None when no fatal stream error happened.
    pub fn fault_position(&self) -> Option<(usize, u64, usize)> {
        let callback = self.shared.fault_callback.load(Ordering::Acquire);
        if callback == usize::MAX {
            return None;
        }
        let frames = self.shared.fault_frames.load(Ordering::Acquire);
        let audio_ms = (frames as u64).saturating_mul(1000) / u64::from(self.sample_rate.max(1));
        Some((
            callback,
            audio_ms,
            self.shared.xruns.load(Ordering::Acquire),
        ))
    }

    pub fn capture_error_kind(&self) -> Option<BackendKind> {
        self.shared
            .last_error_kind
            .lock()
            .ok()
            .and_then(|kind| *kind)
    }

    pub fn stop(self) -> Result<CapturedAudio, AudioError> {
        self.stop_measured().0
    }

    pub fn stop_measured(self) -> (Result<CapturedAudio, AudioError>, CaptureStopTimings) {
        let began = Instant::now();
        drop(self.stream);
        let stream_stop_ms = elapsed_ms(began);
        let conversion = Instant::now();
        let result = (|| {
            if self.shared.failed.load(Ordering::Acquire) {
                return Err(AudioError::CaptureFailed);
            }
            if self.shared.overflow.load(Ordering::Acquire) {
                return Err(AudioError::Overflow);
            }
            let mut guard = self
                .shared
                .samples
                .lock()
                .map_err(|_| AudioError::CaptureFailed)?;
            let samples = std::mem::replace(&mut *guard, RawSamples::F32(Vec::new()));
            if samples.frame_len(self.shared.channels) < (self.sample_rate as usize * 3 / 10) {
                return Err(AudioError::TooShort);
            }
            let samples = resample_mono(samples.into_mono(self.shared.channels), self.sample_rate)?;
            Ok(CapturedAudio {
                samples,
                startup_xrun: self.shared.startup_xruns.load(Ordering::Acquire) != 0,
                max_duration_reached: self.shared.max_duration_reached.load(Ordering::Acquire),
            })
        })();
        (
            result,
            CaptureStopTimings {
                began,
                stream_stop_ms,
                conversion_ms: elapsed_ms(conversion),
            },
        )
    }

    pub fn cancel(self) {
        drop(self.stream);
    }
}

pub fn resample_mono(samples: Vec<f32>, input_rate: u32) -> Result<Vec<f32>, AudioError> {
    if input_rate == 0 || samples.is_empty() || samples.iter().any(|x| !x.is_finite()) {
        return Err(AudioError::ConversionFailed);
    }
    if input_rate == OUTPUT_RATE {
        return Ok(samples);
    }
    let input_len = samples.len();
    let input = InterleavedOwned::new_from(samples, 1, input_len)
        .map_err(|_| AudioError::ConversionFailed)?;
    let mut resampler = Fft::<f32>::new(
        input_rate as usize,
        OUTPUT_RATE as usize,
        1024,
        1,
        FixedSync::Input,
    )
    .map_err(|_| AudioError::ConversionFailed)?;
    let output = resampler
        .process_all(&input, input_len, None)
        .map_err(|_| AudioError::ConversionFailed)?;
    Ok(output.take_data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_failure_categories_preserve_only_fixed_backend_codes() {
        assert_eq!(
            AudioError::InputConfigFailed(cpal::ErrorKind::PermissionDenied).diagnostic(),
            (
                FailureCode::AudioConfig,
                Some(BackendKind::PermissionDenied)
            )
        );
        assert_eq!(
            AudioError::BuildStreamFailed(cpal::ErrorKind::DeviceBusy).diagnostic(),
            (FailureCode::AudioBuild, Some(BackendKind::DeviceBusy))
        );
        assert_eq!(
            AudioError::PlayStreamFailed(cpal::ErrorKind::StreamInvalidated).diagnostic(),
            (FailureCode::AudioPlay, Some(BackendKind::StreamInvalidated))
        );
        assert_eq!(
            AudioError::TooShort.diagnostic(),
            (FailureCode::AudioTooShort, None)
        );
        assert_eq!(backend_kind(cpal::ErrorKind::Xrun), BackendKind::Xrun);
    }
    #[test]
    fn resamples_common_device_rates_without_losing_tail() {
        for rate in [44_100, 48_000] {
            let samples: Vec<f32> = (0..rate)
                .map(|i| ((i as f32 / rate as f32) * 2.0 * std::f32::consts::PI * 440.0).sin())
                .collect();
            let output = resample_mono(samples, rate).unwrap();
            assert!(
                (15_990..=16_010).contains(&output.len()),
                "rate {rate}: {}",
                output.len()
            );
            assert!(output.iter().all(|x| x.is_finite()));
            assert!(output[output.len() - 100..].iter().any(|x| x.abs() > 0.1));
        }
    }
    #[cfg(windows)]
    #[test]
    fn first_callback_promotes_only_its_own_thread() {
        // A dedicated thread stands in for CPAL's capture thread; the test thread is untouched.
        let promoted = std::thread::spawn(boost_capture_thread).join().unwrap();
        assert!(promoted, "MMCSS or high-priority fallback must succeed");
    }

    #[test]
    fn only_one_pre_callback_xrun_is_a_warning() {
        let shared = Shared {
            samples: Mutex::new(RawSamples::F32(Vec::new())),
            max_frames: 48_000,
            channels: 1,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 1_000,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 4_800,
        };
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        assert_eq!(shared.startup_xruns.load(Ordering::Acquire), 1);
        assert!(!shared.failed.load(Ordering::Acquire));

        shared.callback_count.store(1, Ordering::Release);
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        assert!(shared.failed.load(Ordering::Acquire));
    }

    #[test]
    fn one_xrun_in_the_first_tenth_of_a_second_is_a_warning() {
        let shared = Shared {
            samples: Mutex::new(RawSamples::F32(Vec::with_capacity(48_000))),
            max_frames: 48_000,
            channels: 1,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 1_000,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 4_800,
        };
        // Two 10 ms packets (0.02 s), then a lost one: a start-up gap.
        consume(&[0.0_f32; 480], &shared, &mut Vec::new());
        consume(&[0.0_f32; 480], &shared, &mut Vec::new());
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        assert!(!shared.failed.load(Ordering::Acquire));
        assert_eq!(shared.startup_xruns.load(Ordering::Acquire), 1);
        // A second one, or one later in the recording, still fails it.
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        assert!(shared.failed.load(Ordering::Acquire));
    }

    #[test]
    fn an_xrun_after_the_first_tenth_of_a_second_fails() {
        let shared = Shared {
            samples: Mutex::new(RawSamples::F32(Vec::with_capacity(48_000))),
            max_frames: 48_000,
            channels: 1,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 1_000,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 4_800,
        };
        consume(&[0.0_f32; 4_800], &shared, &mut Vec::new());
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        assert!(shared.failed.load(Ordering::Acquire));
        assert_eq!(shared.startup_xruns.load(Ordering::Acquire), 0);
    }

    #[test]
    fn repeated_pre_callback_xrun_fails() {
        let shared = Shared {
            samples: Mutex::new(RawSamples::F32(Vec::new())),
            max_frames: 48_000,
            channels: 1,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 1_000,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 4_800,
        };
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        assert!(shared.failed.load(Ordering::Acquire));
        // The first (tolerated) Xrun is not the fault; the second is, before any audio.
        assert_eq!(shared.fault_callback.load(Ordering::Acquire), 0);
        assert_eq!(shared.fault_frames.load(Ordering::Acquire), 0);
        assert_eq!(shared.xruns.load(Ordering::Acquire), 2);
    }

    #[test]
    fn a_mid_recording_xrun_records_where_it_happened_and_keeps_the_first() {
        let shared = Shared {
            samples: Mutex::new(RawSamples::F32(Vec::with_capacity(8))),
            max_frames: 8,
            channels: 1,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 1_000,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 0,
        };
        consume(&[0.1_f32, 0.2], &shared, &mut Vec::new());
        consume(&[0.3_f32], &shared, &mut Vec::new());
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        consume(&[0.4_f32], &shared, &mut Vec::new());
        mark_stream_error(&shared, cpal::ErrorKind::Xrun);
        assert!(shared.failed.load(Ordering::Acquire));
        assert_eq!(shared.fault_callback.load(Ordering::Acquire), 2);
        assert_eq!(shared.fault_frames.load(Ordering::Acquire), 3);
        assert_eq!(shared.xruns.load(Ordering::Acquire), 2);
    }

    #[test]
    fn stereo_raw_samples_convert_after_capture() {
        let shared = Shared {
            samples: Mutex::new(RawSamples::I16(Vec::with_capacity(8))),
            max_frames: 4,
            channels: 2,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 1_000,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 4_800,
        };
        consume(&[32767_i16, -32768, 16384, 16384], &shared, &mut Vec::new());
        assert_eq!(shared.frame_count.load(Ordering::Acquire), 2);
        let buffer = std::mem::replace(
            &mut *shared.samples.lock().unwrap(),
            RawSamples::I16(Vec::new()),
        );
        let mono = buffer.into_mono(2);
        assert!(mono[0].abs() < 0.0001);
        assert!((mono[1] - 0.5).abs() < 0.0001);
        assert!(!shared.overflow.load(Ordering::Acquire));
    }

    #[test]
    fn reading_the_copy_never_fails_the_recording() {
        let shared = Arc::new(Shared {
            samples: Mutex::new(RawSamples::F32(Vec::with_capacity(16))),
            max_frames: 16,
            channels: 1,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(1),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 8,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 4_800,
        });
        let tap = AudioTap {
            shared: shared.clone(),
            sample_rate: 16_000,
        };
        let mut carry = Vec::new();
        consume(&[0.1_f32, 0.2], &shared, &mut carry);
        // The reader holds the copy while the next block arrives: the recording goes on
        // and the block waits in `carry`.
        {
            let _held = shared.tap.lock().unwrap();
            consume(&[0.3_f32], &shared, &mut carry);
        }
        assert!(!shared.overflow.load(Ordering::Acquire));
        assert_eq!(carry, vec![0.3]);
        consume(&[0.4_f32], &shared, &mut carry);
        assert_eq!(tap.drain(), Some(vec![0.1, 0.2, 0.3, 0.4]));
        assert_eq!(tap.drain(), Some(vec![]));
        assert_eq!(shared.frame_count.load(Ordering::Acquire), 4);
        // Nobody reads: the copy stops at its limit, the recording itself goes on.
        consume(&[0.0_f32; 9], &shared, &mut carry);
        assert_eq!(tap.drain(), None);
        assert!(!shared.overflow.load(Ordering::Acquire));
    }

    #[test]
    fn raw_buffer_duration_limit_is_reported() {
        let shared = Shared {
            samples: Mutex::new(RawSamples::F32(Vec::with_capacity(2))),
            max_frames: 2,
            channels: 1,
            level_bits: AtomicU32::new(0),
            callback_count: AtomicUsize::new(0),
            priority_boosted: AtomicBool::new(false),
            frame_count: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            startup_xruns: AtomicUsize::new(0),
            xruns: AtomicUsize::new(0),
            fault_callback: AtomicUsize::new(usize::MAX),
            fault_frames: AtomicUsize::new(usize::MAX),
            last_error: Mutex::new(None),
            last_error_kind: Mutex::new(None),
            overflow: AtomicBool::new(false),
            max_duration_reached: AtomicBool::new(false),
            tap: Mutex::new(Vec::new()),
            tap_limit: 1_000,
            tap_overflow: AtomicBool::new(false),
            startup_frames: 4_800,
        };
        consume(&[0.1_f32, 0.2, 0.3], &shared, &mut Vec::new());
        assert_eq!(shared.frame_count.load(Ordering::Acquire), 2);
        assert!(shared.max_duration_reached.load(Ordering::Acquire));
        assert!(!shared.overflow.load(Ordering::Acquire));
    }

    #[test]
    fn rejects_nonfinite_samples() {
        assert_eq!(
            resample_mono(vec![f32::NAN], 48_000),
            Err(AudioError::ConversionFailed)
        );
    }
}
