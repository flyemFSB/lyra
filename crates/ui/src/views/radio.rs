//! 电台与广播频道列表视图组件。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{h_flex, v_flex, v_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::{Song, Station, StationClass};

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{notice, row_sizes, title};
use crate::views::tracks::track_row;

/// 电台视图状态结构。
pub(crate) struct RadioView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    classes: Vec<StationClass>,
    stations: Arc<Vec<Station>>,
    songs: Arc<Vec<Song>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    selected_class: Option<String>,
    station_name: Option<SharedString>,
    loading_catalog: bool,
    loading_songs: bool,
    loaded: bool,
    /// 目录分类与频道加载错误信息。
    catalog_error: Option<SharedString>,
    /// 频道歌曲加载错误信息。
    songs_error: Option<SharedString>,
}

impl RadioView {
    /// 创建电台视图结构。
    pub(crate) fn new(backend: Backend, shell: WeakEntity<AppShell>, _cx: &mut Context<Self>) -> Self {
        Self {
            backend,
            shell,
            classes: Vec::new(),
            stations: Arc::new(Vec::new()),
            songs: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            selected_class: None,
            station_name: None,
            loading_catalog: false,
            loading_songs: false,
            loaded: false,
            catalog_error: None,
            songs_error: None,
        }
    }

    /// 首次进入视图时触发分类与电台频道列表请求。
    pub(crate) fn ensure(&mut self, cx: &mut Context<Self>) {
        if self.loaded || self.loading_catalog {
            return;
        }
        self.loading_catalog = true;
        self.catalog_error = None;
        cx.notify();
        let app = self.backend.app();
        let classes_app = app.clone();
        self.backend.spawn(async move {
            Msg::StationClasses(classes_app.station_classes().await.map_err(|error| error.to_string()))
        });
        self.backend
            .spawn(async move { Msg::Stations(app.stations().await.map_err(|error| error.to_string())) });
    }

    /// 响应处理电台分类列表数据。
    pub(crate) fn on_classes(&mut self, result: Result<Vec<StationClass>, String>, cx: &mut Context<Self>) {
        match result {
            Ok(classes) => self.classes = classes,
            Err(error) => self.catalog_error = Some(error.into()),
        }
        cx.notify();
    }

    /// 响应处理电台频道列表数据。
    pub(crate) fn on_stations(&mut self, result: Result<Vec<Station>, String>, cx: &mut Context<Self>) {
        self.loading_catalog = false;
        match result {
            Ok(stations) => {
                self.stations = Arc::new(stations);
                self.loaded = true;
                self.songs_error = None;
            }
            Err(error) => self.songs_error = Some(error.into()),
        }
        cx.notify();
    }

    /// 响应处理频道歌曲列表数据。
    pub(crate) fn on_songs(&mut self, result: Result<Vec<Song>, String>, cx: &mut Context<Self>) {
        self.loading_songs = false;
        match result {
            Ok(songs) => {
                self.songs = Arc::new(songs);
                self.sizes = row_sizes(self.songs.len());
                self.catalog_error = None;
            }
            Err(error) => self.catalog_error = Some(error.into()),
        }
        cx.notify();
    }

    /// 切换分类过滤条件。
    fn filter_class(&mut self, classid: Option<String>, cx: &mut Context<Self>) {
        self.selected_class = classid;
        cx.notify();
    }

    /// 选择指定电台频道并请求其歌曲。
    fn select_station(&mut self, fmid: String, name: SharedString, cx: &mut Context<Self>) {
        self.station_name = Some(name);
        self.loading_songs = true;
        self.songs = Arc::new(Vec::new());
        self.sizes = row_sizes(0);
        cx.notify();
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::StationSongs(app.station_songs(&fmid, 0).await.map_err(|error| error.to_string()))
        });
    }

    /// 渲染单行歌曲组件。
    fn row(&self, index: usize, cx: &App) -> AnyElement {
        track_row(index, &self.songs, &self.shell, &self.backend, None, cx)
    }

    /// 渲染分类筛选标签条。
    fn class_chips(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut row = h_flex()
            .flex_wrap()
            .gap(px(6.0))
            .px_4()
            .py_3()
            .child(self.class_chip(None, "全部", cx));
        for class in &self.classes {
            let label = format!("{}（{}）", class.name, class.count);
            row = row.child(self.class_chip(Some(class.classid.clone()), &label, cx));
        }
        row.w_full().border_b_1().border_color(theme.border)
    }

    /// 渲染单个分类筛选标签按钮。
    fn class_chip(&self, classid: Option<String>, label: &str, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.selected_class == classid;
        let key = classid.clone().unwrap_or_default();
        div()
            .id(ElementId::Name(SharedString::from(format!("class-{key}"))))
            .px_3()
            .py_1()
            .rounded(px(999.0))
            .cursor_pointer()
            .text_sm()
            .when(active, |this| {
                this.bg(theme.primary).text_color(theme.primary_foreground)
            })
            .when(!active, |this| {
                this.bg(theme.muted)
                    .text_color(theme.muted_foreground)
                    .hover(|style| style.text_color(theme.foreground))
            })
            .on_click(cx.listener(move |view, _event, _window, cx| view.filter_class(classid.clone(), cx)))
            .child(SharedString::from(label.to_owned()))
    }

    /// 渲染电台频道卡片网格。
    fn station_grid(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let filter = self.selected_class.as_deref();
        let visible: Vec<&Station> = self
            .stations
            .iter()
            .filter(|station| filter.is_none_or(|classid| station.classid == classid))
            .collect();
        if visible.is_empty() {
            // 目录区展示错误信息。
            if let Some(error) = &self.catalog_error {
                return notice(error.clone(), cx);
            }
            return notice("正在获取频道…", cx);
        }
        let mut grid = div().flex().flex_row().flex_wrap().gap(px(8.0)).p_3();
        for (index, station) in visible.iter().enumerate() {
            let fmid = station.fmid.clone();
            let name = SharedString::from(station.name.as_str());
            let name_for_click = name.clone();
            let heat = station.heat;
            grid = grid.child(
                div()
                    .id(("station", index))
                    .w(px(150.0))
                    .p_3()
                    .rounded(px(8.0))
                    .bg(theme.muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.list_hover))
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.select_station(fmid.clone(), name_for_click.clone(), cx);
                    }))
                    .child(div().text_sm().truncate().child(name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!("热度 {heat}"))),
                    ),
            );
        }
        div()
            .id("station-grid")
            .w_full()
            .h(px(220.0))
            .flex_shrink_0()
            .overflow_y_scroll()
            .child(grid)
            .into_any_element()
    }
}

impl Render for RadioView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let song_header = h_flex()
            .w_full()
            .px_4()
            .py_2()
            .gap(px(12.0))
            .border_b_1()
            .border_color(theme.border)
            .child(title(
                self.station_name
                    .clone()
                    .unwrap_or_else(|| SharedString::new_static("请选择频道")),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!("{} 首", self.songs.len()))),
            );

        let song_body = if self.loading_songs {
            notice("加载中…", cx)
        } else if let Some(error) = &self.songs_error {
            notice(error.clone(), cx)
        } else if self.songs.is_empty() {
            notice("选择一个频道后在此显示歌曲", cx)
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("station-tracks")),
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
            .child(self.class_chips(cx))
            .child(self.station_grid(cx))
            .child(div().h(px(1.0)).w_full().bg(theme.border))
            .child(song_header)
            .child(div().flex_1().min_h(px(0.0)).child(song_body))
    }
}
