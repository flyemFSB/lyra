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
        entry(user)?.set_password(secret).map_err(Error::Secret)
    }

    fn get(&self, user: &str) -> Result<Option<String>> {
        match entry(user)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Secret(e)),
        }
    }

    fn delete(&self, user: &str) -> Result<()> {
        match entry(user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Secret(e)),
        }
    }
}

/// 获取指定用户的凭据条目
fn entry(user: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, user).map_err(Error::Secret)
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
    Error::SecretState("内存凭据锁已中毒".to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// 真实密钥环后端的跨实例持久化测试。
    ///
    /// 第三轮修复记录过：`keyring` 3.x 不显式开 `windows-native` 时会在 Windows 上
    /// 静默退化成进程内 mock（跨进程即丢）。本测试直接打到真实的 Windows 凭据管理器，
    /// 因此默认不随 `cargo test` 执行，需要时显式跑：
    ///
    /// ```text
    /// cargo test -p lyra-store -- --ignored
    /// ```
    ///
    /// 它会在凭据管理器里创建 `lyra.kugou` / `lyra-store-keyring-test` 条目并在结束时清理。
    #[test]
    #[ignore = "需要真实的 Windows 凭据管理器；用 cargo test -p lyra-store -- --ignored 显式执行"]
    fn keyring_backend_persists_across_instances() {
        let user = "lyra-store-keyring-test";
        let secrets = KeyringSecrets;
        // 清理可能残留的旧条目
        secrets.delete(user).unwrap();

        secrets.set(user, "CROSS-INSTANCE-SECRET").unwrap();
        // 换一个实例（新的 KeyringSecrets）读取，验证不是进程内 mock
        let other = KeyringSecrets;
        assert_eq!(
            other.get(user).unwrap().as_deref(),
            Some("CROSS-INSTANCE-SECRET"),
            "凭据必须跨实例可见（真实 Windows 凭据管理器）"
        );
        other.delete(user).unwrap();
        assert!(other.get(user).unwrap().is_none());
    }

    #[test]
    fn memory_backend_round_trips() {
        let secrets = MemorySecrets::new();
        assert!(secrets.get("u").unwrap().is_none());
        secrets.set("u", "s").unwrap();
        assert_eq!(secrets.get("u").unwrap().as_deref(), Some("s"));
        secrets.delete("u").unwrap();
        assert!(secrets.get("u").unwrap().is_none());
    }
}
