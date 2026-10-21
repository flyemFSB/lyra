#![forbid(unsafe_code)]
//! 数据持久化层：基于 redb 管理设备、账号与缓存，敏感凭据隔离存入密钥环。

mod error;
mod secrets;
mod types;

pub use error::{Error, Result};
pub use secrets::{KeyringSecrets, MemorySecrets, Secrets};
pub use types::{Account, PlayMode, Settings};

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use redb::TableDefinition;
use serde::de::DeserializeOwned;

/// 元数据表（设备、账号及设置）
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
/// 临时文件自增序号
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 当前数据库 schema 版本（方案 §4.1：redb 持久化含 schema / 迁移）
const SCHEMA_VERSION: u64 = 1;
/// schema 版本在元数据表中的键名
const SCHEMA_VERSION_KEY: &str = "schema_version";

/// 持久化存储实例
pub struct Store {
    db: redb::Database,
    secrets: Box<dyn Secrets>,
    /// 数据库文件路径（仅用于诊断输出）
    path: PathBuf,
    /// 临时文件守卫（析构时自动清理）
    temp: Option<TempFile>,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 只输出路径，不输出数据库内容
        f.debug_struct("Store")
            .field("path", &self.path)
            .field("temporary", &self.temp.is_some())
            .finish_non_exhaustive()
    }
}

/// 临时文件守卫
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

/// 打开指定路径的数据库（凭据存入系统密钥环）
pub fn open(path: impl AsRef<Path>) -> Result<Store> {
    open_with_secrets(path, Box::new(KeyringSecrets))
}

/// 打开指定路径的数据库并指定凭据存储后端
pub fn open_with_secrets(path: impl AsRef<Path>, secrets: Box<dyn Secrets>) -> Result<Store> {
    let path = path.as_ref().to_path_buf();
    let db = open_database(&path)?;
    migrate(&db)?;
    Ok(Store {
        db,
        secrets,
        path,
        temp: None,
    })
}

/// 创建临时数据库实例（Drop 时自动删除）
pub fn open_temp_with_secrets(secrets: Box<dyn Secrets>) -> Result<Store> {
    let path = unique_temp_path()?;
    let db = match open_database(&path) {
        Ok(db) => db,
        Err(error) => {
            std::fs::remove_file(&path).ok();
            return Err(error);
        }
    };
    if let Err(error) = migrate(&db) {
        std::fs::remove_file(&path).ok();
        return Err(error);
    }
    Ok(Store {
        db,
        secrets,
        path: path.clone(),
        temp: Some(TempFile(path)),
    })
}

/// 打开数据库并把「已被占用」映射成可读的 Busy 错误
fn open_database(path: &Path) -> Result<redb::Database> {
    // redb 使用文件独占锁，已被打开时返回 Busy 错误
    redb::Database::create(path).map_err(|error| match error {
        redb::DatabaseError::DatabaseAlreadyOpen => Error::Busy(path.display().to_string()),
        other => Error::Database(other),
    })
}

/// 把数据库迁移到当前 schema 版本。
///
/// v0（没有版本键的旧库）只需补写版本号，数据不动；
/// 更高版本（由更新版本写入）直接报错，避免旧程序读不懂新结构。
fn migrate(db: &redb::Database) -> Result<()> {
    match read_schema_version(db)? {
        None => write_schema_version(db, SCHEMA_VERSION),
        Some(version) if version == SCHEMA_VERSION => Ok(()),
        Some(version) if version < SCHEMA_VERSION => {
            // 逐版本迁移步骤；当前只有 v0 → v1，且不改动既有数据
            write_schema_version(db, SCHEMA_VERSION)
        }
        Some(version) => Err(Error::SchemaVersion {
            found: version,
            supported: SCHEMA_VERSION,
        }),
    }
}

/// 读取数据库的 schema 版本（没有版本键即视为 v0）
fn read_schema_version(db: &redb::Database) -> Result<Option<u64>> {
    let txn = db.begin_read().map_err(Box::new)?;
    let table = match txn.open_table(META) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let Some(guard) = table.get(SCHEMA_VERSION_KEY)? else {
        return Ok(None);
    };
    let text =
        std::str::from_utf8(guard.value()).map_err(|_| Error::Invalid("schema 版本不是合法的 UTF-8".to_owned()))?;
    text.trim()
        .parse::<u64>()
        .map(Some)
        .map_err(|_| Error::Invalid(format!("schema 版本不是合法数字：{text}")))
}

/// 写入数据库的 schema 版本
fn write_schema_version(db: &redb::Database, version: u64) -> Result<()> {
    let txn = db.begin_write().map_err(Box::new)?;
    {
        let mut table = txn.open_table(META)?;
        table.insert(SCHEMA_VERSION_KEY, version.to_string().as_bytes())?;
    }
    txn.commit()?;
    Ok(())
}

impl Store {
    /// 读取设备标识
    ///
    /// 设备行损坏时返回**错误**而不是 `None`：只有「确实没有记录」才代表首次启动，
    /// 损坏的记录必须显式暴露，绝不能被当成「没有设备」而重新生成一份新身份
    /// （方案 §5.3：随机化会导致风控命中）。
    pub fn device(&self) -> Result<Option<lyra_proto::Device>> {
        decode(self.get_meta("device")?)
    }

    /// 写入设备标识
    pub fn set_device(&self, device: &lyra_proto::Device) -> Result<()> {
        self.put_meta("device", &serde_json::to_vec(device)?)
    }

    /// 读取登录账号信息（从密钥环回填令牌）
    ///
    /// 凭据缺失（被系统清理、被手工删除或上一次写入半失败）时返回 `None`：
    /// 凭据才是登录态的唯一凭据，数据库里残留的账号行不代表已登录。
    pub fn account(&self) -> Result<Option<Account>> {
        let Some(mut account) = decode::<Account>(self.get_meta("account")?)? else {
            return Ok(None);
        };
        let Some(token) = self.secrets.get(&account.userid.to_string())? else {
            return Ok(None);
        };
        account.token = token;
        // VIP 凭据隔离存储于系统密钥环，不落明文
        if let Some(vip) = self.secrets.get(&vip_secret_user(account.userid))? {
            account.vip_token = vip;
        }
        Ok(Some(account))
    }

    /// 保存登录账号信息（令牌写入密钥环，数据库不存明文）
    ///
    /// 顺序是**先写数据库、后写凭据**：凭据写入失败时数据库里还没有账号行，
    /// 应用层会判定为「未登录」，不会带着空 token 去请求上游。
    /// 反过来（先写凭据）一旦数据库写失败，凭据就成了永远没人引用的孤儿条目。
    /// 凭据阶段失败时会把数据库行与凭据一并回滚到调用前的状态。
    pub fn set_account(&self, account: &Account) -> Result<()> {
        if account.userid == 0 {
            return Err(Error::Invalid(
                "userid 不能为 0（否则凭据会写进名为 \"0\" 的条目）".to_owned(),
            ));
        }
        let user = account.userid.to_string();
        let vip_user = vip_secret_user(account.userid);
        let mut stored = account.clone();
        stored.token = String::new();
        stored.vip_token = String::new();

        // 先记下旧值，失败时用于回滚
        let previous_row = self.get_meta("account")?;
        let previous_token = self.secrets.get(&user)?;
        let previous_vip = self.secrets.get(&vip_user)?;

        self.put_meta("account", &serde_json::to_vec(&stored)?)?;

        if let Err(error) = self.write_credentials(account, &user, &vip_user) {
            // 回滚：凭据与数据库行都恢复到调用前的状态
            let _ = self.restore_credential(&user, previous_token.as_deref());
            let _ = self.restore_credential(&vip_user, previous_vip.as_deref());
            self.restore_meta("account", previous_row.as_deref())?;
            return Err(error);
        }
        Ok(())
    }

    /// 清除登录账号与密钥环中的令牌
    ///
    /// 顺序是**先删数据库行、再删凭据**：残留一个凭据远好过残留一个
    /// 「有账号行、无凭据」的半状态（后者会让应用带着空 token 发请求）。
    pub fn clear_account(&self) -> Result<()> {
        let account = decode::<Account>(self.get_meta("account")?)?;
        self.remove_meta("account")?;
        if let Some(account) = account {
            self.secrets.delete(&account.userid.to_string())?;
            self.secrets.delete(&vip_secret_user(account.userid))?;
        }
        Ok(())
    }

    /// 读取配置项（未配置返回默认值）
    pub fn settings(&self) -> Result<Settings> {
        Ok(decode::<Settings>(self.get_meta("settings")?)?.unwrap_or_default())
    }

    /// 保存配置项（音量会被收敛到 `0.0..=1.0`）
    pub fn set_settings(&self, s: &Settings) -> Result<()> {
        if !s.volume.is_finite() {
            return Err(Error::Invalid(format!("音量必须是有限数值，收到 {}", s.volume)));
        }
        self.put_meta("settings", &serde_json::to_vec(&s.normalized())?)
    }

    /// 写事务辅助函数
    fn write<F, T>(&self, body: F) -> Result<T>
    where
        F: FnOnce(&redb::WriteTransaction) -> Result<T>,
    {
        let txn = self.db.begin_write().map_err(Box::new)?;
        let out = body(&txn)?;
        txn.commit()?;
        Ok(out)
    }

    /// 读取元数据原始字节
    fn get_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read().map_err(Box::new)?;
        let table = match txn.open_table(META) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(table.get(key)?.map(|guard| guard.value().to_vec()))
    }

    /// 写入元数据
    fn put_meta(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.write(|txn| {
            txn.open_table(META)?.insert(key, bytes)?;
            Ok(())
        })
    }

    /// 删除元数据
    fn remove_meta(&self, key: &str) -> Result<()> {
        self.write(|txn| {
            txn.open_table(META)?.remove(key)?;
            Ok(())
        })
    }

    /// 把元数据恢复成给定内容（`None` 表示删除该键）
    fn restore_meta(&self, key: &str, previous: Option<&[u8]>) -> Result<()> {
        match previous {
            Some(bytes) => self.put_meta(key, bytes),
            None => self.remove_meta(key),
        }
    }

    /// 写入凭据（令牌为空时删除对应条目）
    fn write_credentials(&self, account: &Account, user: &str, vip_user: &str) -> Result<()> {
        if account.token.is_empty() {
            self.secrets.delete(user)?;
        } else {
            self.secrets.set(user, &account.token)?;
        }
        if account.vip_token.is_empty() {
            self.secrets.delete(vip_user)?;
        } else {
            self.secrets.set(vip_user, &account.vip_token)?;
        }
        Ok(())
    }

    /// 把凭据恢复成给定状态（`None` 表示删除）
    fn restore_credential(&self, user: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(secret) => self.secrets.set(user, secret),
            None => self.secrets.delete(user),
        }
    }
}

/// 反序列化 JSON 载荷
fn decode<T: DeserializeOwned>(raw: Option<Vec<u8>>) -> Result<Option<T>> {
    Ok(raw.map(|bytes| serde_json::from_slice(&bytes)).transpose()?)
}

/// 生成唯一的临时数据库路径
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

/// 生成 VIP 令牌密钥环键名
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

    /// 写入永远失败的凭据后端（用来验证「库写成功、凭据写失败」是硬错误）。
    struct FailingSetSecrets {
        inner: MemorySecrets,
    }

    impl FailingSetSecrets {
        fn new() -> Self {
            Self {
                inner: MemorySecrets::new(),
            }
        }
    }

    impl Secrets for FailingSetSecrets {
        fn set(&self, _user: &str, _secret: &str) -> Result<()> {
            Err(Error::SecretState("凭据写入失败".to_owned()))
        }

        fn get(&self, user: &str) -> Result<Option<String>> {
            self.inner.get(user)
        }

        fn delete(&self, user: &str) -> Result<()> {
            self.inner.delete(user)
        }
    }

    /// 删除永远失败的凭据后端（用来验证登出先删库行）。
    struct FailingDeleteSecrets {
        inner: MemorySecrets,
    }

    impl FailingDeleteSecrets {
        fn new() -> Self {
            Self {
                inner: MemorySecrets::new(),
            }
        }
    }

    impl Secrets for FailingDeleteSecrets {
        fn set(&self, user: &str, secret: &str) -> Result<()> {
            self.inner.set(user, secret)
        }

        fn get(&self, user: &str) -> Result<Option<String>> {
            self.inner.get(user)
        }

        fn delete(&self, _user: &str) -> Result<()> {
            Err(Error::SecretState("凭据删除失败".to_owned()))
        }
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
    fn legacy_device_row_without_new_fields_still_deserializes() {
        // 旧版本落盘的设备记录缺少后加的字段，必须能读回来而不是重新生成身份
        let store = temp_store();
        let legacy = br#"{"guid":"e8456cee7fd2348cfa239342b10a23bd","mid":"12345678901234567890","mac":"02:00:00:00:00:00","dev":"ABCDEFGHIJ"}"#;
        store.put_meta("device", legacy).unwrap();

        let device = store.device().unwrap().expect("旧设备记录必须能读出");
        assert_eq!(device.guid, "e8456cee7fd2348cfa239342b10a23bd");
        assert_eq!(device.mid, "12345678901234567890");
        // 缺失字段回落成空串，而不是整条记录反序列化失败
        assert_eq!(device.webgl, "");
        assert_eq!(device.dfid, "");
    }

    #[test]
    fn a_corrupt_device_row_is_an_error_not_a_missing_device() {
        // 损坏的设备行必须显式报错，绝不能被当成「没有设备」而重新生成身份
        let store = temp_store();
        store.put_meta("device", b"{not-json").unwrap();
        assert!(store.device().is_err(), "损坏的设备行必须报错");
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
            play_mode: PlayMode::Shuffle,
            muted: true,
        };
        store.set_settings(&settings).unwrap();
        assert_eq!(store.settings().unwrap(), settings);
    }

    #[test]
    fn settings_without_play_mode_still_deserialize() {
        // 旧版本落的配置没有 play_mode / muted 字段，必须回落到默认值而不是报错。
        let store = temp_store();
        let legacy = br#"{"volume":0.5,"quality":"128","cache_dir":null,"max_cache_bytes":10}"#;
        store.put_meta("settings", legacy).unwrap();
        let settings = store.settings().unwrap();
        assert_eq!(settings.volume, 0.5);
        assert_eq!(settings.play_mode, PlayMode::ListLoop);
        assert!(!settings.muted);
    }

    #[test]
    fn out_of_range_volume_is_clamped_on_write() {
        // 音量越界必须被收敛，而不是把越界值写进数据库
        let store = temp_store();
        store
            .set_settings(&Settings {
                volume: 4.2,
                ..Settings::default()
            })
            .unwrap();
        assert_eq!(store.settings().unwrap().volume, 1.0);

        store
            .set_settings(&Settings {
                volume: -1.0,
                ..Settings::default()
            })
            .unwrap();
        assert_eq!(store.settings().unwrap().volume, 0.0);
    }

    #[test]
    fn a_non_finite_volume_is_rejected() {
        let store = temp_store();
        let error = store
            .set_settings(&Settings {
                volume: f32::NAN,
                ..Settings::default()
            })
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error:?}");
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
    fn a_missing_credential_means_not_logged_in() {
        // 凭据被删（系统清理 / 手工删除 / 上次写入半失败）时必须判定为「未登录」，
        // 不能返回带空 token 的账号行。
        let store = temp_store();
        store
            .set_account(&Account {
                userid: 42,
                token: "secret-token".to_owned(),
                vip_type: 0,
                vip_token: String::new(),
                nickname: "me".to_owned(),
            })
            .unwrap();
        assert!(store.account().unwrap().is_some());

        // 只删凭据，保留数据库行
        store.secrets.delete("42").unwrap();
        assert!(
            store.account().unwrap().is_none(),
            "凭据缺失时必须返回未登录，而不是空 token 的账号"
        );
    }

    #[test]
    fn a_zero_userid_is_rejected() {
        // userid == 0 会把凭据写进名为 "0" 的条目，必须拒绝
        let store = temp_store();
        let error = store
            .set_account(&Account {
                userid: 0,
                token: "secret-token".to_owned(),
                vip_type: 0,
                vip_token: String::new(),
                nickname: String::new(),
            })
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error:?}");
        assert!(store.secrets.get("0").unwrap().is_none());
    }

    #[test]
    fn the_schema_version_is_written_on_first_open() {
        // 没有版本键的旧库（v0）必须在打开时被补写版本号，且既有数据不动
        let dir = std::env::temp_dir().join(format!(
            "lyra-store-schema-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lyra.redb");

        {
            // 先造一个没有版本键的库
            let store = open_with_secrets(&path, Box::new(MemorySecrets::new())).unwrap();
            store.put_meta("device", b"{\"guid\":\"g\"}").unwrap();
            store.remove_meta(SCHEMA_VERSION_KEY).unwrap();
            drop(store);
        }

        {
            let store = open_with_secrets(&path, Box::new(MemorySecrets::new())).unwrap();
            let version = read_schema_version(&store.db).unwrap();
            assert_eq!(version, Some(SCHEMA_VERSION));
            // 既有数据必须原样保留
            assert_eq!(
                store.get_meta("device").unwrap().as_deref(),
                Some(&b"{\"guid\":\"g\"}"[..])
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_schema_version_is_refused() {
        let dir = std::env::temp_dir().join(format!(
            "lyra-store-schema-newer-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lyra.redb");

        {
            let store = open_with_secrets(&path, Box::new(MemorySecrets::new())).unwrap();
            write_schema_version(&store.db, SCHEMA_VERSION + 1).unwrap();
        }

        let error = open_with_secrets(&path, Box::new(MemorySecrets::new())).unwrap_err();
        assert!(matches!(error, Error::SchemaVersion { .. }), "{error:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Store>();
    }

    #[test]
    fn a_failing_credential_write_is_a_hard_error_and_rolls_back_the_row() {
        // 库写成功、凭据写失败必须是硬错误，并且数据库行要回滚，
        // 不留「有账号行、无凭据」的半状态。
        let store = open_temp_with_secrets(Box::new(FailingSetSecrets::new())).unwrap();
        let error = store
            .set_account(&Account {
                userid: 42,
                token: "secret-token".to_owned(),
                vip_type: 0,
                vip_token: String::new(),
                nickname: "me".to_owned(),
            })
            .unwrap_err();
        assert!(matches!(error, Error::SecretState(_)), "{error:?}");
        assert!(
            store.get_meta("account").unwrap().is_none(),
            "凭据写入失败时数据库行必须回滚"
        );
        assert!(store.account().unwrap().is_none());
    }

    #[test]
    fn clear_account_removes_the_row_before_the_credential() {
        // 凭据删除失败时账号行必须已经被删掉（残留凭据远比残留账号行安全）
        let store = open_temp_with_secrets(Box::new(FailingDeleteSecrets::new())).unwrap();
        store
            .set_account(&Account {
                userid: 42,
                token: "secret-token".to_owned(),
                vip_type: 0,
                vip_token: "vt".to_owned(),
                nickname: "me".to_owned(),
            })
            .unwrap();

        let error = store.clear_account().unwrap_err();
        assert!(matches!(error, Error::SecretState(_)), "{error:?}");
        assert!(store.get_meta("account").unwrap().is_none(), "登出必须先删除数据库行");
        assert!(store.account().unwrap().is_none());
    }

    #[test]
    fn transaction_errors_keep_their_source_chain() {
        // Transaction 变体必须保留底层 redb 错误，anyhow 链才能追到根因
        let inner = redb::TransactionError::Storage(redb::StorageError::Io(std::io::Error::other("boom")));
        let error = Error::from(Box::new(inner));
        assert!(
            std::error::Error::source(&error).is_some(),
            "事务错误必须保留底层错误链"
        );
    }

    #[test]
    fn store_debug_output_does_not_leak_content() {
        let store = temp_store();
        let text = format!("{store:?}");
        assert!(text.contains("Store"), "{text}");
        assert!(!text.contains("token"), "{text}");
    }

    #[test]
    fn opentemp_files_are_unique() {
        let store_a = temp_store();
        let store_b = temp_store();

        let path_a = store_a.temp.as_ref().unwrap().0.clone();
        let path_b = store_b.temp.as_ref().unwrap().0.clone();
        assert_ne!(path_a, path_b);
        assert!(path_a.exists() && path_b.exists());

        // 两个实例互不可见。
        store_a.set_device(&lyra_proto::Device::generate()).unwrap();
        assert!(store_b.device().unwrap().is_none());

        // Drop 时文件被删除。
        drop(store_a);
        assert!(!path_a.exists());
    }
}
