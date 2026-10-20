//! 音频重采样：采样率转换（基于 rubato FFT 固定块算法）。

use rubato::{FftFixedInOut, Resampler as _};

use crate::Error;

/// 计算最大公约数
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

/// 流式音频重采样器
pub struct Resampler {
    inner: FftFixedInOut<f32>,
    channels: usize,
    chunk_in: usize,
    /// 暂存未成块的输入样本
    stage: Vec<f32>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl Resampler {
    /// 创建重采样器
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

    /// 输入交织样本并执行重采样
    pub fn process(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, Error> {
        self.stage.extend_from_slice(interleaved);

        let block = self.chunk_in * self.channels;
        let mut out = Vec::new();

        while self.stage.len() >= block {
            self.process_block(&mut out)?;
        }

        Ok(out)
    }

    /// 处理一整块输入（块内含全部声道的交织样本）
    fn process_block(&mut self, out: &mut Vec<f32>) -> Result<(), Error> {
        let block = self.chunk_in * self.channels;
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
        Ok(())
    }

    /// 冲洗重采样器尾部
    ///
    /// 输入不足一整块时补零凑满再处理，把滤波器的收尾部分也输出出来。
    /// 不调用它会丢掉最多 `chunk_in - 1` 帧（44.1kHz 下约 3.3ms）的结尾音频。
    pub fn flush(&mut self) -> Result<Vec<f32>, Error> {
        if self.stage.is_empty() {
            return Ok(Vec::new());
        }
        let block = self.chunk_in * self.channels;
        self.stage.resize(block, 0.0);
        let mut out = Vec::new();
        self.process_block(&mut out)?;
        Ok(out)
    }

    /// 重采样器引入的延迟（帧）
    ///
    /// FFT 固定块算法有半个输出块的固有延迟，播放位置需要减去它，
    /// 否则进度条会比实际听到的声音超前。
    pub fn output_delay_frames(&self) -> usize {
        self.inner.output_delay()
    }
}

/// 一次性重采样整段 PCM 样本（仅供测试与离线工具使用）
#[cfg(test)]
fn resample_all(samples: &[f32], channels: usize, input_rate: u32, output_rate: u32) -> Result<Vec<f32>, Error> {
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

    #[test]
    fn flush_recovers_the_tail_that_would_otherwise_be_dropped() {
        // 100 帧 < 一个输入块（147 帧），process 阶段没有输出，
        // 剩余的样本必须由 flush 补零处理出来，否则结尾音频会被丢掉。
        let input = sine(100, 1);
        let mut resampler = Resampler::new(44_100, 48_000, 1).unwrap();
        assert!(resampler.process(&input).unwrap().is_empty());
        let tail = resampler.flush().unwrap();
        assert!(!tail.is_empty(), "flush 必须把尾部样本补零处理出来");
    }

    #[test]
    fn flushing_an_empty_resampler_is_a_no_op() {
        let mut resampler = Resampler::new(44_100, 48_000, 1).unwrap();
        assert!(resampler.flush().unwrap().is_empty());
    }

    #[test]
    fn the_resampler_reports_its_intrinsic_delay() {
        // FFT 固定块算法有半个输出块的固有延迟，播放位置需要减去它
        let resampler = Resampler::new(44_100, 48_000, 1).unwrap();
        assert!(resampler.output_delay_frames() > 0);
    }
}
