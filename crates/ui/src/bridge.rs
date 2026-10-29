//! 异步消息桥接：将 Tokio 运行时任务结果转发回 GPUI 线程

use async_channel::Sender;

use lyra_api::model::{LyricLine, Page, Playlist, Song, VipStatus};
use lyra_app::Kugou;

/// 导航页面枚举
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    /// 我的歌单
    Playlists,
    /// 发现
    Daily,
    /// 搜索
    Search,
    /// 歌词
    Lyrics,
    /// 设置
    Settings,
    /// 登录
    Login,
}

/// UI 异步消息枚举
pub(crate) enum Msg {
    /// 验证码下发结果（剩余可用次数）
    AuthCode(Result<u64, String>),
    /// 短信登录结果
    Login(Result<(), String>),
    /// 歌单列表结果
    Playlists(Result<Vec<Playlist>, String>),
    /// 歌单歌曲完整列表结果（附带请求序号以过滤过期响应）
    Tracks(u64, Result<Vec<Song>, String>),
    /// 每日推荐歌曲列表
    Daily(Result<Vec<Song>, String>),
    /// 红心电台歌曲分页结果
    PersonalFm(Result<Page<Song>, String>),
    /// 搜索结果分页（附带请求序号）
    Search(u64, Result<Page<Song>, String>),
    /// 歌词获取结果（附带请求序号）
    Lyrics(u64, Result<Vec<LyricLine>, String>),
    /// 会员状态查询结果
    Vip(Result<VipStatus, String>),
    /// 每日畅听 VIP 领取结果
    VipClaim(Result<(), String>),
    /// 设置保存结果
    SettingsSaved(Result<(), String>),
    /// 移除歌曲结果
    TrackRemoved(Result<(), String>),
    /// 收藏到「我喜欢」结果
    Liked(Result<(), String>),
    /// 添加到指定歌单结果
    AddedToPlaylist(Result<(), String>),
    /// 一批歌曲的真实最高音质（hash → Hi-Res / SQ / 空串）
    Qualities(Vec<(String, &'static str)>),
    /// 当前曲目播放自然结束
    AudioEnded,
    /// 播放直链解析成功（附带请求序号，过期响应必须丢弃）
    PlayReady {
        /// 发起请求时的序号
        seq: u64,
        /// 发起请求时的歌曲哈希
        hash: String,
        /// 待播放的曲目
        track: lyra_audio::TrackRef,
        /// 试听片段时长（毫秒，0 表示完整曲）
        preview_ms: u64,
    },
    /// 播放直链解析失败
    PlayFailed {
        /// 发起请求时的序号
        seq: u64,
        /// 发起请求时的歌曲哈希
        hash: String,
        /// 错误描述
        message: String,
    },
    /// 音频引擎拉流或解码异常
    EngineFailed(String),
    /// 当前登录用户资料（昵称与真实头像）
    Profile(Result<lyra_api::endpoint::session::UserProfile, String>),
    /// 空操作
    Noop,
    /// 顶部气泡提示
    Toast(String),
    /// 已退出登录
    LoggedOut,
    /// 触发从歌单批量移除单曲
    RemoveTracks {
        /// 歌单 listid
        listid: String,
        /// 歌曲内部 fileid 列表
        fileids: Vec<i64>,
    },
}

/// 后端桥接上下文：聚合服务门面、运行时句柄与通道发送端
#[derive(Clone)]
pub(crate) struct Backend {
    app: Kugou,
    handle: tokio::runtime::Handle,
    tx: Sender<Msg>,
}

impl Backend {
    /// 创建后端桥接实例
    pub(crate) fn new(app: Kugou, handle: tokio::runtime::Handle, tx: Sender<Msg>) -> Self {
        Self { app, handle, tx }
    }

    /// 获取应用服务门面副本
    pub(crate) fn app(&self) -> Kugou {
        self.app.clone()
    }

    /// 查询音频引擎是否可用
    pub(crate) fn has_engine(&self) -> bool {
        self.app.engine().is_some()
    }

    /// 获取音频引擎副本
    pub(crate) fn engine(&self) -> Option<lyra_audio::Engine> {
        self.app.engine().cloned()
    }

    /// 获取 Tokio 运行时句柄
    ///
    /// GPUI 的前台执行器没有 Tokio reactor，任何网络 I/O 都必须挂到这个运行时上执行。
    pub(crate) fn runtime(&self) -> tokio::runtime::Handle {
        self.handle.clone()
    }

    /// 发送消息至 UI 主线程
    ///
    /// 使用无界通道 + `try_send`：UI 线程上绝不阻塞等待。
    pub(crate) fn send(&self, msg: Msg) {
        let _ = self.tx.try_send(msg);
    }

    /// 派发异步任务并将结果回传至 UI
    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let _ = tx.send(future.await).await;
        });
    }
}
