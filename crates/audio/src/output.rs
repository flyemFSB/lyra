//! 音频输出层：基于 cpal 构建实时音频流，在回调中执行无锁出队与格式转换。

use std::sync::Arc;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use ringbuf::HeapCons;

use crate::Error;
use crate::ring;
use crate::state::Shared;

/// 从 `Option` 取出环形缓冲消费者。
fn take_cons(cons: &mut Option<HeapCons<f32>>) -> Result<HeapCons<f32>, Error> {
    cons.take()
        .ok_or_else(|| Error::BuildStream("环形缓冲消费者已被占用".into()))
}

/// 按设备采样格式构建一个 `T` 类型的输出流。
fn build_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    shared: Arc<Shared>,
    mut cons: HeapCons<f32>,
) -> Result<Stream, Error>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    let err_fn = |e| tracing::error!("音频输出流错误: {e}");
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _info| {
                // 处理重定位刷新信号，清空环形缓冲残留音频
                if shared.take_flush_request() {
                    ring::drain(&mut cons);
                }
                if shared.is_playing() {
                    ring::fill_output(&mut cons, shared.volume(), data);
                } else {
                    // 暂停或停止状态写入静音样本，不消耗缓冲队列数据
                    for slot in data.iter_mut() {
                        *slot = T::from_sample(0.0);
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| Error::BuildStream(e.to_string()))
}

/// 根据设备采样格式分派具体样本类型并构建输出流。
pub(crate) fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    shared: &Arc<Shared>,
    cons: HeapCons<f32>,
) -> Result<Stream, Error> {
    let mut cons = Some(cons);
    macro_rules! arm {
        ($t:ty) => {
            build_typed::<$t>(device, config, shared.clone(), take_cons(&mut cons)?)
        };
    }

    match format {
        SampleFormat::F32 => arm!(f32),
        SampleFormat::F64 => arm!(f64),
        SampleFormat::I8 => arm!(i8),
        SampleFormat::I16 => arm!(i16),
        SampleFormat::I32 => arm!(i32),
        SampleFormat::U8 => arm!(u8),
        SampleFormat::U16 => arm!(u16),
        SampleFormat::U32 => arm!(u32),
        other => Err(Error::UnsupportedSampleFormat(other.to_string())),
    }
}

/// 启动音频输出流。
pub(crate) fn start(stream: &Stream) -> Result<(), Error> {
    stream.play().map_err(|e| Error::PlayStream(e.to_string()))
}
