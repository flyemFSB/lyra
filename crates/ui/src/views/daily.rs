//! 每日推荐视图组件。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{h_flex, v_flex, v_virtual_list};
use gpui_kit::*;
use lyra_api::model::Song;

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{notice, row_sizes, title};
use crate::views::tracks::track_row;

/// 每日推荐视图结构。
pub(crate) struct DailyView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    songs: Arc<Vec<Song>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    loading: bool,
    loaded: bool,
    error: Option<SharedString>,
}

impl DailyView {
    /// 创建每日推荐视图。
    pub(crate) fn new(backend: Backend, shell: WeakEntity<AppShell>, _cx: &mut Context<Self>) -> Self {
        Self {
            backend,
            shell,
            songs: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            loading: false,
            loaded: false,
            error: None,
        }
    }

    /// 视图初次加载时触发推荐歌曲请求。
    pub(crate) fn ensure(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && !self.loading {
            self.reload(cx);
        }
    }

    /// 强制重新请求每日推荐歌曲。
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Daily(app.daily().await.map_err(|error| error.to_string())) });
    }

    /// 响应处理每日推荐歌曲列表数据。
    pub(crate) fn on_daily(&mut self, result: Result<Vec<Song>, String>, cx: &mut Context<Self>) {
        self.loading = false;
        match result {
            Ok(songs) => {
                self.songs = Arc::new(songs);
                self.sizes = row_sizes(self.songs.len());
                self.loaded = true;
                self.error = None;
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    /// 渲染单行歌曲组件。
    fn row(&self, index: usize, cx: &App) -> AnyElement {
        track_row(index, &self.songs, &self.shell, &self.backend, None, cx)
    }
}

impl Render for DailyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap(px(12.0))
            .border_b_1()
            .border_color(theme.border)
            .child(title("每日推荐", cx))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!("{} 首", self.songs.len()))),
            );

        let body = if self.loading && self.songs.is_empty() {
            notice("加载中…", cx)
        } else if let Some(error) = &self.error {
            notice(error.clone(), cx)
        } else if self.songs.is_empty() {
            notice("暂无推荐（登录后可用）", cx)
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("daily-tracks")),
                sizes,
                move |view, range, _window, cx| {
                    let app: &App = cx;
                    range.map(|index| view.row(index, app)).collect::<Vec<_>>()
                },
            )
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(header)
            .child(div().flex_1().min_h(px(0.0)).child(body))
    }
}
