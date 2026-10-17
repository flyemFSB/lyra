//! 音频输出层：基于 cpal 构建输出流并在实时回调中出队。

use std::sync::Arc;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use ringbuf::HeapCons;

use crate::Error;
use crate::ring;
use crate::state::Shared;

/// 取出消费者句柄
fn take_cons(cons: &mut Option<HeapCons<f32>>) -> Result<HeapCons<f32>, Error> {
    cons.take()
        .ok_or_else(|| Error::BuildStream("环形缓冲消费者已被占用".into()))
}

/// 构建强类型输出流
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
                // 响应刷新请求，清空残留音频
                if shared.take_flush_request() {
                    ring::drain(&mut cons);
                }
                if shared.is_playing() {
                    ring::fill_output(&mut cons, shared.volume(), data);
                } else {
                    // 非播放状态填充静音，不消耗队列
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

/// 按采样格式构建输出流
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

/// 启动音频输出流
pub(crate) fn start(stream: &Stream) -> Result<(), Error> {
    stream.play().map_err(|e| Error::PlayStream(e.to_string()))
}
