//! 音频解码层：基于 Symphonia 将输入流解码为交织 f32 PCM。

use std::io::{ErrorKind as IoErrorKind, Read, Seek, SeekFrom};
use std::sync::Mutex;

use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder as SymphoniaDecoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::{Error, ReadSeek};

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

/// 音频流分包解码器
pub struct Decoder {
    format: Box<dyn FormatReader>,
    codec: Box<dyn SymphoniaDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
}

impl Decoder {
    /// 打开音频流并探测格式
    pub fn open(reader: Box<dyn ReadSeek + Send>) -> Result<Self, Error> {
        Self::open_with_hint(reader, None)
    }

    /// 打开音频流并结合扩展名探测格式
    pub fn open_with_hint(reader: Box<dyn ReadSeek + Send>, extension: Option<&str>) -> Result<Self, Error> {
        let source = SharedSource {
            inner: Mutex::new(reader),
        };
        let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());

        let mut hint = Hint::new();
        if let Some(ext) = extension {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
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
    pub fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, Error> {
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
                    if let Some(samples) = interleave(&audio) {
                        return Ok(Some(samples));
                    }
                    // 跳过空帧包
                }
                // 忽略单帧损坏错误，继续解码
                Err(SymphoniaError::DecodeError(_)) => {}
                Err(SymphoniaError::IoError(e)) if e.kind() == IoErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            }
        }
    }
}

/// 提取交织 f32 样本（空帧包返回 None）
fn interleave(audio: &AudioBufferRef<'_>) -> Option<Vec<f32>> {
    if audio.frames() == 0 {
        return None;
    }
    let mut buf = SampleBuffer::<f32>::new(audio.frames() as u64, *audio.spec());
    buf.copy_interleaved_ref(audio.clone());
    Some(buf.samples().to_vec())
}

/// 一次性解码整条音频流
pub fn decode_all(reader: Box<dyn ReadSeek + Send>) -> Result<Decoded, Error> {
    let mut decoder = Decoder::open(reader)?;
    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();
    let mut samples = Vec::new();
    while let Some(chunk) = decoder.next_chunk()? {
        samples.extend_from_slice(&chunk);
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
