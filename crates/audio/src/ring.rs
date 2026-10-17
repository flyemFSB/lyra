//! 环形缓冲区：容量计算、样本写入与实时回调消费。

use cpal::{FromSample, Sample};
use ringbuf::traits::{Consumer, Producer};

/// 计算环形缓冲区容量
pub(crate) fn ring_capacity(sample_rate: u32, channels: usize, ring_ms: u32) -> usize {
    let channels = channels.max(1);
    let ring_ms = ring_ms.clamp(20, 5_000);
    let frames = u64::from(sample_rate) * u64::from(ring_ms) / 1000;
    let samples = frames * channels as u64;
    usize::try_from(samples).unwrap_or(usize::MAX).max(channels)
}

/// 写入 PCM 样本到环形缓冲区，返回实际推入数量
pub(crate) fn push_slice(prod: &mut impl Producer<Item = f32>, samples: &[f32]) -> usize {
    prod.push_slice(samples)
}

/// 清空缓冲区待消费样本
pub(crate) fn drain(cons: &mut impl Consumer<Item = f32>) -> usize {
    cons.clear()
}

/// 从缓冲区提取样本并填充输出（应用音量缩放，欠载补静音）
pub(crate) fn fill_output<T>(cons: &mut impl Consumer<Item = f32>, volume: f32, out: &mut [T]) -> usize
where
    T: Sample + FromSample<f32>,
{
    let mut popped = 0;
    for slot in out.iter_mut() {
        match cons.try_pop() {
            Some(s) => {
                *slot = T::from_sample(s * volume);
                popped += 1;
            }
            // 欠载时填零静音
            None => *slot = T::from_sample(0.0),
        }
    }
    popped
}

/// 均值下混为单声道
fn downmix_to_mono(frame: &[f32]) -> f32 {
    frame.iter().sum::<f32>() / frame.len() as f32
}

/// 声道映射转换
pub(crate) fn map_channels(input: &[f32], src: usize, dst: usize) -> Vec<f32> {
    if src == dst || src == 0 {
        return input.to_vec();
    }
    let frames = input.len() / src;
    let mut out = Vec::with_capacity(frames * dst);
    for frame in 0..frames {
        let base = frame * src;
        if dst == 1 && src > 1 {
            out.push(downmix_to_mono(&input[base..base + src]));
            continue;
        }
        for ch in 0..dst {
            out.push(input[base + ch % src]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]
    use super::*;
    use ringbuf::{HeapRb, traits::Split};

    #[test]
    fn drain_empties_the_ring() {
        // 验证消费端清空环形缓冲区功能
        use ringbuf::traits::Observer as _;
        let rb = HeapRb::<f32>::new(8);
        let (mut prod, mut cons) = rb.split();
        assert_eq!(push_slice(&mut prod, &[1.0, 2.0, 3.0]), 3);
        assert_eq!(prod.occupied_len(), 3);
        assert_eq!(drain(&mut cons), 3);
        assert_eq!(prod.occupied_len(), 0);
        // 清空之后缓冲仍可正常使用
        assert_eq!(push_slice(&mut prod, &[4.0]), 1);
        let mut out = [0.0f32; 1];
        assert_eq!(fill_output(&mut cons, 1.0, &mut out), 1);
        assert_eq!(out, [4.0]);
    }

    #[test]
    fn ring_is_fifo() {
        let rb = HeapRb::<f32>::new(4);
        let (mut prod, mut cons) = rb.split();
        assert_eq!(push_slice(&mut prod, &[1.0, 2.0, 3.0]), 3);
        let mut out = [0.0f32; 3];
        assert_eq!(fill_output(&mut cons, 1.0, &mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn ring_reports_overflow_drop() {
        let rb = HeapRb::<f32>::new(3);
        let (mut prod, mut cons) = rb.split();
        // 容量为 3 时超出样本被丢弃，仅写入前 3 个。
        let pushed = push_slice(&mut prod, &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(pushed, 3);
        let mut out = [0.0f32; 3];
        assert_eq!(fill_output(&mut cons, 1.0, &mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        // 缓冲已空时返回 0 个有效样本。
        let mut empty = [0.0f32; 1];
        assert_eq!(fill_output(&mut cons, 1.0, &mut empty), 0);
        assert_eq!(empty, [0.0]);
    }

    #[test]
    fn fill_output_applies_volume_and_underrun() {
        let rb = HeapRb::<f32>::new(4);
        let (mut prod, mut cons) = rb.split();
        push_slice(&mut prod, &[0.5, -0.5]);
        let mut out = [0.0f32; 4];
        let popped = fill_output(&mut cons, 0.5, &mut out);
        assert_eq!(popped, 2);
        assert_eq!(out, [0.25, -0.25, 0.0, 0.0]);
    }

    #[test]
    fn capacity_covers_duration() {
        // 48kHz 立体声 200ms = 19200 样本。
        assert_eq!(ring_capacity(48_000, 2, 200), 19_200);
    }

    #[test]
    fn channel_mapping_mono_to_stereo_and_back() {
        assert_eq!(map_channels(&[1.0, 2.0], 1, 2), vec![1.0, 1.0, 2.0, 2.0]);
        // 立体声转单声道需均值混音，避免丢失右声道内容。
        assert_eq!(map_channels(&[1.0, 2.0, 3.0, 4.0], 2, 1), vec![1.5, 3.5]);
        assert_eq!(map_channels(&[1.0, 2.0], 2, 2), vec![1.0, 2.0]);
    }
}
