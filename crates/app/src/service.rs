//! 用例门面：组装持久化、网络接口与音频引擎。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lyra_api::endpoint::{audio, flow, library, lyrics, playlist, privilege, session};
use lyra_api::model::{LyricLine, Page, Playlist, Song, SongRights, SongUrl, Station, StationClass, VipStatus};
use lyra_api::transport::Transport;
use lyra_api::{Api, Session};
use lyra_audio::{AudioConfig, Engine, Event, TrackRef};
use lyra_store::{Account as StoreAccount, Settings, Store};

use crate::error::{Error, Result};
use crate::fetcher::BlobFetcher;

/// 默认音质回落优先级（从高到低）。
const QUALITY_FALLBACK: &[&str] = &["flac", "320", "128"];

/// 酷狗核心业务用例门面。
#[derive(Clone)]
pub struct Kugou {
    store: Arc<Store>,
    api: Api,
    blobs: lyra_api::blob::BlobCache,
    engine: Option<Engine>,
    data_dir: PathBuf,
}

impl std::fmt::Debug for Kugou {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kugou")
            .field("data_dir", &self.data_dir)
            .field("logged_in", &self.is_logged_in())
            .field("has_engine", &self.engine.is_some())
            .finish_non_exhaustive()
    }
}

impl Kugou {
    /// 注入自定义传输实现创建门面实例。
    pub fn with_transport(transport: Arc<dyn Transport>, store: Store, data_dir: impl Into<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let settings = store.settings()?;

        // 设备身份首次启动时生成并持久化。
        let device = if let Some(device) = store.device()? {
            device
        } else {
            let device = lyra_proto::Device::generate();
            store.set_device(&device)?;
            tracing::info!(mid = %device.mid, "已生成设备身份并落盘");
            device
        };

        let mut session = Session::new(device);
        if let Some(account) = store.account()? {
            session.set_account(Some(lyra_api::Account {
                userid: account.userid,
                token: account.token,
                vip_type: account.vip_type,
                vip_token: account.vip_token,
            }));
        }

        let blob_root = settings
            .cache_dir
            .as_ref()
            .map_or_else(|| data_dir.join("blobs"), PathBuf::from);
        let blobs = lyra_api::blob::BlobCache::new(blob_root, settings.max_cache_bytes, transport.clone());
        let api = Api::new(transport, session, 2048);

        Ok(Self {
            store: Arc::new(store),
            api,
            blobs,
            engine: None,
            data_dir,
        })
    }

    /// 打开本地数据目录并使用 Reqwest 传输创建实例。
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        // 确保数据目录存在。
        std::fs::create_dir_all(data_dir)?;
        let store = lyra_store::open(data_dir.join("lyra.redb"))?;
        Self::with_transport(Arc::new(lyra_api::http::ReqwestTransport::new()), store, data_dir)
    }

    /// 挂载音频引擎并注册事件监听回调。
    pub fn attach_audio(&mut self, events: Box<dyn Fn(Event) + Send + Sync>) -> Result<bool> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| Error::Audio("attach_audio 必须在 tokio 运行时内调用".to_owned()))?;
        let fetcher = Arc::new(BlobFetcher::new(self.blobs.clone(), runtime));
        match lyra_audio::spawn_with(fetcher, events, AudioConfig::default()) {
            Ok(engine) => {
                // 同步设置中保存的音量至音频引擎。
                let volume = self.settings().map_or(1.0, |settings| settings.volume);
                engine.set_volume(volume.clamp(0.0, 1.0));
                self.engine = Some(engine);
                Ok(true)
            }
            Err(error) => {
                tracing::warn!(%error, "缺少可用音频输出设备，已禁用播放功能");
                Ok(false)
            }
        }
    }

    /// 获取应用数据目录。
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 获取持久化存储引用。
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 获取接口层引用。
    #[must_use]
    pub fn api(&self) -> &Api {
        &self.api
    }

    /// 获取二进制 Blob 缓存引用。
    #[must_use]
    pub fn blobs(&self) -> &lyra_api::blob::BlobCache {
        &self.blobs
    }

    /// 获取音频引擎引用。
    #[must_use]
    pub fn engine(&self) -> Option<&Engine> {
        self.engine.as_ref()
    }

    /// 获取当前会话快照。
    #[must_use]
    pub fn session(&self) -> Session {
        self.api.session()
    }

    /// 查询当前是否处于登录状态。
    #[must_use]
    pub fn is_logged_in(&self) -> bool {
        self.api.session().is_logged_in()
    }

    /// 读取用户配置。
    pub fn settings(&self) -> Result<Settings> {
        Ok(self.store.settings()?)
    }

    /// 保存用户配置并同步音量与缓存上限。
    pub fn set_settings(&self, settings: &Settings) -> Result<()> {
        self.store.set_settings(settings)?;
        if let Some(engine) = &self.engine {
            engine.set_volume(settings.volume.clamp(0.0, 1.0));
        }
        // 缓存上限是运行时设置项：改完立刻生效并按 LRU 收敛，不必重启。
        self.blobs.set_max_bytes(settings.max_cache_bytes);
        Ok(())
    }

    // ---------------------------------------------------------- 账号与会话

    /// 发送短信验证码并返回剩余可用次数。
    pub async fn send_code(&self, mobile: &str) -> Result<u64> {
        if !is_mobile(mobile) {
            return Err(Error::Invalid("请输入 11 位手机号".to_owned()));
        }
        // 上游对「无 dfid 的新设备」发码极易触发风控，因此先补齐设备指纹。
        self.ensure_device_registered().await;
        let value = session::send_mobile_code(&self.api, mobile).await?;
        Ok(lyra_api::codec::find_u64(&value, &["count"]))
    }

    /// 无 dfid 时先注册设备；失败只告警，不阻断发码（缺 dfid 不是硬性前置）。
    async fn ensure_device_registered(&self) {
        if self.api.session().dfid() != "-" {
            return;
        }
        if let Err(error) = self.register_device().await {
            tracing::warn!(%error, "发码前设备注册失败，继续尝试发送");
        }
    }

    /// 短信验证码登录并持久化凭据。
    pub async fn login(&self, mobile: &str, code: &str) -> Result<StoreAccount> {
        if !is_mobile(mobile) {
            return Err(Error::Invalid("请输入 11 位手机号".to_owned()));
        }
        let result = session::login_cellphone(&self.api, mobile, code).await?;
        let account = StoreAccount {
            userid: result.userid,
            token: result.token,
            vip_type: result.vip_type,
            vip_token: result.vip_token,
            nickname: result.nickname,
        };
        // 敏感令牌安全持久化。
        self.store.set_account(&account)?;
        self.api.set_account(Some(lyra_api::Account {
            userid: account.userid,
            token: account.token.clone(),
            vip_type: account.vip_type,
            vip_token: account.vip_token.clone(),
        }));
        // 登录后注册设备获取设备指纹；失败不阻断登录，但必须留下痕迹。
        if let Err(error) = self.register_device().await {
            tracing::warn!(%error, "登录后设备注册失败，dfid 缺失会影响取播放地址");
        }
        Ok(account)
    }

    /// 退出登录并清除已持久化凭据。
    pub fn logout(&self) -> Result<()> {
        self.store.clear_account()?;
        self.api.set_account(None);
        Ok(())
    }

    /// 注册设备并持久化指纹信息，对空数据响应执行有限重试。
    pub async fn register_device(&self) -> Result<String> {
        const ATTEMPTS: u32 = 3;
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1_200);

        let mut last_error = None;
        for attempt in 1..=ATTEMPTS {
            match session::register_dev(&self.api, None).await {
                Ok(dfid) => {
                    let mut device = self.api.session().device;
                    device.dfid.clone_from(&dfid);
                    self.store.set_device(&device)?;
                    let mut session = self.api.session();
                    session.device = device;
                    self.api.set_session(session);
                    return Ok(dfid);
                }
                // 只有「成功状态但数据为空」值得重试；其余错误立即上报，避免无谓等待。
                Err(error) => {
                    let retryable = matches!(error, lyra_api::Error::EmptyData(_));
                    last_error = Some(error);
                    if !retryable {
                        break;
                    }
                }
            }
            if attempt < ATTEMPTS {
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
        Err(last_error.map_or_else(
            || Error::Api("设备注册未返回 dfid".to_owned()),
            |error| Error::Api(error.to_string()),
        ))
    }

    /// 获取当前登录用户的详细资料。
    pub async fn user_detail(&self) -> Result<serde_json::Value> {
        Ok(library::user_detail(&self.api).await?)
    }

    // ---------------------------------------------------------- 曲库管理

    /// 获取用户的全部歌单。
    pub async fn playlists(&self) -> Result<Vec<Playlist>> {
        Ok(library::playlists(&self.api).await?)
    }

    /// 分页获取指定歌单内的歌曲列表。
    pub async fn tracks(&self, global_collection_id: &str, page: u32) -> Result<Page<Song>> {
        Ok(library::playlist_tracks(&self.api, global_collection_id, page, 100).await?)
    }

    /// 添加指定歌曲至“我喜欢”歌单。
    pub async fn like(&self, song: &Song) -> Result<()> {
        Ok(playlist::add_songs(&self.api, Playlist::LIKED_LISTID, std::slice::from_ref(song)).await?)
    }

    /// 从指定歌单中移除歌曲。
    pub async fn remove(&self, listid: &str, fileids: &[i64]) -> Result<()> {
        Ok(playlist::delete_songs(&self.api, listid, fileids).await?)
    }

    // ---------------------------------------------------------- 音频播放

    /// 查询歌曲的音质权益矩阵。
    pub async fn rights(&self, song: &Song) -> Result<SongRights> {
        Ok(privilege::privilege_for_song(&self.api, song, None).await?)
    }

    /// 获取歌曲播放直链。
    pub async fn song_url(&self, song: &Song, quality: &str) -> Result<SongUrl> {
        let rights = self.rights(song).await?;
        let chosen = pick_quality(&rights, quality)?;
        let entry = rights.quality(&chosen).ok_or_else(|| Error::NoPlayableQuality {
            requested: chosen.clone(),
            available: rights.free_qualities.clone(),
        })?;
        // 若变体音质提供专属哈希则优先使用。
        let hash = if entry.hash.is_empty() {
            song.hash.clone()
        } else {
            entry.hash.clone()
        };
        let album_id = song.album_id.parse().unwrap_or(0);
        Ok(audio::song_url(
            &self.api,
            &hash,
            album_id,
            song.album_audio_id.parse().unwrap_or(0),
            &chosen,
            &rights.auth,
        )
        .await?)
    }

    /// 解析歌曲播放直链并投送至音频引擎播放。
    pub async fn play(&self, song: &Song, quality: &str) -> Result<()> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| Error::Audio("没有可用的音频输出设备".to_owned()))?;
        let url = self.song_url(song, quality).await?;
        engine.play(TrackRef {
            // 缓存键用上游给的变体 hash（内容寻址），与 CLI 的 download 共用同一条规则。
            hash: url.blob_key(&song.hash).to_owned(),
            url: url.url.clone(),
            duration_ms: if url.duration_ms == 0 {
                song.duration_ms
            } else {
                url.duration_ms
            },
            ext: url.ext_or_guess(),
        });
        Ok(())
    }

    /// 获取逐行滚动歌词。
    pub async fn lyrics(&self, song: &Song) -> Result<Vec<LyricLine>> {
        Ok(lyrics::lyric_for_song(&self.api, song).await?)
    }

    // ---------------------------------------------------------- 推荐与电台

    /// 获取每日推荐歌曲列表。
    pub async fn daily(&self) -> Result<Vec<Song>> {
        Ok(flow::daily(&self.api, "android").await?.items)
    }

    /// 分页获取个性化电台歌曲。
    pub async fn personal_fm(&self, cursor: Option<&str>, mode: &str) -> Result<Page<Song>> {
        Ok(flow::personal_fm(&self.api, "play", mode, cursor, None).await?)
    }

    /// 获取首页发现歌曲推荐。
    pub async fn discover(&self, pagesize: u32) -> Result<Vec<Song>> {
        Ok(flow::home_discover(&self.api, pagesize, 0).await?.items)
    }

    /// 搜索单曲列表。
    pub async fn search(&self, keyword: &str, page: u32) -> Result<Vec<Song>> {
        Ok(lyra_api::endpoint::search::songs(&self.api, keyword, page, 30, None).await?)
    }

    /// 获取卡片专区歌曲推荐。
    pub async fn card(&self, card_id: u32) -> Result<Vec<Song>> {
        Ok(flow::card(&self.api, card_id, 30).await?.items)
    }

    /// 获取推荐电台频道列表。
    pub async fn stations(&self) -> Result<Vec<Station>> {
        catalog_stations(&self.api).await
    }

    /// 获取电台分类列表。
    pub async fn station_classes(&self) -> Result<Vec<StationClass>> {
        catalog_classes(&self.api).await
    }

    /// 获取指定电台频道的歌曲列表。
    pub async fn station_songs(&self, fmid: &str, offset: i64) -> Result<Vec<Song>> {
        catalog_songs(&self.api, fmid, offset).await
    }

    // ---------------------------------------------------------- 会员权益

    /// 查询当前账号的会员状态。
    pub async fn vip(&self) -> Result<VipStatus> {
        Ok(privilege::vip_status(&self.api).await?)
    }

    /// 领取每日概念版畅听 VIP 权益。
    pub async fn claim_daily_vip(&self) -> Result<privilege::VipClaim> {
        Ok(privilege::claim_daily_vip(&self.api).await?)
    }

    /// 清空内存中已缓存的接口数据。
    pub fn clear_cache(&self) {
        self.api.cache().clear();
    }
}

/// 校验手机号格式是否为 11 位数字字符。
#[must_use]
pub fn is_mobile(mobile: &str) -> bool {
    let mobile = mobile.trim();
    mobile.chars().count() == 11 && mobile.chars().all(|character| character.is_ascii_digit())
}

/// 匹配最符合用户偏好且可播放的音质。
fn pick_quality(rights: &SongRights, requested: &str) -> Result<String> {
    let preferred = if requested.is_empty() { "flac" } else { requested };
    if rights.quality(preferred).is_some_and(|q| q.obtainable) {
        return Ok(preferred.to_owned());
    }
    for candidate in QUALITY_FALLBACK {
        if rights.quality(candidate).is_some_and(|q| q.obtainable) {
            return Ok((*candidate).to_owned());
        }
    }
    if !rights.best_free_quality.is_empty() {
        return Ok(rights.best_free_quality.clone());
    }
    Err(Error::NoPlayableQuality {
        requested: preferred.to_owned(),
        available: rights.free_qualities.clone(),
    })
}

// 辅助目录请求接口封装。
async fn catalog_stations(api: &Api) -> Result<Vec<Station>> {
    Ok(lyra_api::endpoint::catalog::fm_recommend(api).await?)
}
async fn catalog_classes(api: &Api) -> Result<Vec<StationClass>> {
    Ok(lyra_api::endpoint::catalog::fm_classes(api).await?)
}
async fn catalog_songs(api: &Api, fmid: &str, offset: i64) -> Result<Vec<Song>> {
    Ok(lyra_api::endpoint::catalog::fm_songs(api, fmid, 2, offset, 20).await?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn mobile_must_be_11_ascii_digits() {
        assert!(is_mobile("13800138000"));
        assert!(is_mobile(" 13800138000 "), "两侧空白应被容忍");
        // 排除包含汉字等非数字字符的情况。
        assert!(!is_mobile("一二三四五六七八九十一"));
        assert!(!is_mobile("1380013800"), "10 位不合法");
        assert!(!is_mobile("138001380000"), "12 位不合法");
        assert!(!is_mobile("1380013800a"));
        assert!(!is_mobile(""));
    }
}
