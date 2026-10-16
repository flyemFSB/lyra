//! 异步消息桥接：将 Tokio 运行时任务结果转发回 GPUI 线程。

use std::sync::mpsc::Sender;

use lyra_api::model::{LyricLine, Page, Playlist, Song, Station, StationClass, VipStatus};
use lyra_app::Kugou;

/// 左侧导航目标页枚举。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    /// 我的歌单。
    Playlists,
    /// 每日推荐。
    Daily,
    /// 电台。
    Radio,
    /// 搜索。
    Search,
    /// 歌词。
    Lyrics,
    /// 设置。
    Settings,
    /// 登录。
    Login,
}

/// 界面消息枚举。
pub(crate) enum Msg {
    /// 验证码下发结果（剩余可发次数）。
    AuthCode(Result<u64, String>),
    /// 登录结果。
    Login(Result<(), String>),
    /// 歌单列表。
    Playlists(Result<Vec<Playlist>, String>),
    /// 某歌单的一页歌曲（带上发起时的请求序号，用于丢弃过期响应）。
    Tracks(u64, Result<Page<Song>, String>),
    /// 每日推荐。
    Daily(Result<Vec<Song>, String>),
    /// 搜索结果（带上发起时的请求序号）。
    Search(u64, Result<Vec<Song>, String>),
    /// 电台频道目录。
    Stations(Result<Vec<Station>, String>),
    /// 电台分类目录。
    StationClasses(Result<Vec<StationClass>, String>),
    /// 某频道的歌（带上发起时的请求序号）。
    StationSongs(u64, Result<Vec<Song>, String>),
    /// 当前曲目的歌词。
    Lyrics(u64, Result<Vec<LyricLine>, String>),
    /// 会员状态。
    Vip(Result<VipStatus, String>),
    /// 领取每日畅听 VIP 的结果。
    VipClaim(Result<(), String>),
    /// 设置保存结果。
    SettingsSaved(Result<(), String>),
    /// 从歌单移除的结果。
    TrackRemoved(Result<(), String>),
    /// 当前曲目自然播完。
    AudioEnded,
    /// 播放失败（含取地址失败）。
    AudioFailed(String),
    /// 完成但无需上报。
    Noop,
    /// 顶部提示。
    Toast(String),
    /// 已登出。
    LoggedOut,
    /// 从歌单移除一首（`listid`, `fileid`）。
    RemoveTrack {
        /// 歌单 `listid`。
        listid: String,
        /// 歌曲 `fileid`。
        fileid: i64,
    },
}

/// 聚合服务门面、异步运行时句柄与消息发送端。
#[derive(Clone)]
pub(crate) struct Backend {
    app: Kugou,
    handle: tokio::runtime::Handle,
    tx: Sender<Msg>,
}

impl Backend {
    /// 创建后端桥接实例。
    pub(crate) fn new(app: Kugou, handle: tokio::runtime::Handle, tx: Sender<Msg>) -> Self {
        Self { app, handle, tx }
    }

    /// 获取应用服务门面克隆。
    pub(crate) fn app(&self) -> Kugou {
        self.app.clone()
    }

    /// 查询是否存在可用音频引擎。
    pub(crate) fn has_engine(&self) -> bool {
        self.app.engine().is_some()
    }

    /// 获取音频引擎克隆。
    pub(crate) fn engine(&self) -> Option<lyra_audio::Engine> {
        self.app.engine().cloned()
    }

    /// 发送消息至界面主线程。
    pub(crate) fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    /// 在后台运行时派发异步任务并将结果回传界面。
    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let _ = tx.send(future.await);
        });
    }
}
