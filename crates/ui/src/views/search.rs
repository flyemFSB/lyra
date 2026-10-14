//! 歌曲搜索与结果列表视图组件。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, v_virtual_list};
use gpui_kit::*;
use lyra_api::model::Song;

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{notice, row_sizes, title};
use crate::views::tracks::track_row;

/// 歌曲搜索视图结构。
pub(crate) struct SearchView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    input: Entity<InputState>,
    songs: Arc<Vec<Song>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    loading: bool,
    /// 是否已触发过搜索请求。
    searched: bool,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

impl SearchView {
    /// 创建搜索视图实例。
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索歌曲，回车开始"));
        let sub = cx.subscribe(&input, |this, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(cx);
            }
        });
        Self {
            backend,
            shell,
            input,
            songs: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            loading: false,
            searched: false,
            error: None,
            _subs: vec![sub],
        }
    }

    /// 提交当前搜索框中的关键字。
    pub(crate) fn submit(&mut self, cx: &mut Context<Self>) {
        let keyword = self.input.read(cx).value().to_string();
        if !is_searchable(&keyword) {
            return;
        }
        self.loading = true;
        self.error = None;
        cx.notify();

        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Search(app.search(&keyword, 1).await.map_err(|error| error.to_string())) });
    }

    /// 响应处理歌曲搜索结果。
    pub(crate) fn on_search(&mut self, result: Result<Vec<Song>, String>, cx: &mut Context<Self>) {
        self.loading = false;
        self.searched = true;
        match result {
            Ok(songs) => {
                self.songs = Arc::new(songs);
                self.sizes = row_sizes(self.songs.len());
                self.error = None;
            }
            Err(error) => {
                self.songs = Arc::new(Vec::new());
                self.sizes = row_sizes(0);
                self.error = Some(error.into());
            }
        }
        cx.notify();
    }

    /// 渲染单行歌曲组件。
    fn row(&self, index: usize, cx: &App) -> AnyElement {
        track_row(index, &self.songs, &self.shell, &self.backend, None, cx)
    }
}

/// 校验搜索关键字是否有效（非空且非全空格）。
fn is_searchable(keyword: &str) -> bool {
    !keyword.trim().is_empty()
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let header = v_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap(px(10.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex().w_full().gap(px(12.0)).child(title("搜索", cx)).child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!("{} 条结果", self.songs.len()))),
                ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.0))
                    .child(div().flex_1().child(Input::new(&self.input).w_full()))
                    .child(
                        Button::new("search-go")
                            .primary()
                            .label("搜索")
                            .on_click(cx.listener(|this, _event, _window, cx| this.submit(cx))),
                    ),
            );

        let body = if self.loading {
            notice("搜索中…", cx)
        } else if let Some(error) = &self.error {
            notice(error.clone(), cx)
        } else if !self.searched {
            notice("输入关键词后回车", cx)
        } else if self.songs.is_empty() {
            notice("没有找到相关歌曲", cx)
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("search-results")),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

    use super::is_searchable;

    #[test]
    fn blank_keywords_are_not_submitted() {
        assert!(!is_searchable(""));
        assert!(!is_searchable("   "));
        assert!(is_searchable("夜曲"));
        assert!(is_searchable("  周杰伦  "));
    }
}
