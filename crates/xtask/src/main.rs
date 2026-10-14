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

    /// 验证 Schannel 是否仅用于读取系统根证书。
    fn schannel_is_only_a_cert_reader(&self) -> bool {
        !self.system_bindings.iter().any(|name| name == "schannel")
            || !self.banned.iter().any(|name| name == "native-tls")
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
    anyhow::ensure!(
        report.schannel_is_only_a_cert_reader(),
        "native-tls 与 schannel 同时存在 —— TLS 握手可能已交给系统后端"
    );
    println!("\n✅ 未发现被禁用的 C 实现依赖");
    Ok(())
}

/// 上游接口存活状态探测。
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
        println!("上游冒烟（数据目录 {}）\n", dir.display());

        match app.register_device().await {
            Ok(dfid) => println!(
                "  ✅ /risk/v2/r_register_dev         dfid={}",
                // 截取前八位字符打印指纹。
                dfid.chars().take(8).collect::<String>()
            ),
            Err(error) => {
                println!("  ❌ /risk/v2/r_register_dev         {error}");
                failures.push("register_dev");
            }
        }

        match app.station_classes().await {
            Ok(items) => println!("  ✅ /v1/class_fm_song               {} 个分类", items.len()),
            Err(error) => {
                println!("  ❌ /v1/class_fm_song               {error}");
                failures.push("class_fm_song");
            }
        }
        match app.stations().await {
            Ok(items) => println!("  ✅ /v1/rcmd_list                   {} 个电台", items.len()),
            Err(error) => {
                println!("  ❌ /v1/rcmd_list                   {error}");
                failures.push("rcmd_list");
            }
        }
        match app.daily().await {
            Ok(items) => println!("  ✅ /everyday_song_recommend        {} 首", items.len()),
            Err(error) => {
                println!("  ❌ /everyday_song_recommend        {error}");
                failures.push("everyday_song_recommend");
            }
        }
        match app.personal_fm(None, "normal").await {
            Ok(page) => println!(
                "  ✅ /v2/personal_recommend          {} 首，游标 {:?}",
                page.items.len(),
                page.cursor
            ),
            Err(error) => {
                println!("  ❌ /v2/personal_recommend          {error}");
                failures.push("personal_recommend");
            }
        }
        match app.discover(4).await {
            Ok(items) => println!("  ✅ /homediscoverrec/...             {} 首", items.len()),
            Err(error) => {
                println!("  ❌ /homediscoverrec/...             {error}");
                failures.push("home_discover");
            }
        }

        println!();
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

    // 生成绿色免安装压缩包
    let zip_dest = dist_dir.join("Lyra-v0.1.0-windows-x64-portable.zip");
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
    if let Ok(status) = tar_status
        && status.success()
    {
        println!("✅ 绿色便携包生成完成: {}", zip_dest.display());
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
                "/DAppVersion=0.1.0",
                r"/DSourceDir=..\target\release",
                r"/DOutputDir=..\target\dist",
                r"installer\lyra.iss",
            ])
            .status()?;
        anyhow::ensure!(iscc_status.success(), "ISCC 打包失败");
        println!("✅ 安装包生成完成: target/dist/Lyra-v0.1.0-windows-x64-setup.exe");
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
        let report = classify(&names(&["schannel", "rustls-native-certs", "ring"]));
        assert!(report.is_ok());
        assert!(report.schannel_is_only_a_cert_reader());
        // 但和 native-tls 一起出现就说明 TLS 走了系统后端
        let bad = classify(&names(&["schannel", "native-tls"]));
        assert!(!bad.is_ok());
        assert!(!bad.schannel_is_only_a_cert_reader());
    }

    #[test]
    fn duplicates_are_collapsed() {
        let report = classify(&names(&["ring", "ring", "serde"]));
        assert_eq!(report.non_rust, vec!["ring".to_owned()]);
        assert_eq!(report.total, 3, "输入总数包含重复项");
    }
}
