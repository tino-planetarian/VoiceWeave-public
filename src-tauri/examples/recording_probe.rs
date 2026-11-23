use std::thread;
use std::time::Duration;
use uuid::Uuid;
use voiceweave_lib::recording::{CaptureEvent, CaptureService, CaptureStopReason};

fn main() {
    let limit_seconds = std::env::args().skip(1).find_map(|argument| {
        if argument == "--limit" {
            Some(300)
        } else {
            argument
                .strip_prefix("--limit=")
                .and_then(|value| value.parse::<u64>().ok())
        }
    });
    let service = match limit_seconds {
        Some(seconds) => CaptureService::spawn_with_duration(Duration::from_secs(seconds)),
        None => CaptureService::spawn(),
    }
    .expect("start audio owner thread");
    let session_id = Uuid::new_v4();
    service.start(session_id).expect("request capture start");
    match service.recv_timeout(Duration::from_secs(10)) {
        Ok(CaptureEvent::Started(id)) if id == session_id => {}
        Ok(CaptureEvent::StartFailed(_, error)) => panic!("capture start failed: {error:?}"),
        _ => panic!("missing capture start event"),
    }

    if let Some(seconds) = limit_seconds {
        match service.recv_timeout(Duration::from_secs(seconds + 90)) {
            Ok(CaptureEvent::Stopped {
                session_id: id,
                reason,
                diagnostics,
                result,
            }) if id == session_id => {
                let audio = result.unwrap_or_else(|error| {
                    panic!(
                        "automatic capture stop failed: {error:?}; reason={reason:?}; callbacks={}; frames={}; startup_xruns={}; stream_error={:?}",
                        diagnostics.callbacks, diagnostics.frames, diagnostics.startup_xruns, diagnostics.stream_error
                    )
                });
                assert_eq!(reason, CaptureStopReason::MaxDuration);
                println!(
                    "auto_stopped: callbacks={}; frames={}; startup_xruns={}; 16k_samples={}; max_duration_reached={}",
                    diagnostics.callbacks,
                    diagnostics.frames,
                    diagnostics.startup_xruns,
                    audio.samples.len(),
                    audio.max_duration_reached,
                );
                return;
            }
            Ok(_) => panic!("unexpected automatic capture event"),
            Err(error) => {
                let (callbacks, frames) = service.capture_counts();
                panic!("automatic capture event unavailable: {error:?}; callbacks={callbacks}; frames={frames}")
            }
        }
    }

    thread::sleep(Duration::from_secs(2));
    let level = service.level();
    service.stop(session_id).expect("request capture stop");
    match service.recv_timeout(Duration::from_secs(20)) {
        Ok(CaptureEvent::Stopped {
            session_id: id,
            reason,
            diagnostics,
            result,
        }) if id == session_id => {
            let audio = result.unwrap_or_else(|error| {
                panic!(
                    "capture stop failed: {error:?}; reason={reason:?}; callbacks={}; frames={}; startup_xruns={}; stream_error={:?}",
                    diagnostics.callbacks, diagnostics.frames, diagnostics.startup_xruns, diagnostics.stream_error
                )
            });
            assert_eq!(reason, CaptureStopReason::User);
            println!(
                "stopped: callbacks={}; frames={}; startup_xruns={}; 16k_samples={}; max_duration_reached={}; last_level={level:.3}",
                diagnostics.callbacks,
                diagnostics.frames,
                diagnostics.startup_xruns,
                audio.samples.len(),
                audio.max_duration_reached,
            );
        }
        Ok(CaptureEvent::Rejected(id)) => panic!("stop rejected: id_matches={}", id == session_id),
        Ok(_) => panic!("unexpected event while stopping"),
        Err(error) => panic!("capture stop event unavailable: {error:?}"),
    }

    let cancelled_id = Uuid::new_v4();
    service.start(cancelled_id).expect("request second capture");
    match service.recv_timeout(Duration::from_secs(10)) {
        Ok(CaptureEvent::Started(id)) if id == cancelled_id => {}
        _ => panic!("missing second capture start event"),
    }
    service
        .cancel(cancelled_id)
        .expect("request capture cancel");
    match service.recv_timeout(Duration::from_secs(10)) {
        Ok(CaptureEvent::Cancelled(id)) if id == cancelled_id => {
            println!("cancelled second capture")
        }
        _ => panic!("missing capture cancel event"),
    }
}
