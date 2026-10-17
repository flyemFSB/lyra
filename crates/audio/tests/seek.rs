//! 定位回归：引擎一律走「重开解码器 + 顺序丢弃」，不依赖 symphonia 的 seek。
//!
//! 修复前 `Session::seek` 调用 symphonia 的 `SeekMode::Accurate`，而 Ogg 解复用器在
//! 数据源报告 `byte_len() == None`（边下边播的 GrowingFile 就是如此）时会 `unwrap()`
//! panic —— release 是 panic=abort，整个进程闪退。
//!
//! 真实 Ogg 用例需要外部文件（仓库不放受版权保护的音频）：
//! `$env:LYRA_PROBE_OGG="<某个 ogg 路径>"; cargo test -p lyra-audio --test seek -- --nocapture`

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lyra_audio::{AudioConfig, Engine, FetchResult, Fetcher, ReadSeek, State, TrackRef, spawn_with};

/// 直接读取本地文件的抓取器。
struct FileFetcher(PathBuf);

impl Fetcher for FileFetcher {
    fn open(&self, _track: &TrackRef) -> FetchResult<Box<dyn ReadSeek + Send>> {
        Ok(Box::new(std::fs::File::open(&self.0).unwrap()))
    }
}

/// 打开播放本地文件的引擎；无声卡环境返回 `None`（CI 上跳过）。
fn engine_for(path: &Path) -> Option<Engine> {
    spawn_with(
        Arc::new(FileFetcher(path.to_path_buf())),
        Box::new(|_event| {}),
        AudioConfig::default(),
    )
    .ok()
}

/// 构造曲目信息。
fn track(ext: &str, duration_ms: u64) -> TrackRef {
    TrackRef {
        hash: "probe".to_owned(),
        url: "file://probe".to_owned(),
        duration_ms,
        ext: ext.to_owned(),
    }
}

/// 生成指定秒数、44.1kHz 单声道 16bit 正弦 WAV。
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

#[test]
fn seek_moves_the_position_forward_and_backward() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sine.wav");
    write_sine_wav(&path, 20);
    let Some(engine) = engine_for(&path) else {
        return;
    };

    engine.play(track("wav", 20_000));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(engine.snapshot().state, State::Playing);

    engine.seek(6_000);
    std::thread::sleep(Duration::from_millis(400));
    let forward = engine.snapshot().position_ms;
    assert!((6_000..7_500).contains(&forward), "前进定位后进度 {forward}");

    engine.seek(1_000);
    std::thread::sleep(Duration::from_millis(400));
    let backward = engine.snapshot().position_ms;
    assert!((1_000..3_000).contains(&backward), "回退定位后进度 {backward}");
}

#[test]
fn seek_on_a_real_ogg_file_keeps_the_engine_alive() {
    let Ok(path) = std::env::var("LYRA_PROBE_OGG") else {
        return;
    };
    let Some(engine) = engine_for(Path::new(&path)) else {
        return;
    };

    engine.play(track("ogg", 100_000));
    std::thread::sleep(Duration::from_millis(800));
    for ms in [0_u64, 30_000, 60_000] {
        let started = Instant::now();
        engine.seek(ms);
        // 等到进度追上目标位置（引擎定位完成时会直接写入目标进度）。
        while started.elapsed() < Duration::from_secs(30) && engine.snapshot().position_ms < ms {
            std::thread::sleep(Duration::from_millis(20));
        }
        println!(
            "定位 {ms} ms 耗时 {:?}，进度 {}",
            started.elapsed(),
            engine.snapshot().position_ms
        );
        assert!(
            engine.snapshot().position_ms >= ms,
            "定位 {ms} ms 未完成，进度 {}",
            engine.snapshot().position_ms
        );
    }
    // 修复前任意一次定位都会 panic（release 下即闪退）。
    assert_eq!(engine.snapshot().state, State::Playing);
}
