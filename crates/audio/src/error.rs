//! 音频引擎统一错误定义。

/// 音频引擎错误枚举
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 无可用音频输出设备
    #[error("无可用音频输出设备")]
    NoOutputDevice,

    /// 获取输出设备配置失败
    #[error("获取输出设备配置失败: {0}")]
    OutputConfig(String),

    /// 输出采样格式不受支持
    #[error("不支持的输出采样格式: {0}")]
    UnsupportedSampleFormat(String),

    /// 创建音频输出流失败
    #[error("创建音频输出流失败: {0}")]
    BuildStream(String),

    /// 启动音频输出流失败
    #[error("启动音频输出流失败: {0}")]
    PlayStream(String),

    /// 启动解码线程失败
    #[error("启动解码线程失败: {0}")]
    SpawnThread(String),

    /// 音频解封装或解码失败
    #[error("解码失败: {0}")]
    Decode(String),

    /// 音频重采样失败
    #[error("重采样失败: {0}")]
    Resample(String),
}
