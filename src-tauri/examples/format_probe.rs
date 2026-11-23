use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn build<T: cpal::SizedSample + Send + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    callbacks: Arc<AtomicUsize>,
    frames: Arc<AtomicUsize>,
    errors: Arc<Mutex<Vec<(usize, String)>>>,
) -> Stream {
    let callback_count_for_error = callbacks.clone();
    device
        .build_input_stream::<T, _, _>(
            *config,
            move |data, _| {
                callbacks.fetch_add(1, Ordering::Relaxed);
                frames.fetch_add(data.len(), Ordering::Relaxed);
            },
            move |cause| {
                if let Ok(mut slot) = errors.lock() {
                    if slot.len() < 8 {
                        slot.push((
                            callback_count_for_error.load(Ordering::Relaxed),
                            cause.to_string(),
                        ));
                    }
                }
            },
            None,
        )
        .expect("build input stream")
}

fn main() {
    let format = std::env::args().nth(1).expect("f32 or i16");
    let index: usize = std::env::args()
        .nth(2)
        .expect("device index")
        .parse()
        .expect("numeric index");
    let device = cpal::default_host()
        .input_devices()
        .expect("input devices")
        .nth(index)
        .expect("device index exists");
    let config = device
        .default_input_config()
        .expect("default config")
        .config();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let frames = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let stream = match format.as_str() {
        "f32" => build::<f32>(
            &device,
            &config,
            callbacks.clone(),
            frames.clone(),
            errors.clone(),
        ),
        "i16" => build::<i16>(
            &device,
            &config,
            callbacks.clone(),
            frames.clone(),
            errors.clone(),
        ),
        _ => panic!("expected f32 or i16"),
    };
    stream.play().expect("play stream");
    thread::sleep(Duration::from_secs(2));
    eprintln!(
        "format={format}; channels={}; rate={}; callbacks={}; sample_values={}; errors={:?}",
        config.channels,
        config.sample_rate,
        callbacks.load(Ordering::Relaxed),
        frames.load(Ordering::Relaxed),
        errors.lock().ok().map(|slot| slot.clone())
    );
    drop(stream);
    eprintln!("stopped");
}
