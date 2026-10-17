//! 客户端桌面可执行程序入口：分发图形界面与命令行模式。

#![forbid(unsafe_code)]
// 发布版必须是 GUI 子系统：控制台子系统的 exe 从资源管理器/快捷方式启动时会额外弹出一个
// 黑框控制台窗口，用户看到的就是"打开程序先黑屏一下"。debug 保持控制台，方便 `cargo run -- <命令>` 看 CLI 输出。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("lyra=info,warn")),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        // 无命令行参数时默认启动图形界面。
        return match lyra_ui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("图形界面启动失败: {error}");
                ExitCode::FAILURE
            }
        };
    }

    // 携带命令行参数时启动异步运行时并执行 CLI 命令。
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("无法创建运行时: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(lyra_app::cli::run(&args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误: {error}");
            ExitCode::FAILURE
        }
    }
}
