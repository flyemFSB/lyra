//! 持久化层错误定义。

/// 持久化层错误枚举
///
/// 标记为 `#[non_exhaustive]`：新增变体对下游是向后兼容的破坏性变更。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// 打开或初始化数据库失败
    #[error("打开数据库失败: {0}")]
    Database(#[from] redb::DatabaseError),
    /// 数据库已被其他实例占用
    #[error("数据目录已被另一个 lyra 实例占用（GUI 正在运行？）：{0}")]
    Busy(String),
    /// 事务执行失败（保留底层错误链，`Error::source()` 可追到 redb 原因）
    #[error("事务失败: {0}")]
    Transaction(#[from] Box<redb::TransactionError>),
    /// 表操作失败
    #[error("表操作失败: {0}")]
    Table(#[from] redb::TableError),
    /// 存储后端错误
    #[error("存储后端失败: {0}")]
    Storage(#[from] redb::StorageError),
    /// 事务提交失败
    #[error("提交事务失败: {0}")]
    Commit(#[from] redb::CommitError),
    /// JSON 序列化或反序列化失败
    #[error("JSON 编解码失败: {0}")]
    Json(#[from] serde_json::Error),
    /// 文件系统 I/O 错误
    #[error("IO 失败: {0}")]
    Io(#[from] std::io::Error),
    /// 系统凭据存储失败
    #[error("凭据存储失败: {0}")]
    Secret(#[source] keyring::Error),
    /// 凭据存储内部状态异常（如内存实现的锁中毒）
    #[error("凭据存储状态异常: {0}")]
    SecretState(String),
    /// 数据库 schema 版本高于当前程序支持的版本
    #[error("数据库 schema 版本 {found} 高于当前程序支持的 {supported}")]
    SchemaVersion {
        /// 数据库中记录的版本
        found: u64,
        /// 当前程序支持的版本
        supported: u64,
    },
    /// 调用方传入的参数非法
    #[error("参数非法: {0}")]
    Invalid(String),
}

/// 持久化层 Result 别名
pub type Result<T, E = Error> = core::result::Result<T, E>;
