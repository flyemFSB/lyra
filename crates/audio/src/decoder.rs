//! 音频解码层：基于 Symphonia 将输入流解码为交织 f32 PCM。

use std::io::{ErrorKind as IoErrorKind, Read, Seek, SeekFrom};
use std::sync::Mutex;

use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder as SymphoniaDecoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::{Error, ReadSeek};

/// 连续解码错误的容忍上限
///
/// 单帧损坏是常见的（网络抖动、文件轻微截断），跳过即可；
/// 但如果**每一帧**都失败，继续"跳过"就会一路走到 EOF，
/// 引擎会误判为"正常播完"并切到下一首。这里给一个上限把它变成显式失败。
const MAX_DECODE_ERRORS: u32 = 32;

/// 解码后的 PCM 音频数据
#[derive(Debug, Clone)]
pub struct Decoded {
    /// 采样率（Hz）
    pub sample_rate: u32,
    /// 声道数
    pub channels: usize,
    /// 交织 PCM 样本
    pub samples: Vec<f32>,
}

impl Decoded {
    /// 音频帧数（样本数 / 声道数）
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }

    /// 音频时长（毫秒）
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            0
        } else {
            self.frames() as u64 * 1000 / u64::from(self.sample_rate)
        }
    }
}

/// 适配 ReadSeek 为 Symphonia MediaSource
struct SharedSource {
    inner: Mutex<Box<dyn ReadSeek + Send>>,
}

impl Read for SharedSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // 获取内部可变引用进行读取
        match self.inner.get_mut() {
            Ok(s) => s.read(buf),
            Err(e) => e.into_inner().read(buf),
        }
    }
}

impl Seek for SharedSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self.inner.get_mut() {
            Ok(s) => s.seek(pos),
            Err(e) => e.into_inner().seek(pos),
        }
    }
}

impl MediaSource for SharedSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// 解码出的一个音频块
#[derive(Debug, Clone)]
pub struct DecodedChunk {
    /// 交织 PCM 样本
    pub samples: Vec<f32>,
    /// 实际声道数（以解码结果为准，不信任容器的声明）
    pub channels: usize,
    /// 实际采样率（以解码结果为准）
    pub sample_rate: u32,
}

/// 音频流分包解码器
pub struct Decoder {
    format: Box<dyn FormatReader>,
    codec: Box<dyn SymphoniaDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    /// 连续解码错误计数
    decode_errors: u32,
}

impl Decoder {
    /// 打开音频流并探测格式
    ///
    /// symphonia 0.5 的格式探测完全基于内容（`Probe::format` 会忽略 `Hint`），
    /// 因此这里不接收扩展名参数 —— 传了也没有任何作用。
    pub fn open(reader: Box<dyn ReadSeek + Send>) -> Result<Self, Error> {
        let source = SharedSource {
            inner: Mutex::new(reader),
        };
        let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());

        let probed = symphonia::default::get_probe()
            .format(
                &symphonia::core::probe::Hint::new(),
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| Error::Decode(e.to_string()))?;

        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| Error::Decode("未找到可解码音轨".into()))?;

        let track_id = track.id;
        let params = track.codec_params.clone();
        let sample_rate = params.sample_rate.ok_or_else(|| Error::Decode("未知采样率".into()))?;
        let channels = params
            .channels
            .map(symphonia::core::audio::Channels::count)
            .ok_or_else(|| Error::Decode("未知声道数".into()))?;
        if channels == 0 {
            return Err(Error::Decode("声道数为 0".into()));
        }

        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(|e| Error::Decode(e.to_string()))?;

        Ok(Self {
            format,
            codec: decoder,
            track_id,
            sample_rate,
            channels,
            decode_errors: 0,
        })
    }

    /// 获取采样率（Hz）
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 获取声道数
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// 解码下一个音频数据块（到达流末尾返回 Ok(None)）
    pub fn next_chunk(&mut self) -> Result<Option<DecodedChunk>, Error> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(e)) if e.kind() == IoErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.codec.reset();
                    continue;
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.codec.decode(&packet) {
                Ok(audio) => {
                    if let Some((samples, channels, sample_rate)) = interleave(&audio) {
                        self.decode_errors = 0;
                        // 以实际解码结果为准：个别容器会少报声道数，
                        // 沿用声明值会让声道映射错乱（音频失真）。
                        self.channels = channels;
                        self.sample_rate = sample_rate;
                        return Ok(Some(DecodedChunk {
                            samples,
                            channels,
                            sample_rate,
                        }));
                    }
                    // 跳过空帧包
                }
                // 单帧损坏可以跳过，但连续失败必须报错，
                // 否则整条音轨损坏时会一路"跳过"到 EOF，被误判成正常播完。
                Err(SymphoniaError::DecodeError(_)) => {
                    self.decode_errors += 1;
                    if self.decode_errors >= MAX_DECODE_ERRORS {
                        return Err(Error::Decode(format!(
                            "连续 {MAX_DECODE_ERRORS} 帧解码失败，音频可能已损坏"
                        )));
                    }
                }
                Err(SymphoniaError::IoError(e)) if e.kind() == IoErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            }
        }
    }
}

/// 提取交织 f32 样本与实际规格（空帧包返回 None）
fn interleave(audio: &AudioBufferRef<'_>) -> Option<(Vec<f32>, usize, u32)> {
    if audio.frames() == 0 {
        return None;
    }
    let spec = audio.spec();
    let channels = spec.channels.count();
    let sample_rate = spec.rate;
    let mut buf = SampleBuffer::<f32>::new(audio.frames() as u64, *spec);
    buf.copy_interleaved_ref(audio.clone());
    Some((buf.samples().to_vec(), channels, sample_rate))
}

/// 一次性解码整条音频流
pub fn decode_all(reader: Box<dyn ReadSeek + Send>) -> Result<Decoded, Error> {
    let mut decoder = Decoder::open(reader)?;
    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();
    let mut samples = Vec::new();
    while let Some(chunk) = decoder.next_chunk()? {
        samples.extend_from_slice(&chunk.samples);
    }
    Ok(Decoded {
        sample_rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]
    use super::*;
    use std::f32::consts::TAU;

    /// 生成测试用正弦波 WAV 流
    pub(crate) fn sine_wav(seconds: f32) -> Box<dyn ReadSeek + Send> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut file = tempfile::tempfile().unwrap();
        {
            let mut writer = hound::WavWriter::new(&mut file, spec).unwrap();
            let total = (44_100.0 * seconds) as u32;
            for i in 0..total {
                let t = i as f32 / 44_100.0;
                let v = (TAU * 440.0 * t).sin() * 0.5;
                writer.write_sample((v * f32::from(i16::MAX)) as i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        file.seek(SeekFrom::Start(0)).unwrap();
        Box::new(file)
    }

    #[test]
    fn zero_frame_packets_are_skipped_instead_of_panicking() {
        // 跳过 0 帧包，防止切片越界恐慌
        use std::borrow::Cow;
        use symphonia::core::audio::{AudioBuffer, Channels, Signal, SignalSpec};

        let spec = SignalSpec::new(44_100, Channels::FRONT_LEFT | Channels::FRONT_RIGHT);
        let empty = AudioBuffer::<f32>::new(0, spec);
        assert_eq!(empty.frames(), 0);
        assert!(interleave(&AudioBufferRef::F32(Cow::Borrowed(&empty))).is_none());
    }

    #[test]
    fn decode_reports_format_and_length() {
        let decoded = decode_all(sine_wav(0.5)).unwrap();
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.frames(), 22_050);
        assert_eq!(decoded.duration_ms(), 500);
    }
}
