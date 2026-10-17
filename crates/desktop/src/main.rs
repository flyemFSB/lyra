//! 桌面端程序入口：调度图形界面与命令行模式。

#![forbid(unsafe_code)]
// 发布版隐藏控制台窗口，Debug 模式保留控制台便于查看输出
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
        // 无参数时启动图形界面
        return match lyra_ui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("图形界面启动失败: {error}");
                ExitCode::FAILURE
            }
        };
    }

    // 有参数时执行 CLI 命令行逻辑
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
