//! 持久化层错误定义。

/// 持久化层统一错误枚举。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 打开或初始化数据库文件失败。
    #[error("打开数据库失败: {0}")]
    Database(#[from] redb::DatabaseError),
    /// 数据库已被另一个实例占用（GUI 与 CLI 共用同一数据目录）。
    #[error("数据目录已被另一个 lyra 实例占用（GUI 正在运行？）：{0}")]
    Busy(String),
    /// 开始读写事务失败（装箱以减少枚举内存占用）。
    #[error("事务失败: {0}")]
    Transaction(Box<redb::TransactionError>),
    /// 打开或操作表失败。
    #[error("表操作失败: {0}")]
    Table(#[from] redb::TableError),
    /// 存储后端读写数据页失败。
    #[error("存储后端失败: {0}")]
    Storage(#[from] redb::StorageError),
    /// 提交事务失败。
    #[error("提交事务失败: {0}")]
    Commit(#[from] redb::CommitError),
    /// `JSON` 序列化或反序列化失败。
    #[error("JSON 编解码失败: {0}")]
    Json(#[from] serde_json::Error),
    /// 文件系统 IO 失败。
    #[error("IO 失败: {0}")]
    Io(#[from] std::io::Error),
    /// 系统凭据存储失败（`keyring`）。
    #[error("凭据存储失败: {0}")]
    Secret(String),
}

/// 持久化层结果别名。
pub type Result<T> = std::result::Result<T, Error>;

impl From<redb::TransactionError> for Error {
    fn from(e: redb::TransactionError) -> Self {
        Self::Transaction(Box::new(e))
    }
}
