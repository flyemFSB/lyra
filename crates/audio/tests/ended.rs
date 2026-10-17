//! 播放结束事件回归测试：验证曲目自然播完触发 Event::Ended。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lyra_audio::{AudioConfig, Engine, Event, FetchResult, Fetcher, ReadSeek, State, TrackRef, spawn_with};

/// 本地文件抓取器
struct FileFetcher(PathBuf);

impl Fetcher for FileFetcher {
    fn open(&self, _track: &TrackRef) -> FetchResult<Box<dyn ReadSeek + Send>> {
        Ok(Box::new(std::fs::File::open(&self.0).unwrap()))
    }
}

/// 生成测试正弦波 WAV 文件
fn write_sine_wav(path: &Path, seconds: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..44_100 * seconds {
        let sample = ((i as f32 * 0.01).sin() * 12_000.0) as i16;
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

/// 创建收集事件的测试引擎
fn engine_with_events(path: &Path, events: Arc<Mutex<Vec<Event>>>) -> Option<Engine> {
    spawn_with(
        Arc::new(FileFetcher(path.to_path_buf())),
        Box::new(move |event| events.lock().unwrap().push(event)),
        AudioConfig::default(),
    )
    .ok()
}

#[test]
fn a_finished_track_emits_ended() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sine.wav");
    write_sine_wav(&path, 1);
    let events = Arc::new(Mutex::new(Vec::new()));
    let Some(engine) = engine_with_events(&path, events.clone()) else {
        return;
    };

    engine.play(TrackRef {
        hash: "probe".to_owned(),
        url: "file://probe".to_owned(),
        duration_ms: 1_000,
        ext: "wav".to_owned(),
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if events.lock().unwrap().iter().any(|e| matches!(e, Event::Ended)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let events = events.lock().unwrap().clone();
    assert!(
        events.iter().any(|e| matches!(e, Event::Ended)),
        "1 秒音频播完后应抛出 Ended，实际事件: {events:?}"
    );
    assert_eq!(engine.snapshot().state, State::Idle);
}
