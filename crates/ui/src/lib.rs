//! 酷狗概念版界面层：运行时装配与 GPUI 视图管理。
#![forbid(unsafe_code)]
#![allow(clippy::wildcard_imports)]

mod bridge;
mod format;
mod shell;
mod theme;
mod views;

use std::sync::mpsc;

use gpui_kit::*;

use crate::bridge::Backend;

/// 启动图形界面并阻塞至窗口关闭。
pub fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let handle = runtime.handle().clone();
    let data_dir = lyra_app::cli::default_data_dir();
    let (tx, rx) = mpsc::channel::<bridge::Msg>();

    // 初始化应用服务并挂载音频引擎。
    let backend = runtime.block_on(async {
        let mut kugou = lyra_app::Kugou::open(&data_dir)?;
        // 监听音频引擎事件并转发至界面通道。
        let events_tx = tx.clone();
        let events = Box::new(move |event: lyra_audio::Event| {
            let msg = match event {
                lyra_audio::Event::Ended => bridge::Msg::AudioEnded,
                lyra_audio::Event::Failed { message } => bridge::Msg::AudioFailed(message),
            };
            let _ = events_tx.send(msg);
        });
        let _ = kugou.attach_audio(events)?;
        Ok::<_, lyra_app::Error>(Backend::new(kugou, handle.clone(), tx.clone()))
    })?;

    // 标记窗口创建状态。
    let opened = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let opened_in_closure = opened.clone();

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let result = gpui_kit::open_window(window_options(), cx, move |window, cx| {
            let entity = cx.new(|cx| shell::AppShell::new(backend, rx, cx));
            entity.update(cx, |shell, cx| shell.install(window, cx));
            entity
        });
        match result {
            Ok(_) => opened_in_closure.store(true, std::sync::atomic::Ordering::SeqCst),
            Err(error) => tracing::error!(%error, "打开窗口失败"),
        }
    });

    if opened.load(std::sync::atomic::Ordering::SeqCst) {
        Ok(())
    } else {
        Err("打开窗口失败（详见日志）".into())
    }
}

/// 配置主窗口尺寸与属性。
fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(120.0), px(80.0)),
            size(px(1180.0), px(780.0)),
        ))),
        window_min_size: Some(size(px(880.0), px(560.0))),
        titlebar: Some(TitlebarOptions {
            title: Some("lyra · 酷狗概念版".into()),
            ..Default::default()
        }),
        // 设置桌面环境应用标识。
        app_id: Some("lyra".to_owned()),
        ..Default::default()
    }
}
