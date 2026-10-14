//! 引擎统一错误类型。

/// `lyra-audio` 的错误。所有失败都收敛到这里，向上以 `Result` 传播。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 系统没有默认音频输出设备。
    #[error("无可用音频输出设备")]
    NoOutputDevice,

    /// 读取输出设备默认配置失败。
    #[error("获取输出设备配置失败: {0}")]
    OutputConfig(String),

    /// 设备默认采样格式不受支持。
    #[error("不支持的输出采样格式: {0}")]
    UnsupportedSampleFormat(String),

    /// 创建输出流失败。
    #[error("创建音频输出流失败: {0}")]
    BuildStream(String),

    /// 启动输出流失败。
    #[error("启动音频输出流失败: {0}")]
    PlayStream(String),

    /// 启动解码线程失败。
    #[error("启动解码线程失败: {0}")]
    SpawnThread(String),

    /// 解码 / 解封装失败。
    #[error("解码失败: {0}")]
    Decode(String),

    /// 重采样失败。
    #[error("重采样失败: {0}")]
    Resample(String),
}
