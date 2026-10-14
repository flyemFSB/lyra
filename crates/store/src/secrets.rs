//! 敏感凭据存储抽象（支持系统密钥环与内存存储）。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{Error, Result};

/// 系统密钥环服务名。
const KEYRING_SERVICE: &str = "lyra.kugou";

/// 键值式凭据存储：`user` 为键，`secret` 为值。
pub trait Secrets: Send + Sync + 'static {
    /// 写入（或覆盖）`user` 的凭据。
    ///
    /// # Errors
    /// 底层凭据存储不可用时返回 [`Error::Secret`]。
    fn set(&self, user: &str, secret: &str) -> Result<()>;

    /// 读取 `user` 的凭据；不存在时返回 `None`。
    ///
    /// # Errors
    /// 底层凭据存储不可用时返回 [`Error::Secret`]。
    fn get(&self, user: &str) -> Result<Option<String>>;

    /// 删除 `user` 的凭据；本就不存在时视为成功。
    ///
    /// # Errors
    /// 底层凭据存储不可用时返回 [`Error::Secret`]。
    fn delete(&self, user: &str) -> Result<()>;
}

/// 生产实现：Windows 凭据管理器。
pub struct KeyringSecrets;

impl Secrets for KeyringSecrets {
    fn set(&self, user: &str, secret: &str) -> Result<()> {
        entry(user)?.set_password(secret).map_err(|e| secret_err(&e))
    }

    fn get(&self, user: &str) -> Result<Option<String>> {
        match entry(user)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(secret_err(&e)),
        }
    }

    fn delete(&self, user: &str) -> Result<()> {
        match entry(user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(secret_err(&e)),
        }
    }
}

/// 打开一个凭据条目。
fn entry(user: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, user).map_err(|e| secret_err(&e))
}

/// 凭据错误统一转换为 Error::Secret（防止令牌内容流入错误日志）。
fn secret_err(e: &keyring::Error) -> Error {
    Error::Secret(e.to_string())
}

/// 测试与无头环境使用的进程内内存凭据存储。
#[derive(Default)]
pub struct MemorySecrets {
    map: Mutex<HashMap<String, String>>,
}

impl MemorySecrets {
    /// 新建一个空的进程内凭据库。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Secrets for MemorySecrets {
    fn set(&self, user: &str, secret: &str) -> Result<()> {
        self.map
            .lock()
            .map_err(|_| poisoned())?
            .insert(user.to_owned(), secret.to_owned());
        Ok(())
    }

    fn get(&self, user: &str) -> Result<Option<String>> {
        Ok(self.map.lock().map_err(|_| poisoned())?.get(user).cloned())
    }

    fn delete(&self, user: &str) -> Result<()> {
        self.map.lock().map_err(|_| poisoned())?.remove(user);
        Ok(())
    }
}

/// 互斥锁中毒异常转换。
fn poisoned() -> Error {
    Error::Secret("内存凭据锁已中毒".to_owned())
}
