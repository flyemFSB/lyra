//! 用例门面：组装持久化、网络接口与音频引擎

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lyra_api::endpoint::{audio, flow, library, lyrics, playlist, privilege, search, session};
use lyra_api::model::{LyricLine, Page, Playlist, Song, SongRights, SongUrl, Station, StationClass, VipStatus};
use lyra_api::transport::Transport;
use lyra_api::{Api, Session};
use lyra_audio::{Engine, Event, TrackRef};
use lyra_store::{Account as StoreAccount, PlayMode, Settings, Store};

use crate::error::{Error, Result};
use crate::fetcher::BlobFetcher;
use crate::policy::{is_replacement, pick_quality};

/// 换源搜索候选上限
const REPLACEMENT_CANDIDATES: u32 = 10;

/// 单次播放请求结果
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Playback {
    /// 试听片段时长（毫秒，0 表示完整播放）
    pub preview_ms: u64,
}

/// Lyra音乐核心业务门面
#[derive(Clone)]
pub struct Kugou {
    store: Arc<Store>,
    api: Api,
    blobs: lyra_api::blob::BlobCache,
    engine: Option<Engine>,
    data_dir: PathBuf,
    /// 进程内唯一的 tokio 运行时句柄（由调用方注入，应用层不自建运行时）
    runtime: tokio::runtime::Handle,
    /// 设置读改写的互斥锁（保证「读 → 改 → 写」不会互相覆盖）
    settings_lock: Arc<std::sync::Mutex<()>>,
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
    /// 注入自定义传输实现与运行时句柄创建门面实例
    ///
    /// 运行时句柄必须由调用方提供：应用层**不创建**运行时，
    /// 全进程只应存在一个 tokio 运行时（见 PLAN §B2）。
    pub fn with_transport(
        transport: Arc<dyn Transport>,
        store: Store,
        data_dir: impl Into<PathBuf>,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let settings = store.settings()?;

        // 首次启动生成并持久化设备身份
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
            runtime,
            settings_lock: Arc::new(std::sync::Mutex::new(())),
        })
    }

    /// 打开本地数据目录并创建实例
    pub fn open(data_dir: impl AsRef<Path>, runtime: tokio::runtime::Handle) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        std::fs::create_dir_all(data_dir)?;
        let store = lyra_store::open(data_dir.join("lyra.redb"))?;
        Self::with_transport(
            Arc::new(lyra_api::http::ReqwestTransport::new()),
            store,
            data_dir,
            runtime,
        )
    }

    /// 挂载音频引擎并注册事件监听回调
    pub fn attach_audio(&mut self, events: Box<dyn Fn(Event) + Send + Sync>) -> Result<bool> {
        let fetcher = Arc::new(BlobFetcher::new(self.blobs.clone(), self.runtime.clone()));
        match lyra_audio::spawn(fetcher, events) {
            Ok(engine) => {
                // 同步持久化的音量与静音设置至音频引擎
                let (volume, muted) = match self.settings() {
                    Ok(s) => (s.volume.clamp(0.0, 1.0), s.muted),
                    Err(_) => (0.8, false),
                };
                let effective_volume = if muted { 0.0 } else { volume };
                engine.set_volume(effective_volume);
                self.engine = Some(engine);
                Ok(true)
            }
            Err(error) => {
                tracing::warn!(%error, "缺少可用音频输出设备，已禁用播放功能");
                Ok(false)
            }
        }
    }

    /// 获取应用数据目录
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 获取持久化存储引用
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 获取 Blob 缓存引用
    #[must_use]
    pub fn blobs(&self) -> &lyra_api::blob::BlobCache {
        &self.blobs
    }

    /// 获取音频引擎引用
    #[must_use]
    pub fn engine(&self) -> Option<&Engine> {
        self.engine.as_ref()
    }

    /// 获取当前会话快照
    #[must_use]
    pub fn session(&self) -> Session {
        self.api.session()
    }

    /// 查询当前是否处于登录状态
    #[must_use]
    pub fn is_logged_in(&self) -> bool {
        self.api.session().is_logged_in()
    }

    /// 读取用户配置
    pub fn settings(&self) -> Result<Settings> {
        Ok(self.store.settings()?)
    }

    /// 获取当前播放模式（缺省时回落为列表循环）
    #[must_use]
    pub fn play_mode(&self) -> PlayMode {
        match self.settings() {
            Ok(settings) => settings.play_mode,
            Err(error) => {
                tracing::warn!(%error, "读取播放模式失败，回落到默认值");
                PlayMode::default()
            }
        }
    }

    /// 获取当前播放音量配置（缺省回落为 0.8）
    #[must_use]
    pub fn volume(&self) -> f32 {
        match self.settings() {
            Ok(settings) => settings.volume.clamp(0.0, 1.0),
            Err(error) => {
                tracing::warn!(%error, "读取音量配置失败，回落到默认值");
                0.8
            }
        }
    }

    /// 获取当前是否处于静音状态（缺省为 false）
    #[must_use]
    pub fn is_muted(&self) -> bool {
        match self.settings() {
            Ok(settings) => settings.muted,
            Err(_) => false,
        }
    }

    /// 保存用户配置并同步音量与缓存上限
    pub fn set_settings(&self, settings: &Settings) -> Result<()> {
        self.store.set_settings(settings)?;
        if let Some(engine) = &self.engine {
            let effective_volume = if settings.muted {
                0.0
            } else {
                settings.volume.clamp(0.0, 1.0)
            };
            engine.set_volume(effective_volume);
        }
        // 运行时更新缓存上限并按 LRU 即时收敛
        self.blobs.set_max_bytes(settings.max_cache_bytes);
        Ok(())
    }

    /// 原子地读改写用户配置
    ///
    /// 「读 → 改 → 写」必须在一把锁内完成：多个界面动作并发时，
    /// 各自基于旧快照写回会互相覆盖对方的修改。
    pub fn update_settings(&self, mutate: impl FnOnce(&mut Settings)) -> Result<()> {
        let _guard = self
            .settings_lock
            .lock()
            .map_err(|_| Error::Invalid("配置锁已中毒".to_owned()))?;
        let mut settings = self.store.settings()?;
        mutate(&mut settings);
        self.set_settings(&settings)
    }

    // ---------------------------------------------------------- 账号与会话

    /// 发送短信验证码并返回剩余可用次数
    pub async fn send_code(&self, mobile: &str) -> Result<u64> {
        // 裁除首尾空白，避免格式校验与加密参数不一致
        let mobile = mobile.trim();
        if !is_mobile(mobile) {
            return Err(Error::Invalid("请输入 11 位手机号".to_owned()));
        }
        // 优先补齐设备指纹降低发码风控概率
        self.ensure_device_registered().await;
        let value = session::send_mobile_code(&self.api, mobile).await?;
        Ok(lyra_api::codec::find_u64(&value, &["count"]))
    }

    /// 确保设备已注册指纹（非硬性前置，失败仅记录警告）
    async fn ensure_device_registered(&self) {
        if self.api.session().dfid() != "-" {
            return;
        }
        if let Err(error) = self.register_device().await {
            tracing::warn!(%error, "发码前设备注册失败，继续尝试发送");
        }
    }

    /// 短信验证码登录并持久化凭据
    pub async fn login(&self, mobile: &str, code: &str) -> Result<StoreAccount> {
        let mobile = mobile.trim();
        let code = code.trim();
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
        self.store.set_account(&account)?;
        self.api.set_account(Some(lyra_api::Account {
            userid: account.userid,
            token: account.token.clone(),
            vip_type: account.vip_type,
            vip_token: account.vip_token.clone(),
        }));
        // 登录后尝试补齐设备指纹，失败不阻断流程
        if let Err(error) = self.register_device().await {
            tracing::warn!(%error, "登录后设备注册失败，dfid 缺失会影响取播放地址");
        }
        Ok(account)
    }

    /// 退出登录并清除持久化凭据
    ///
    /// 凭据删除要访问 redb 与系统密钥环，属于阻塞 I/O：放到独立线程执行，
    /// 避免占住 tokio 的工作线程。
    pub async fn logout(&self) -> Result<()> {
        let store = self.store.clone();
        self.runtime
            .spawn_blocking(move || store.clear_account())
            .await
            .map_err(|error| Error::Invalid(format!("退出登录失败: {error}")))??;
        self.api.set_account(None);
        Ok(())
    }

    /// 注册设备并持久化指纹信息（对空数据有限重试）
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
                    // 只在写锁内改设备字段：整体写回会话会把并发的登出/登录覆盖掉
                    self.api.update_device(|session| {
                        session.device.dfid.clone_from(&dfid);
                    });
                    return Ok(dfid);
                }
                // 仅在返回空数据时重试，其余错误立即返回
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
            || Error::Api(lyra_api::Error::EmptyData("设备注册未返回 dfid".to_owned())),
            Error::Api,
        ))
    }

    // ---------------------------------------------------------- 曲库管理

    /// 获取用户的全部歌单
    pub async fn playlists(&self) -> Result<Vec<Playlist>> {
        Ok(library::playlists(&self.api).await?)
    }

    /// 分页获取指定歌单内的歌曲列表
    pub async fn tracks(&self, global_collection_id: &str, page: u32) -> Result<Page<Song>> {
        Ok(library::playlist_tracks(&self.api, global_collection_id, page, 100).await?)
    }

    /// 添加指定歌曲至“我喜欢”歌单
    pub async fn like(&self, song: &Song) -> Result<()> {
        Ok(playlist::add_songs(&self.api, Playlist::LIKED_LISTID, std::slice::from_ref(song)).await?)
    }

    /// 从指定歌单中移除歌曲
    pub async fn remove(&self, listid: &str, fileids: &[i64]) -> Result<()> {
        Ok(playlist::delete_songs(&self.api, listid, fileids).await?)
    }

    // ---------------------------------------------------------- 音频播放

    /// 查询歌曲的音质权益矩阵
    pub async fn rights(&self, song: &Song) -> Result<SongRights> {
        Ok(privilege::privilege_for_song(&self.api, song, None).await?)
    }

    /// 获取歌曲播放直链（无版权时自动尝试换源）
    pub async fn song_url(&self, song: &Song, quality: &str) -> Result<SongUrl> {
        match self.resolve_url(song, quality).await {
            Err(Error::NoCopyright) => match self.replacement_for(song).await {
                Some(other) => {
                    tracing::info!(from = %song.hash, to = %other.hash, "无版权歌曲已换源");
                    // 换源后的失败要保留真实原因，不能一律报成「无版权」
                    self.resolve_url(&other, quality).await.map_err(|error| {
                        tracing::warn!(%error, from = %song.hash, to = %other.hash, "换源后取址失败");
                        error
                    })
                }
                None => Err(Error::NoCopyright),
            },
            other => other,
        }
    }

    /// 解析单次播放直链（不执行换源）
    async fn resolve_url(&self, song: &Song, quality: &str) -> Result<SongUrl> {
        let rights = self.rights(song).await?;
        // 无版权音源直接交由上层换源处理
        if rights.is_no_copyright() {
            return Err(Error::NoCopyright);
        }
        let chosen = pick_quality(&rights, quality)?;
        let entry = rights.quality(&chosen).ok_or_else(|| Error::NoPlayableQuality {
            requested: chosen.clone(),
            available: rights.free_qualities.clone(),
        })?;
        // 优先选用变体专属哈希
        let hash = if entry.hash.is_empty() {
            song.hash.clone()
        } else {
            entry.hash.clone()
        };
        let album_id = parse_optional_id(&song.album_id, "album_id");
        let album_audio_id = parse_optional_id(&song.album_audio_id, "album_audio_id");
        Ok(audio::song_url(&self.api, &hash, album_id, album_audio_id, &chosen, &rights.auth).await?)
    }

    /// 查找匹配的可用替换音源（同名且时长相近，优先选取免费条目）
    async fn replacement_for(&self, song: &Song) -> Option<Song> {
        let keyword = if song.singer.is_empty() {
            song.name.clone()
        } else {
            format!("{} {}", song.name, song.singer)
        };
        let hits = search::songs(&self.api, &keyword, 1, REPLACEMENT_CANDIDATES, None)
            .await
            .ok()?;
        hits.into_iter()
            .filter(|hit| is_replacement(song, hit))
            // 优先选取免费条目，次选时长最接近项
            .min_by_key(|hit| (hit.privilege != 0, hit.duration_ms.abs_diff(song.duration_ms)))
    }

    /// 解析播放直链并准备曲目（**不启动引擎**，由调用方决定何时起播）
    ///
    /// 引擎的启动被刻意留在调用方：取址是异步的，若在服务层直接起播，
    /// 后发起的请求可能先返回，导致旧歌抢占新歌。
    pub async fn prepare_play(&self, song: &Song, quality: &str) -> Result<(TrackRef, Playback)> {
        if self.engine.is_none() {
            return Err(Error::Audio("没有可用的音频输出设备".to_owned()));
        }
        let url = self.song_url(song, quality).await?;
        let track = TrackRef {
            // 使用变体哈希作为缓存键（试听片段自带 _preview 后缀隔离缓存）
            hash: url.blob_key(&song.hash),
            url: url.url.clone(),
            duration_ms: if url.duration_ms == 0 {
                song.duration_ms
            } else {
                url.duration_ms
            },
            ext: url.ext_or_guess(),
        };
        Ok((
            track,
            Playback {
                preview_ms: url.preview_ms,
            },
        ))
    }

    /// 获取逐行滚动歌词
    pub async fn lyrics(&self, song: &Song) -> Result<Vec<LyricLine>> {
        Ok(lyrics::lyric_for_song(&self.api, song).await?)
    }

    // ---------------------------------------------------------- 推荐与电台

    /// 获取每日推荐歌曲列表
    pub async fn daily(&self) -> Result<Vec<Song>> {
        Ok(flow::daily(&self.api, "android").await?.items)
    }

    /// 拉取个性化电台歌曲
    ///
    /// 首进传 `previous=None`；续拉必须传本地队列最后一首与剩余首数，
    /// 否则上游会把每次请求都当成重新开始，反复返回同一批歌。
    pub async fn personal_fm(&self, previous: Option<&Song>, remain: u32, mark: Option<&str>) -> Result<Page<Song>> {
        Ok(flow::personal_fm(&self.api, "normal", previous, remain, mark).await?)
    }

    /// 获取首页发现歌曲推荐
    pub async fn discover(&self, pagesize: u32) -> Result<Vec<Song>> {
        Ok(flow::home_discover(&self.api, pagesize, 0).await?.items)
    }

    /// 搜索单曲列表
    pub async fn search(&self, keyword: &str, page: u32) -> Result<Vec<Song>> {
        Ok(lyra_api::endpoint::search::songs(&self.api, keyword, page, 30, None).await?)
    }

    /// 获取推荐电台频道列表
    pub async fn stations(&self) -> Result<Vec<Station>> {
        catalog_stations(&self.api).await
    }

    /// 获取电台分类列表
    pub async fn station_classes(&self) -> Result<Vec<StationClass>> {
        catalog_classes(&self.api).await
    }

    /// 获取指定电台频道的歌曲列表
    pub async fn station_songs(&self, fmid: &str, offset: i64) -> Result<Vec<Song>> {
        catalog_songs(&self.api, fmid, offset).await
    }

    // ---------------------------------------------------------- 会员权益

    /// 查询当前账号的会员状态
    pub async fn vip(&self) -> Result<VipStatus> {
        Ok(privilege::vip_status(&self.api).await?)
    }

    /// 领取每日畅听 VIP
    pub async fn claim_daily_vip(&self) -> Result<privilege::VipClaim> {
        Ok(privilege::claim_daily_vip(&self.api).await?)
    }

    /// 清空内存中已缓存的 API 响应数据
    pub fn clear_cache(&self) {
        self.api.cache().clear();
    }
}

/// 校验是否为 11 位有效手机号
#[must_use]
pub fn is_mobile(mobile: &str) -> bool {
    let mobile = mobile.trim();
    mobile.chars().count() == 11 && mobile.chars().all(|character| character.is_ascii_digit())
}

/// 解析上游返回的可选数字 ID
///
/// 空串按 0 处理；非数字值记录告警后同样按 0 处理（不再静默吞掉）。
fn parse_optional_id(raw: &str, field: &str) -> i64 {
    if raw.is_empty() {
        return 0;
    }
    match raw.parse::<i64>() {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, value = raw, field, "上游返回的 ID 不是数字，按 0 处理");
            0
        }
    }
}

// 辅助目录请求封装
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
        // 包含汉字等非数字字符
        assert!(!is_mobile("一二三四五六七八九十一"));
        assert!(!is_mobile("1380013800"), "10 位不合法");
        assert!(!is_mobile("138001380000"), "12 位不合法");
        assert!(!is_mobile("1380013800a"));
        assert!(!is_mobile(""));
    }

    #[test]
    fn optional_ids_fall_back_to_zero_but_warn_on_garbage() {
        assert_eq!(parse_optional_id("", "album_id"), 0, "空串按 0 处理");
        assert_eq!(parse_optional_id("1234", "album_id"), 1234);
        assert_eq!(
            parse_optional_id("not-a-number", "album_id"),
            0,
            "非数字 ID 必须退化为 0，而不是被静默接受"
        );
    }
}
