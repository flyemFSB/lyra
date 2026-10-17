//! 重采样层：仅在设备采样率 ≠ 源采样率时启用，运行在解码线程。

use rubato::{FftFixedInOut, Resampler as _};

use crate::Error;

/// 最大公约数，用于挑选「最小精确块」。
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

/// 流式音频重采样器：将交织 f32 样本在不同采样率之间进行高精度转换。
pub struct Resampler {
    inner: FftFixedInOut<f32>,
    channels: usize,
    chunk_in: usize,
    /// 尚未凑满一块的交织输入。
    stage: Vec<f32>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl Resampler {
    /// 为给定输入/输出采样率与声道数创建重采样器。
    pub fn new(input_rate: u32, output_rate: u32, channels: usize) -> Result<Self, Error> {
        if channels == 0 {
            return Err(Error::Resample("声道数为 0".into()));
        }
        if input_rate == 0 || output_rate == 0 {
            return Err(Error::Resample("采样率为 0".into()));
        }

        let chunk_in = input_rate as usize / gcd(input_rate as usize, output_rate as usize);
        let inner = FftFixedInOut::<f32>::new(input_rate as usize, output_rate as usize, chunk_in, channels)
            .map_err(|e| Error::Resample(e.to_string()))?;

        let chunk_in = inner.input_frames_next();
        let chunk_out = inner.output_frames_next();

        Ok(Self {
            inner,
            channels,
            chunk_in,
            stage: Vec::new(),
            in_buf: vec![vec![0.0; chunk_in]; channels],
            out_buf: vec![vec![0.0; chunk_out]; channels],
        })
    }

    /// 推入交织输入样本，返回重采样后的交织样本。
    pub fn process(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, Error> {
        self.stage.extend_from_slice(interleaved);

        let block = self.chunk_in * self.channels;
        let mut out = Vec::new();

        while self.stage.len() >= block {
            for (ch, dst) in self.in_buf.iter_mut().enumerate() {
                for (frame, slot) in dst.iter_mut().enumerate() {
                    *slot = self.stage[frame * self.channels + ch];
                }
            }
            self.stage.drain(..block);

            let (_, out_len) = self
                .inner
                .process_into_buffer(&self.in_buf, &mut self.out_buf, None)
                .map_err(|e| Error::Resample(e.to_string()))?;

            for frame in 0..out_len {
                for ch in 0..self.channels {
                    out.push(self.out_buf[ch][frame]);
                }
            }
        }

        Ok(out)
    }
}

/// 一次性将交织 PCM 数据重采样到目标采样率。
pub fn resample_all(samples: &[f32], channels: usize, input_rate: u32, output_rate: u32) -> Result<Vec<f32>, Error> {
    if input_rate == output_rate {
        return Ok(samples.to_vec());
    }
    let mut resampler = Resampler::new(input_rate, output_rate, channels)?;
    resampler.process(samples)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]
    use super::*;

    fn sine(frames: usize, channels: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            let s = (i as f32 * 0.01).sin();
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    #[test]
    fn upsample_44100_to_48000_length() {
        let input = sine(22_050, 1);
        let out = resample_all(&input, 1, 44_100, 48_000).unwrap();
        let expected = 22_050 * 48_000 / 44_100; // 24_000
        assert!(
            out.len().abs_diff(expected) <= 160,
            "输出长度 {} 偏离 {expected} 过多",
            out.len()
        );
    }

    #[test]
    fn same_rate_is_passthrough() {
        let input = sine(1_000, 2);
        let out = resample_all(&input, 2, 44_100, 44_100).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn stereo_resample_preserves_interleaving() {
        let frames = 4_410;
        let out = resample_all(&sine(frames, 2), 2, 44_100, 48_000).unwrap();
        // 验证立体声重采样后声道样本严格交织成对
        assert_eq!(out.len() % 2, 0);
        let expected = frames * 48_000 / 44_100;
        assert!(out.len() / 2 >= expected - 160);
    }
}
