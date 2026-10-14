#![forbid(unsafe_code)]
//! 数据持久化层：基于 redb 管理设备、登录态、配置、缓存与历史，敏感令牌隔离存入密钥环。

mod error;
mod secrets;
mod types;

pub use error::{Error, Result};
pub use secrets::{KeyringSecrets, MemorySecrets, Secrets};
pub use types::{Account, HistoryEntry, Settings};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redb::{ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;

/// 单例键值表（设备、账户及配置信息）。
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
/// 命名空间二级缓存表：(ns, key) -> (过期毫秒, 二进制数据)。
const BLOBS: TableDefinition<(&str, &str), (u64, &[u8])> = TableDefinition::new("blobs");
/// 播放历史表：hash -> HistoryEntry JSON 数据。
const HISTORY: TableDefinition<&str, &[u8]> = TableDefinition::new("history");

/// 临时文件去重自增序号。
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 持久化存储实例。
pub struct Store {
    db: redb::Database,
    secrets: Box<dyn Secrets>,
    /// 临时文件守卫（释放文件句柄后自动清理）。
    _temp: Option<TempFile>,
}

/// 临时库文件守卫，`Drop` 时删除。
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

/// 打开（不存在则创建）位于 `path` 的存储，凭据走系统密钥环。
///
/// # Errors
/// 文件不可用或不是合法 `redb` 库时返回 [`Error::Database`]。
pub fn open(path: impl AsRef<Path>) -> Result<Store> {
    open_with_secrets(path, Box::new(KeyringSecrets))
}

/// 同 [`open`]，但允许注入凭据实现（测试 / 无头 CLI）。
///
/// # Errors
/// 同 [`open`]。
pub fn open_with_secrets(path: impl AsRef<Path>, secrets: Box<dyn Secrets>) -> Result<Store> {
    let db = redb::Database::create(path)?;
    Ok(Store {
        db,
        secrets,
        _temp: None,
    })
}

/// 打开一个落在系统临时目录下的一次性存储，`Drop` 时删除文件。
///
/// # Errors
/// 临时目录不可写时返回 [`Error::Io`]。
pub fn open_temp() -> Result<Store> {
    open_temp_with_secrets(Box::new(KeyringSecrets))
}

/// 同 [`open_temp`]，但允许注入凭据实现。
///
/// # Errors
/// 同 [`open_temp`]。
pub fn open_temp_with_secrets(secrets: Box<dyn Secrets>) -> Result<Store> {
    let path = unique_temp_path()?;
    match redb::Database::create(&path) {
        Ok(db) => Ok(Store {
            db,
            secrets,
            _temp: Some(TempFile(path)),
        }),
        Err(e) => {
            std::fs::remove_file(&path).ok();
            Err(e.into())
        }
    }
}

impl Store {
    /// 读取设备身份；未注册时返回 `None`。
    ///
    /// # Errors
    /// 数据库或 `JSON` 失败时返回错误。
    pub fn device(&self) -> Result<Option<lyra_proto::Device>> {
        decode(self.get_meta("device")?)
    }

    /// 持久化设备身份（覆盖写入）。
    ///
    /// # Errors
    /// 数据库或 `JSON` 失败时返回错误。
    pub fn set_device(&self, device: &lyra_proto::Device) -> Result<()> {
        self.put_meta("device", &serde_json::to_vec(device)?)
    }

    /// 读取登录态；token 与 vip_token 从系统凭据管理器回填。
    ///
    /// # Errors
    /// 数据库、`JSON` 或凭据存储失败时返回错误。
    pub fn account(&self) -> Result<Option<Account>> {
        let Some(mut account) = decode::<Account>(self.get_meta("account")?)? else {
            return Ok(None);
        };
        if let Some(token) = self.secrets.get(&account.userid.to_string())? {
            account.token = token;
        }
        // VIP 凭据隔离存储于系统密钥环，不落明文
        if let Some(vip) = self.secrets.get(&vip_secret_user(account.userid))? {
            account.vip_token = vip;
        }
        Ok(Some(account))
    }

    /// 写入登录态；token 与 vip_token 只进凭据管理器，`redb` 中的副本永远不含它们。
    ///
    /// # Errors
    /// 数据库、`JSON` 或凭据存储失败时返回错误。
    pub fn set_account(&self, account: &Account) -> Result<()> {
        let user = account.userid.to_string();
        if account.token.is_empty() {
            // 空令牌视为登出，清理持久化凭据
            self.secrets.delete(&user)?;
        } else {
            self.secrets.set(&user, &account.token)?;
        }
        let vip_user = vip_secret_user(account.userid);
        if account.vip_token.is_empty() {
            self.secrets.delete(&vip_user)?;
        } else {
            self.secrets.set(&vip_user, &account.vip_token)?;
        }
        let mut stored = account.clone();
        stored.token = String::new();
        stored.vip_token = String::new();
        self.put_meta("account", &serde_json::to_vec(&stored)?)
    }

    /// 登出：删除凭据管理器中的两个凭据与 `redb` 中的登录态。
    ///
    /// # Errors
    /// 数据库或凭据存储失败时返回错误。
    pub fn clear_account(&self) -> Result<()> {
        if let Some(account) = decode::<Account>(self.get_meta("account")?)? {
            self.secrets.delete(&account.userid.to_string())?;
            self.secrets.delete(&vip_secret_user(account.userid))?;
        }
        self.remove_meta("account")
    }

    /// 读取设置；从未设置过时返回 [`Settings::default`]。
    ///
    /// # Errors
    /// 数据库或 `JSON` 失败时返回错误。
    pub fn settings(&self) -> Result<Settings> {
        Ok(decode::<Settings>(self.get_meta("settings")?)?.unwrap_or_default())
    }

    /// 覆盖写入设置。
    ///
    /// # Errors
    /// 数据库或 `JSON` 失败时返回错误。
    pub fn set_settings(&self, s: &Settings) -> Result<()> {
        self.put_meta("settings", &serde_json::to_vec(s)?)
    }

    /// 写入一个缓存块；`ttl` 为 `None` 或 `Some(Duration::ZERO)` 时立即过期。
    ///
    /// # Errors
    /// 数据库失败时返回错误。
    pub fn put_blob(&self, ns: &str, key: &str, bytes: &[u8], ttl: Option<Duration>) -> Result<()> {
        let expires = ttl.map_or(0, |ttl| now_ms().saturating_add(millis(ttl)));
        self.write(|txn| {
            txn.open_table(BLOBS)?.insert((ns, key), (expires, bytes))?;
            Ok(())
        })
    }

    /// 读取一个缓存块；未知键或已过期都返回 `None`。
    ///
    /// # Errors
    /// 数据库失败时返回错误。
    pub fn get_blob(&self, ns: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(BLOBS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let Some(guard) = table.get((ns, key))? else {
            return Ok(None);
        };
        let (expires, bytes) = guard.value();
        if is_expired(expires, now_ms()) {
            return Ok(None);
        }
        Ok(Some(bytes.to_vec()))
    }

    /// 删除一个缓存块；不存在也算成功。
    ///
    /// # Errors
    /// 数据库失败时返回错误。
    pub fn remove_blob(&self, ns: &str, key: &str) -> Result<()> {
        self.write(|txn| {
            txn.open_table(BLOBS)?.remove((ns, key))?;
            Ok(())
        })
    }

    /// 清理所有已过期的缓存块，返回删除条数。
    ///
    /// # Errors
    /// 数据库失败时返回错误。
    pub fn drop_expired(&self) -> Result<usize> {
        let now = now_ms();
        self.write(|txn| {
            let mut table = txn.open_table(BLOBS)?;
            let mut stale: Vec<(String, String)> = Vec::new();
            for row in table.iter()? {
                let (k, v) = row?;
                let (ns, key) = k.value();
                let (expires, _) = v.value();
                if is_expired(expires, now) {
                    stale.push((ns.to_owned(), key.to_owned()));
                }
            }
            let mut removed = 0;
            for (ns, key) in stale {
                if table.remove((ns.as_str(), key.as_str()))?.is_some() {
                    removed += 1;
                }
            }
            Ok(removed)
        })
    }

    /// 清空一个命名空间，返回删除条数；其它命名空间不受影响。
    ///
    /// # Errors
    /// 数据库失败时返回错误。
    pub fn clear_ns(&self, ns: &str) -> Result<usize> {
        self.write(|txn| {
            let mut table = txn.open_table(BLOBS)?;
            let mut keys: Vec<String> = Vec::new();
            for row in table.iter()? {
                let (k, _) = row?;
                let (row_ns, key) = k.value();
                if row_ns == ns {
                    keys.push(key.to_owned());
                }
            }
            let mut removed = 0;
            for key in keys {
                if table.remove((ns, key.as_str()))?.is_some() {
                    removed += 1;
                }
            }
            Ok(removed)
        })
    }

    /// 追加一条播放历史；同一 `hash` 只保留最新一条（覆盖旧记录）。
    ///
    /// # Errors
    /// 数据库或 `JSON` 失败时返回错误。
    pub fn push_history(&self, entry: &HistoryEntry) -> Result<()> {
        let bytes = serde_json::to_vec(entry)?;
        self.write(|txn| {
            txn.open_table(HISTORY)?.insert(entry.hash.as_str(), bytes.as_slice())?;
            Ok(())
        })
    }

    /// 读取最近 `limit` 条播放历史，最近优先。
    ///
    /// # Errors
    /// 数据库或 `JSON` 失败时返回错误。
    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(HISTORY) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut entries: Vec<HistoryEntry> = Vec::new();
        for row in table.iter()? {
            let (_, value) = row?;
            entries.push(serde_json::from_slice(value.value())?);
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.played_at_ms));
        entries.truncate(limit);
        Ok(entries)
    }

    /// 清空全部播放历史。
    ///
    /// # Errors
    /// 数据库失败时返回错误。
    pub fn clear_history(&self) -> Result<()> {
        self.write(|txn| {
            txn.delete_table(HISTORY)?;
            Ok(())
        })
    }

    /// 统一写事务辅助方法（执行闭包并在成功后自动提交）。
    fn write<F, T>(&self, body: F) -> Result<T>
    where
        F: FnOnce(&redb::WriteTransaction) -> Result<T>,
    {
        let txn = self.db.begin_write()?;
        let out = body(&txn)?;
        txn.commit()?;
        Ok(out)
    }

    /// 读取一个单例键的原始 `JSON` 字节。
    fn get_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(META) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(table.get(key)?.map(|guard| guard.value().to_vec()))
    }

    /// 写入/覆盖一个单例键。
    fn put_meta(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.write(|txn| {
            txn.open_table(META)?.insert(key, bytes)?;
            Ok(())
        })
    }

    /// 删除一个单例键；不存在也算成功。
    fn remove_meta(&self, key: &str) -> Result<()> {
        self.write(|txn| {
            txn.open_table(META)?.remove(key)?;
            Ok(())
        })
    }
}

/// 反序列化可选的 `JSON` 载荷。
fn decode<T: DeserializeOwned>(raw: Option<Vec<u8>>) -> Result<Option<T>> {
    Ok(raw.map(|bytes| serde_json::from_slice(&bytes)).transpose()?)
}

/// 在系统临时目录里占位一个唯一文件名。
fn unique_temp_path() -> Result<PathBuf> {
    let dir = std::env::temp_dir();
    for _ in 0..64 {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("lyra-store-{}-{nanos}-{seq}.redb", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "无法在临时目录创建唯一的数据库文件",
    )))
}

/// `expires == 0` 表示永不过期。
fn is_expired(expires: u64, now: u64) -> bool {
    expires != 0 && expires <= now
}

/// 当前 Unix 毫秒时间戳。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// `Duration` 转毫秒（饱和到 u64）。
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// 格式化 VIP 令牌在系统凭据库中的专属键名。
fn vip_secret_user(userid: i64) -> String {
    format!("{userid}#vip")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

    use super::*;

    /// 使用内存凭据库构造临时存储实例。
    fn temp_store() -> Store {
        open_temp_with_secrets(Box::new(MemorySecrets::new())).unwrap()
    }

    #[test]
    fn both_tokens_round_trip_through_the_secret_store() {
        let store = temp_store();
        store
            .set_account(&Account {
                userid: 2_043_841_403,
                token: "TKN-SECRET".to_owned(),
                vip_type: 1,
                vip_token: "VIP-SECRET".to_owned(),
                nickname: "lyra".to_owned(),
            })
            .unwrap();

        // 验证两类凭据隔离存储
        assert_eq!(store.secrets.get("2043841403").unwrap().as_deref(), Some("TKN-SECRET"));
        assert_eq!(
            store.secrets.get(&vip_secret_user(2_043_841_403)).unwrap().as_deref(),
            Some("VIP-SECRET")
        );
        // 读取登录态时从凭据库回填令牌
        let read_back = store.account().unwrap().unwrap();
        assert_eq!(read_back.token, "TKN-SECRET");
        assert_eq!(read_back.vip_token, "VIP-SECRET");
        assert_eq!(read_back.nickname, "lyra");

        // 登出时同步清理凭据库
        store.clear_account().unwrap();
        assert!(store.account().unwrap().is_none());
        assert!(store.secrets.get("2043841403").unwrap().is_none());
        assert!(store.secrets.get(&vip_secret_user(2_043_841_403)).unwrap().is_none());
    }

    #[test]
    fn no_credential_is_written_to_the_redb_file() {
        // 验证 redb 数据库文件中不存在任何敏感令牌明文
        let dir = std::env::temp_dir().join(format!(
            "lyra-store-plaintext-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lyra.redb");

        let store = open_with_secrets(&path, Box::new(MemorySecrets::new())).unwrap();
        store
            .set_account(&Account {
                userid: 2_043_841_403,
                token: "TKN-SECRET".to_owned(),
                vip_type: 1,
                vip_token: "VIP-SECRET".to_owned(),
                nickname: "lyra".to_owned(),
            })
            .unwrap();
        drop(store);

        let raw = std::fs::read(&path).unwrap();
        assert!(contains(&raw, b"lyra"), "非敏感元数据应该在 redb 里");
        assert!(!contains(&raw, b"TKN-SECRET"), "token 明文出现在 redb 文件里");
        assert!(
            !contains(&raw, b"VIP-SECRET"),
            "vip_token 同样是凭据，不能落明文（方案风险 R7）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 在字节流里找一段子序列。
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn device_round_trip_and_missing() {
        let store = temp_store();
        assert!(store.device().unwrap().is_none());

        let device = lyra_proto::Device::generate();
        store.set_device(&device).unwrap();
        assert_eq!(store.device().unwrap().as_ref(), Some(&device));
    }

    #[test]
    fn settings_default_and_round_trip() {
        let store = temp_store();
        let default = store.settings().unwrap();
        assert_eq!(default, Settings::default());
        assert!((default.volume - 0.8).abs() < f32::EPSILON);
        assert_eq!(default.quality, "flac");
        assert_eq!(default.max_cache_bytes, 2 * 1024 * 1024 * 1024);

        let settings = Settings {
            volume: 0.42,
            quality: "320k".to_owned(),
            cache_dir: Some("D:/cache".to_owned()),
            max_cache_bytes: 123,
        };
        store.set_settings(&settings).unwrap();
        assert_eq!(store.settings().unwrap(), settings);
    }

    #[test]
    fn blob_cache_round_trip_ttl_and_namespaces() {
        let store = temp_store();

        store.put_blob("meta", "a", b"hello", None).unwrap();
        assert_eq!(
            store.get_blob("meta", "a").unwrap().as_deref(),
            Some(b"hello".as_slice())
        );
        assert!(store.get_blob("meta", "missing").unwrap().is_none());

        // 零 TTL 视为立即过期。
        store.put_blob("meta", "ttl", b"x", Some(Duration::ZERO)).unwrap();
        assert!(store.get_blob("meta", "ttl").unwrap().is_none());
        assert!(store.drop_expired().unwrap() >= 1);

        // 命名空间隔离。
        store.put_blob("n1", "k", b"1", None).unwrap();
        store.put_blob("n2", "k", b"2", None).unwrap();
        assert_eq!(store.clear_ns("n1").unwrap(), 1);
        assert!(store.get_blob("n1", "k").unwrap().is_none());
        assert_eq!(store.get_blob("n2", "k").unwrap().as_deref(), Some(b"2".as_slice()));
    }

    #[test]
    fn history_dedup_order_and_clear() {
        let store = temp_store();
        let entry = |hash: &str, ms: i64| HistoryEntry {
            hash: hash.to_owned(),
            name: "n".to_owned(),
            singer: "s".to_owned(),
            album_id: "a".to_owned(),
            album_audio_id: "b".to_owned(),
            played_at_ms: ms,
        };

        store.push_history(&entry("h1", 100)).unwrap();
        store.push_history(&entry("h2", 200)).unwrap();
        store.push_history(&entry("h3", 300)).unwrap();
        let hashes: Vec<String> = store.history(10).unwrap().into_iter().map(|e| e.hash).collect();
        assert_eq!(hashes, ["h3", "h2", "h1"]);

        // 同 hash 覆盖而非新增。
        store.push_history(&entry("h2", 400)).unwrap();
        let recent = store.history(10).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].hash, "h2");
        assert_eq!(recent[0].played_at_ms, 400);

        assert_eq!(store.history(2).unwrap().len(), 2);

        store.clear_history().unwrap();
        assert!(store.history(10).unwrap().is_empty());
    }

    #[test]
    fn account_token_round_trip_via_secrets() {
        let store = temp_store();
        assert!(store.account().unwrap().is_none());

        let account = Account {
            userid: 42,
            token: "secret-token".to_owned(),
            vip_type: 1,
            vip_token: "vt".to_owned(),
            nickname: "me".to_owned(),
        };
        store.set_account(&account).unwrap();
        assert_eq!(store.account().unwrap().as_ref(), Some(&account));

        store.clear_account().unwrap();
        assert!(store.account().unwrap().is_none());
    }

    #[test]
    fn open_temp_files_are_unique() {
        let store_a = temp_store();
        let store_b = temp_store();

        let path_a = store_a._temp.as_ref().unwrap().0.clone();
        let path_b = store_b._temp.as_ref().unwrap().0.clone();
        assert_ne!(path_a, path_b);
        assert!(path_a.exists() && path_b.exists());

        // 两个实例互不可见。
        store_a.put_blob("t", "k", b"a", None).unwrap();
        assert!(store_b.get_blob("t", "k").unwrap().is_none());

        // Drop 时文件被删除。
        drop(store_a);
        assert!(!path_a.exists());
    }
}
