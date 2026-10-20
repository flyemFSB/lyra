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
    // 实时线程禁止调用 tracing：格式化会分配内存，抢全局订阅锁会造成优先级反转。
    // 这里只把错误文本放进共享槽位，由解码线程（非实时）负责打日志。
    // 错误回调与数据回调各自持有一份 Arc，互不抢占所有权。
    let err_shared = shared.clone();
    let err_fn = move |e: cpal::StreamError| err_shared.record_stream_error(e.to_string());
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
        // cpal 为这些格式实现了 SizedSample，必须一并支持，
        // 否则遇到 I24 / i64 / u64 设备的整个引擎会拒绝启动。
        SampleFormat::I24 => arm!(cpal::I24),
        SampleFormat::I64 => arm!(i64),
        SampleFormat::U64 => arm!(u64),
        other => Err(Error::UnsupportedSampleFormat(other.to_string())),
    }
}

/// 启动音频输出流
pub(crate) fn start(stream: &Stream) -> Result<(), Error> {
    stream.play().map_err(|e| Error::PlayStream(e.to_string()))
}
