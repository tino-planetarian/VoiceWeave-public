use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let verbose_index = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok());
    let host = cpal::default_host();
    if let Some(default) = host.default_input_device() {
        println!(
            "default: {}",
            default
                .description()
                .map(|d| d.name().to_owned())
                .unwrap_or_else(|_| "unknown".into())
        );
    }
    let devices = host.input_devices().expect("input devices");
    for (index, device) in devices.enumerate() {
        let config = device.default_input_config().expect("default config");
        println!(
            "{index}: {} — {:?} Hz, {} channels, {:?}",
            device
                .description()
                .map(|d| d.name().to_owned())
                .unwrap_or_else(|_| "unknown".into()),
            config.sample_rate(),
            config.channels(),
            config.sample_format()
        );
        if Some(index) == verbose_index {
            match device.supported_input_configs() {
                Ok(configs) => {
                    for supported in configs {
                        println!("  supported: {supported:?}");
                    }
                }
                Err(error) => eprintln!("  supported config query failed: {error}"),
            }
        }
    }
}
