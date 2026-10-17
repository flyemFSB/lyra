//! 敏感凭据存储抽象（支持系统密钥环与内存存储）。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{Error, Result};

/// 系统密钥环服务名
const KEYRING_SERVICE: &str = "lyra.kugou";

/// 敏感凭据存取接口
pub trait Secrets: Send + Sync + 'static {
    /// 写入或更新凭据
    fn set(&self, user: &str, secret: &str) -> Result<()>;

    /// 获取凭据（不存在返回 None）
    fn get(&self, user: &str) -> Result<Option<String>>;

    /// 删除凭据
    fn delete(&self, user: &str) -> Result<()>;
}

/// 系统密钥环存储实现（Windows 凭据管理器）
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

/// 获取指定用户的凭据条目
fn entry(user: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, user).map_err(|e| secret_err(&e))
}

/// 转换凭据错误，防止敏感信息泄漏到日志
fn secret_err(e: &keyring::Error) -> Error {
    Error::Secret(e.to_string())
}

/// 内存凭据存储实现（用于测试）
#[derive(Default)]
pub struct MemorySecrets {
    map: Mutex<HashMap<String, String>>,
}

impl MemorySecrets {
    /// 创建内存凭据存储
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

/// 互斥锁中毒错误转换
fn poisoned() -> Error {
    Error::Secret("内存凭据锁已中毒".to_owned())
}
