//! 工程维护与辅助自动化任务。

#![forbid(unsafe_code)]

use std::process::Command;

fn main() -> anyhow::Result<()> {
    let task = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    match task.as_str() {
        "audit" => audit(),
        "smoke" => smoke(),
        "package" => package(),
        "help" | "--help" | "-h" => {
            println!(
                "可用任务：\n  audit    扫描依赖树，输出非 Rust 工件清单并守住禁用依赖\n  smoke    打真实上游，验证端点存活（手动 / 定时，不进 PR CI）\n  package  构建 Release 二进制并打包为 Windows 安装程序"
            );
            Ok(())
        }
        other => anyhow::bail!("未知任务 {other}（试试 cargo xtask help）"),
    }
}

/// 明确禁止引入的非纯 Rust 传输或 TLS 依赖项。
const BANNED: &[&str] = &["native-tls", "openssl", "openssl-sys"];
/// 允许引入的非 Rust 底层密码学或编译辅助依赖。
const KNOWN_NON_RUST: &[&str] = &["ring", "aws-lc-sys", "aws-lc-rs", "cc", "blake3"];
/// 纯 Rust 实现的系统底层接口绑定库。
const SYSTEM_BINDINGS: &[&str] = &[
    "windows",
    "windows-sys",
    "windows-targets",
    "windows-link",
    "winapi",
    "schannel",
    "security-framework",
    "security-framework-sys",
];

/// 依赖分类审计统计结果。
#[derive(Debug, Default, PartialEq, Eq)]
struct Report {
    total: usize,
    banned: Vec<String>,
    non_rust: Vec<String>,
    system_bindings: Vec<String>,
}

impl Report {
    /// 校验依赖清单是否符合合规要求。
    fn is_ok(&self) -> bool {
        self.banned.is_empty()
    }
}

/// 提取分类归并依赖包列表。
fn classify(crates: &[String]) -> Report {
    let hit = |names: &[&str]| -> Vec<String> {
        let mut found: Vec<String> = crates
            .iter()
            .filter(|name| names.contains(&name.as_str()))
            .cloned()
            .collect();
        found.sort();
        found.dedup();
        found
    };
    Report {
        total: crates.len(),
        banned: hit(BANNED),
        non_rust: hit(KNOWN_NON_RUST),
        system_bindings: hit(SYSTEM_BINDINGS),
    }
}

/// 执行依赖合规性静态审计。
fn audit() -> anyhow::Result<()> {
    let output = Command::new(env!("CARGO"))
        .args(["tree", "--edges", "normal", "--prefix", "none", "--workspace"])
        .output()?;
    anyhow::ensure!(output.status.success(), "cargo tree 执行失败");

    let mut crates: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    crates.sort();
    crates.dedup();

    let report = classify(&crates);
    println!("依赖树共 {} 个 crate\n", report.total);

    println!("非 Rust 工件（预期只有 TLS 密码学实现）：");
    if report.non_rust.is_empty() {
        println!("  （无）");
    } else {
        for name in &report.non_rust {
            println!("  - {name}");
        }
    }

    println!("\n系统库绑定（纯 Rust FFI，编译期不产生 C）：");
    if report.system_bindings.is_empty() {
        println!("  （无）");
    } else {
        for name in &report.system_bindings {
            println!("  - {name}");
        }
    }

    if report.system_bindings.iter().any(|name| name == "schannel") {
        // 查证引入 Schannel 的直接父级依赖。
        let who = Command::new(env!("CARGO"))
            .args(["tree", "-i", "schannel", "--edges", "normal", "--prefix", "none"])
            .output()?;
        let who = String::from_utf8_lossy(&who.stdout);
        anyhow::ensure!(
            who.contains("rustls-native-certs"),
            "schannel 在依赖树里，但不是由 rustls-native-certs 引入的 —— \
             说明 TLS 后端可能已换成系统实现，请人工确认。cargo tree -i schannel 输出：\n{who}"
        );
        println!(
            "\n说明：schannel 由 rustls-native-certs 引入（已用 cargo tree -i 核实），\n                    仅用于枚举 Windows 受信任根证书；它本身是纯 Rust 绑定，\n                    没有把 TLS 握手交给系统后端。"
        );
    }

    anyhow::ensure!(
        report.is_ok(),
        "依赖树里出现了被禁用的 C 实现：{:?}。请检查 TLS 后端是否被改成 native-tls / OpenSSL。",
        report.banned
    );
    // 确保 keyring 启用了 windows-native 后端以正常持久化登录凭据。
    let features = Command::new(env!("CARGO"))
        .args(["tree", "-e", "features", "-i", "keyring", "--prefix", "none"])
        .output()?;
    let features = String::from_utf8_lossy(&features.stdout);
    anyhow::ensure!(
        features.contains("windows-native"),
        "keyring 没有启用 windows-native 后端，凭据会落到进程内 mock（登录态不会持久化）。\n\
         cargo tree -e features -i keyring 输出：\n{features}"
    );

    println!("\n✅ 未发现被禁用的 C 实现依赖");
    Ok(())
}

/// 上游接口存活状态探测。
/// 输出统计型端点探测结果，空列表记录为告警。
fn report_count(warnings: &mut Vec<&'static str>, name: &'static str, count: usize) {
    if count == 0 {
        println!("  ⚠️ {name:<34} 0 条");
        warnings.push(name);
    } else {
        println!("  ✅ {name:<34} {count} 条");
    }
}

/// 打印失败的端点并登记到失败列表。
fn report_error(failures: &mut Vec<&'static str>, name: &'static str, error: &impl std::fmt::Display) {
    println!("  ❌ {name:<34} {error}");
    failures.push(name);
}

fn smoke() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let dir =
            std::env::var("LYRA_DATA_DIR").map_or_else(|_| lyra_app::cli::default_data_dir(), std::path::PathBuf::from);
        let app = lyra_app::Kugou::open(&dir)?;

        let mut failures = Vec::new();
        let mut warnings = Vec::new();
        println!("上游冒烟（数据目录 {}）\n", dir.display());

        match app.register_device().await {
            Ok(dfid) => println!(
                "  ✅ {:<34} dfid={}",
                "/risk/v2/r_register_dev",
                // 截取前八位字符打印指纹。
                dfid.chars().take(8).collect::<String>()
            ),
            Err(error) => report_error(&mut failures, "/risk/v2/r_register_dev", &error),
        }
        match app.station_classes().await {
            Ok(items) => report_count(&mut warnings, "/v1/class_fm_song", items.len()),
            Err(error) => report_error(&mut failures, "/v1/class_fm_song", &error),
        }
        match app.stations().await {
            Ok(items) => report_count(&mut warnings, "/v1/rcmd_list", items.len()),
            Err(error) => report_error(&mut failures, "/v1/rcmd_list", &error),
        }
        match app.daily().await {
            Ok(items) => report_count(&mut warnings, "/everyday_song_recommend", items.len()),
            Err(error) => report_error(&mut failures, "/everyday_song_recommend", &error),
        }
        match app.personal_fm(None, "normal").await {
            Ok(page) => {
                report_count(&mut warnings, "/v2/personal_recommend", page.items.len());
                println!("     游标 {:?}", page.cursor);
            }
            Err(error) => report_error(&mut failures, "/v2/personal_recommend", &error),
        }
        match app.discover(4).await {
            Ok(items) => report_count(&mut warnings, "/homediscoverrec/...", items.len()),
            Err(error) => report_error(&mut failures, "/homediscoverrec/...", &error),
        }

        println!();
        if !warnings.is_empty() {
            println!(
                "⚠️  {} 个端点返回空表（上游版本号 / 登录态不满足时会静默返回空列表）: {warnings:?}",
                warnings.len()
            );
        }
        if failures.is_empty() {
            println!("全部端点存活");
            Ok::<(), anyhow::Error>(())
        } else {
            anyhow::bail!("{} 个端点失败: {failures:?}", failures.len())
        }
    })
}

/// 本地构建与打包 Windows 安装包。
fn package() -> anyhow::Result<()> {
    println!("📦 开始构建 Release 二进制...");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "-p", "lyra"])
        .status()?;
    anyhow::ensure!(status.success(), "cargo build --release 执行失败");

    let dist_dir = std::path::Path::new("target").join("dist");
    std::fs::create_dir_all(&dist_dir)?;

    let exe_src = std::path::Path::new("target").join("release").join("lyra.exe");
    anyhow::ensure!(exe_src.exists(), "未找到 target/release/lyra.exe");

    println!("✅ Release 二进制构建完成: {}", exe_src.display());

    // 提取统一的工作区版本号。
    let version = env!("CARGO_PKG_VERSION");

    // 生成绿色免安装压缩包
    let zip_dest = dist_dir.join(format!("Lyra-v{version}-windows-x64-portable.zip"));
    let tar_status = Command::new("tar")
        .args([
            "-a",
            "-c",
            "-f",
            &zip_dest.to_string_lossy(),
            "-C",
            "target/release",
            "lyra.exe",
        ])
        .status();
    match tar_status {
        Ok(status) if status.success() => println!("✅ 绿色便携包生成完成: {}", zip_dest.display()),
        // 捕获压缩归档异常退出状态。
        Ok(status) => println!("⚠️  tar 退出码 {status}，便携包可能未生成: {}", zip_dest.display()),
        Err(error) => println!("⚠️  无法执行 tar（{error}），已跳过便携包: {}", zip_dest.display()),
    }

    // 寻找 Inno Setup 编译器 ISCC.exe
    let iscc_paths = [
        "iscc",
        "ISCC.exe",
        r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        r"C:\Program Files\Inno Setup 6\ISCC.exe",
    ];

    let mut iscc_found = None;
    for path in &iscc_paths {
        if let Ok(output) = Command::new(path).arg("/?").output()
            && (output.status.success() || !output.stdout.is_empty() || !output.stderr.is_empty())
        {
            iscc_found = Some((*path).to_owned());
            break;
        }
    }

    if let Some(iscc) = iscc_found {
        println!("🚀 找到 Inno Setup 编译器: {iscc}，开始打包安装程序...");
        let iscc_status = Command::new(&iscc)
            .args([
                &format!("/DAppVersion={version}"),
                r"/DSourceDir=..\target\release",
                r"/DOutputDir=..\target\dist",
                r"installer\lyra.iss",
            ])
            .status()?;
        anyhow::ensure!(iscc_status.success(), "ISCC 打包失败");
        println!("✅ 安装包生成完成: target/dist/Lyra-v{version}-windows-x64-setup.exe");
    } else {
        println!("⚠️  未在系统 PATH 或默认路径中检测到 Inno Setup (ISCC.exe)。");
        println!("   提示: 可通过 `winget install JRSoftware.InnoSetup` 安装后再次运行以生成安装包。");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn ring_is_reported_as_the_only_non_rust_artifact() {
        let report = classify(&names(&["ring", "windows", "serde", "lyra-api"]));
        assert!(report.is_ok());
        assert_eq!(report.non_rust, vec!["ring".to_owned()]);
        assert_eq!(report.system_bindings, vec!["windows".to_owned()]);
        assert_eq!(report.total, 4);
    }

    #[test]
    fn openssl_and_native_tls_are_rejected() {
        assert!(!classify(&names(&["openssl-sys", "ring"])).is_ok());
        assert!(!classify(&names(&["native-tls", "schannel"])).is_ok());
        assert!(!classify(&names(&["openssl"])).is_ok());
    }

    #[test]
    fn schannel_alone_is_accepted_because_it_only_reads_the_cert_store() {
        // 验证允许 schannel 仅用于系统根证书枚举。
        assert!(classify(&names(&["schannel", "rustls-native-certs", "ring"])).is_ok());
        assert!(!classify(&names(&["schannel", "native-tls"])).is_ok());
    }

    #[test]
    fn duplicates_are_collapsed() {
        let report = classify(&names(&["ring", "ring", "serde"]));
        assert_eq!(report.non_rust, vec!["ring".to_owned()]);
        assert_eq!(report.total, 3, "输入总数包含重复项");
    }
}
