//! 电台分类、频道卡片与电台曲目列表视图。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, Sizable as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::{Song, Station, StationClass};

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_BORDER, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CARD_BG, CARD_BORDER, SURFACE_HOVER,
    TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, VIP_GOLD, notice_with_icon, row_sizes, title,
};
use crate::views::player::PlayerModel;
use crate::views::tracks::track_row;

/// 电台视图
pub(crate) struct RadioView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    classes: Vec<StationClass>,
    stations: Arc<Vec<Station>>,
    songs: Arc<Vec<Song>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
    selected_class: Option<String>,
    station_name: Option<SharedString>,
    loading_catalog: bool,
    loading_songs: bool,
    loaded: bool,
    /// 分类与频道加载错误信息
    catalog_error: Option<SharedString>,
    /// 频道歌曲加载错误信息
    songs_error: Option<SharedString>,
    /// 请求序号（用于丢弃过期响应）
    request_seq: u64,
    _sub: Subscription,
}

impl RadioView {
    /// 创建电台视图并订阅播放器状态
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        player: Entity<PlayerModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.observe(&player, |_this, _player, cx| {
            cx.notify();
        });
        Self {
            backend,
            shell,
            player,
            classes: Vec::new(),
            stations: Arc::new(Vec::new()),
            songs: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            scroll_handle: VirtualListScrollHandle::new(),
            selected_class: None,
            station_name: None,
            loading_catalog: false,
            loading_songs: false,
            loaded: false,
            catalog_error: None,
            songs_error: None,
            request_seq: 0,
            _sub: sub,
        }
    }

    /// 重置视图状态并废弃在途请求
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.classes = Vec::new();
        self.stations = Arc::new(Vec::new());
        self.songs = Arc::new(Vec::new());
        self.sizes = row_sizes(0);
        self.scroll_handle = VirtualListScrollHandle::new();
        self.selected_class = None;
        self.station_name = None;
        self.loading_catalog = false;
        self.loading_songs = false;
        self.loaded = false;
        self.catalog_error = None;
        self.songs_error = None;
        self.request_seq += 1;
        cx.notify();
    }

    /// 确保电台分类与频道数据已加载
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

    /// 处理电台分类响应结果
    pub(crate) fn on_classes(&mut self, result: Result<Vec<StationClass>, String>, cx: &mut Context<Self>) {
        match result {
            Ok(classes) => {
                self.classes = classes;
                self.catalog_error = None;
            }
            Err(error) => self.catalog_error = Some(error.into()),
        }
        cx.notify();
    }

    /// 处理电台频道列表响应结果
    pub(crate) fn on_stations(&mut self, result: Result<Vec<Station>, String>, cx: &mut Context<Self>) {
        self.loading_catalog = false;
        self.loaded = true;
        match result {
            Ok(stations) => {
                self.stations = Arc::new(stations);
                self.catalog_error = None;
                if self.station_name.is_none() && !self.stations.is_empty() {
                    self.select_station(0, cx);
                }
            }
            Err(error) => self.catalog_error = Some(error.into()),
        }
        cx.notify();
    }

    /// 处理电台曲目列表响应结果
    pub(crate) fn on_songs(&mut self, seq: u64, result: Result<Vec<Song>, String>, cx: &mut Context<Self>) {
        if seq != self.request_seq {
            return;
        }
        self.loading_songs = false;
        match result {
            Ok(songs) => {
                self.sizes = row_sizes(songs.len());
                self.songs = Arc::new(songs);
                self.songs_error = None;
            }
            Err(error) => {
                self.songs_error = Some(error.into());
                self.sizes = row_sizes(0);
                self.songs = Arc::new(Vec::new());
            }
        }
        cx.notify();
    }

    /// 切换选中的电台分类
    fn select_class(&mut self, classid: Option<String>, cx: &mut Context<Self>) {
        self.selected_class = classid;
        cx.notify();
    }

    /// 选中电台频道并拉取曲目
    fn select_station(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(station) = self.filtered_stations().get(index).cloned() else {
            return;
        };
        self.station_name = Some(SharedString::from(station.name));
        self.loading_songs = true;
        self.songs_error = None;
        self.scroll_handle = VirtualListScrollHandle::new();
        let fmid = station.fmid;
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::StationSongs(
                seq,
                app.station_songs(&fmid, 0).await.map_err(|error| error.to_string()),
            )
        });
    }

    /// 渲染单首曲目行
    fn row(&self, index: usize, cx: &App) -> AnyElement {
        let (is_current, is_playing) = {
            let model = self.player.read(cx);
            let cur_hash = model.current_song().map(|s| &s.hash);
            let this_hash = self.songs.get(index).map(|s| &s.hash);
            (
                cur_hash.is_some() && cur_hash == this_hash,
                model.snapshot.state == lyra_audio::State::Playing,
            )
        };
        track_row(
            index,
            &self.songs,
            &self.shell,
            &self.backend,
            None,
            is_current,
            is_playing,
            cx,
        )
    }

    /// 渲染分类标签栏
    fn class_chips(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut row = h_flex()
            .flex_wrap()
            .gap(px(8.0))
            .px_5()
            .py_3()
            .child(self.class_chip(None, "全部频道", 0, cx));
        for (index, class) in self.classes.iter().enumerate() {
            let label = format!("{}（{}）", class.name, class.count);
            row = row.child(self.class_chip(Some(class.classid.clone()), &label, index + 1, cx));
        }
        row.w_full().border_b_1().border_color(CARD_BORDER)
    }

    /// 渲染单个分类胶囊标签
    fn class_chip(&self, classid: Option<String>, label: &str, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let active = self.selected_class == classid;

        div()
            .id(("class_chip", index))
            .role(Role::Button)
            .cursor_pointer()
            .px_3()
            .py_1()
            .rounded(px(999.0))
            .text_xs()
            .font_weight(if active { FontWeight::BOLD } else { FontWeight::NORMAL })
            .when(active, |this| {
                this.bg(ACCENT_GREEN_LIGHT)
                    .border_1()
                    .border_color(ACCENT_GREEN_BORDER)
                    .text_color(ACCENT_GREEN_HOVER)
            })
            .when(!active, |this| {
                this.bg(CARD_BG)
                    .border_1()
                    .border_color(CARD_BORDER)
                    .text_color(TEXT_MUTED)
                    .hover(|style| style.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
            })
            .on_click(cx.listener(move |view, _event, _window, cx| view.select_class(classid.clone(), cx)))
            .child(SharedString::from(label.to_owned()))
    }

    /// 获取当前分类过滤后的电台频道列表
    fn filtered_stations(&self) -> Vec<Station> {
        let Some(target) = &self.selected_class else {
            return (*self.stations).clone();
        };
        self.stations
            .iter()
            .filter(|station| &station.classid == target)
            .cloned()
            .collect()
    }

    /// 渲染电台频道卡片列表
    fn station_grid(&self, cx: &Context<Self>) -> AnyElement {
        let stations = self.filtered_stations();
        if self.loading_catalog && stations.is_empty() {
            return div()
                .h(px(140.0))
                .w_full()
                .child(notice_with_icon(IconName::RefreshCw, "载入电台频道中…", cx))
                .into_any_element();
        }
        if let Some(error) = &self.catalog_error {
            return div()
                .h(px(140.0))
                .w_full()
                .child(notice_with_icon(IconName::CircleAlert, error.clone(), cx))
                .into_any_element();
        }

        let mut row = h_flex()
            .id("station_scroll_row")
            .w_full()
            .px_5()
            .py_3()
            .gap(px(12.0))
            .overflow_x_scrollbar();

        for (index, station) in stations.iter().enumerate() {
            let active = self.station_name.as_ref().map(|s| s.as_ref()) == Some(&station.name);
            let name = station.name.clone();
            let heat_str = if station.heat > 10_000 {
                format!("{:.1}万", station.heat as f64 / 10_000.0)
            } else {
                format!("{}", station.heat)
            };

            row = row.child(
                div()
                    .id(("station_card", index))
                    .role(Role::Button)
                    .w(px(160.0))
                    .p_3()
                    .flex_shrink_0()
                    .rounded(px(10.0))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .when(active, |this| {
                        this.bg(ACCENT_GREEN_LIGHT).border_1().border_color(ACCENT_GREEN_BORDER)
                    })
                    .when(!active, |this| {
                        this.bg(CARD_BG)
                            .border_1()
                            .border_color(CARD_BORDER)
                            .hover(|s| s.bg(SURFACE_HOVER))
                    })
                    .on_click(cx.listener(move |view, _event, _window, cx| view.select_station(index, cx)))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded(px(8.0))
                                    .bg(ACCENT_GREEN)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::Radio).size(px(18.0)).text_color(rgb(0xffffff))),
                            )
                            .child(
                                h_flex()
                                    .gap(px(3.0))
                                    .items_center()
                                    .child(Icon::new(IconName::Flame).size(px(12.0)).text_color(VIP_GOLD))
                                    .child(div().text_xs().text_color(TEXT_FAINT).child(heat_str)),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                            .text_color(if active { ACCENT_GREEN_HOVER } else { TEXT_PRIMARY })
                            .truncate()
                            .child(name),
                    ),
            );
        }
        row.into_any_element()
    }
}

impl Render for RadioView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut song_header = h_flex()
            .w_full()
            .px_5()
            .py_3()
            .gap(px(12.0))
            .items_center()
            .border_b_1()
            .border_color(CARD_BORDER)
            .child(title(
                self.station_name
                    .clone()
                    .unwrap_or_else(|| SharedString::new_static("请选择上方电台频道")),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(TEXT_MUTED)
                    .child(SharedString::from(format!("{} 首", self.songs.len()))),
            )
            .child(div().flex_1());

        if !self.songs.is_empty() {
            let songs = self.songs.clone();
            let shell = self.shell.clone();
            song_header = song_header.child(
                Button::new("radio_play_all")
                    .primary()
                    .small()
                    .icon(IconName::Play)
                    .label("播放电台")
                    .on_click(move |_event, _window, cx| {
                        shell
                            .update(cx, |shell, cx| shell.play_queue(songs.clone(), 0, cx))
                            .ok();
                    }),
            );
        }

        let song_body = if self.loading_songs {
            notice_with_icon(IconName::RefreshCw, "正在调频载入频道歌曲…", cx)
        } else if let Some(error) = &self.songs_error {
            notice_with_icon(IconName::CircleAlert, error.clone(), cx)
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::Radio, "点击上方电台卡片即可收听实时频道", cx)
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
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(self.class_chips(cx))
            .child(self.station_grid(cx))
            .child(song_header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .p_2()
                    .vertical_scrollbar(&self.scroll_handle)
                    .child(song_body),
            )
    }
}
