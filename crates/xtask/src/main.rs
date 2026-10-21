//! 工程维护任务：依赖审计、接口冒烟与打包发布。

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let task = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    match task.as_str() {
        "audit" => audit(),
        "smoke" => smoke(),
        "package" => package(),
        "help" | "--help" | "-h" => {
            println!(
                "可用任务：\n  audit    扫描依赖树，输出非 Rust 工件清单并守住禁用依赖\n  smoke    打真实上游，验证端点存活（需要 --features smoke，且手动 / 定时，不进 PR CI）\n  package  构建 Release 二进制并打包为 Windows 安装程序（仅 Windows）"
            );
            Ok(())
        }
        other => anyhow::bail!("未知任务 {other}（试试 cargo xtask help）"),
    }
}

/// 禁止引入的非纯 Rust TLS 依赖
const BANNED: &[&str] = &["native-tls", "openssl", "openssl-sys"];
/// 允许出现的、**内含 C / 汇编源码**的依赖
///
/// 除此之外，依赖树里不允许再出现任何携带 C 代码的 crate。
const KNOWN_NON_RUST: &[&str] = &["ring"];
/// 允许的 FFI 绑定 crate
///
/// 这些 crate 只是用 Rust 声明调用系统 API，本身不携带 C 源码。
/// 其它 `-sys` / `-ffi` 结尾的 crate 通常会捆绑并编译 C 库，必须逐个人工确认。
const ALLOWED_FFI: &[&str] = &[
    "windows",
    "windows-sys",
    "windows-targets",
    "windows-link",
    "windows-strings",
    "winapi",
    "winapi-util",
    "dirs-sys",
    "vswhom-sys",
    "schannel",
    "security-framework",
    "security-framework-sys",
];

/// 依赖审计结果
#[derive(Debug, PartialEq, Eq)]
struct Report {
    /// 依赖树节点数（同一 crate 在多处出现会重复计数）
    nodes: usize,
    /// 去重后的 crate 名数量
    unique: usize,
    /// 被禁用的依赖
    banned: Vec<String>,
    /// 已确认的非 Rust 工件
    non_rust: Vec<String>,
    /// 需要人工确认的 FFI 绑定 crate
    unapproved_ffi: Vec<String>,
}

impl Report {
    /// 校验是否合规（无禁用依赖、无未确认的 FFI crate）
    fn is_ok(&self) -> bool {
        self.banned.is_empty() && self.unapproved_ffi.is_empty()
    }
}

/// 判断是否为「通常会捆绑 C 源码」的 crate
///
/// `-sys` / `-ffi` 结尾的 crate 基本都是 C 库的 Rust 绑定，
/// 它们会把 C 源码一起编译进来，必须逐个人工确认。
fn looks_like_c_binding(name: &str) -> bool {
    name.ends_with("-sys") || name.ends_with("-ffi")
}

/// 对依赖进行分类统计
///
/// `nodes` 是 `cargo tree` 的原始行（同一 crate 会重复出现），
/// 分类时按名字去重，因此 `unique` 才是「依赖树里有多少个不同的 crate」。
fn classify(nodes: &[String]) -> Report {
    let hit = |names: &[&str]| -> Vec<String> {
        let mut found: Vec<String> = nodes
            .iter()
            .filter(|name| names.contains(&name.as_str()))
            .cloned()
            .collect();
        found.sort();
        found.dedup();
        found
    };
    let mut unique: Vec<&str> = nodes.iter().map(String::as_str).collect();
    unique.sort_unstable();
    unique.dedup();
    // 需要人工确认的 FFI 绑定：既不在允许清单里，也不在已确认的非 Rust 工件里
    let unapproved_ffi: Vec<String> = unique
        .iter()
        .filter(|name| looks_like_c_binding(name) && !ALLOWED_FFI.contains(name) && !KNOWN_NON_RUST.contains(name))
        .map(|name| (*name).to_owned())
        .collect();
    Report {
        nodes: nodes.len(),
        unique: unique.len(),
        banned: hit(BANNED),
        non_rust: hit(KNOWN_NON_RUST),
        unapproved_ffi,
    }
}

/// 执行 cargo 子命令并附加错误上下文
fn cargo(args: &[&str]) -> anyhow::Result<std::process::Output> {
    Command::new(env!("CARGO"))
        .args(args)
        .output()
        .with_context(|| format!("执行 cargo {} 失败", args.join(" ")))
}

/// 执行依赖静态审计
fn audit() -> anyhow::Result<()> {
    // 必须用 `--edges all`：C 代码几乎总是经由构建脚本进入依赖树，
    // 只看 normal 边会把 `cc` 这类构建依赖整类漏掉。
    let output = cargo(&["tree", "--edges", "all", "--prefix", "none", "--workspace"])?;
    anyhow::ensure!(
        output.status.success(),
        "cargo tree 执行失败：{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    // 原始行 = 依赖树节点（含重复）；名字去重后才是「不同的 crate」
    let nodes: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();

    let report = classify(&nodes);
    println!("依赖树共 {} 个节点（{} 个不同的 crate）\n", report.nodes, report.unique);

    // 先守住禁用依赖：这是最严重的违规，不该被后面的提示淹没
    anyhow::ensure!(
        report.banned.is_empty(),
        "依赖树里出现了被禁用的 C 实现：{:?}。请检查 TLS 后端是否被改成 native-tls / OpenSSL。",
        report.banned
    );

    println!("非 Rust 工件（预期只有 TLS 密码学实现）：");
    if report.non_rust.is_empty() {
        println!("  （无）");
    } else {
        for name in &report.non_rust {
            println!("  - {name}");
        }
    }

    if report.unapproved_ffi.is_empty() {
        println!("\n✅ 未发现被禁用的 C 实现依赖，且没有未确认的 FFI 绑定 crate");
    } else {
        anyhow::ensure!(
            report.is_ok(),
            "依赖树里出现了 {} 个未确认的 FFI 绑定 crate：{:?}。\n\
             这类 crate 通常会捆绑并编译 C 源码，请确认后加入 crates/xtask/src/main.rs 的 ALLOWED_FFI 清单。",
            report.unapproved_ffi.len(),
            report.unapproved_ffi
        );
    }

    if report.non_rust.iter().any(|name| name == "ring") {
        // 核查 schannel 的依赖来源：schannel 是系统 TLS 后端，绝不能被启用
        let who = cargo(&["tree", "-i", "schannel", "--edges", "all", "--prefix", "none"])?;
        anyhow::ensure!(
            who.status.success(),
            "cargo tree -i schannel 执行失败：{}",
            String::from_utf8_lossy(&who.stderr).trim()
        );
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

    // 确保 keyring 启用 windows-native 后端
    let features = cargo(&["tree", "-e", "features", "-i", "keyring", "--prefix", "none"])?;
    anyhow::ensure!(
        features.status.success(),
        "cargo tree -e features -i keyring 执行失败：{}",
        String::from_utf8_lossy(&features.stderr).trim()
    );
    let features = String::from_utf8_lossy(&features.stdout);
    // 只匹配 keyring 自己的特性行，避免整棵树里的其它 "windows-native" 字样造成误判
    let keyring_uses_native = features
        .lines()
        .filter(|line| line.contains("keyring"))
        .any(|line| line.contains("windows-native"));
    anyhow::ensure!(
        keyring_uses_native,
        "keyring 没有启用 windows-native 后端，凭据会落到进程内 mock（登录态不会持久化）。\n\
         cargo tree -e features -i keyring 输出：\n{features}"
    );

    println!("\n✅ 依赖审计通过：没有被禁用的 C 实现，也没有未确认的 FFI 绑定 crate");
    Ok(())
}

/// 上报列表型端点结果
#[cfg(feature = "smoke")]
fn report_count(warnings: &mut Vec<&'static str>, name: &'static str, count: usize) {
    if count == 0 {
        println!("  ⚠️ {name:<34} 0 条");
        warnings.push(name);
    } else {
        println!("  ✅ {name:<34} {count} 条");
    }
}

/// 记录失败端点
#[cfg(feature = "smoke")]
fn report_error(failures: &mut Vec<&'static str>, name: &'static str, error: &impl std::fmt::Display) {
    println!("  ❌ {name:<34} {error}");
    failures.push(name);
}

/// 打真实上游，验证端点存活
#[cfg(feature = "smoke")]
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
        let app = lyra_app::Kugou::open(&dir, runtime.handle().clone())?;

        let mut failures = Vec::new();
        let mut warnings = Vec::new();
        println!("上游冒烟（数据目录 {}）\n", dir.display());

        match app.register_device().await {
            Ok(dfid) => println!(
                "  ✅ {:<34} dfid={}",
                "/risk/v2/r_register_dev",
                // 打印前 8 位指纹
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
        if !failures.is_empty() {
            anyhow::bail!("{} 个端点失败: {failures:?}", failures.len());
        }
        // 区分「端点存活且返回数据」与「端点可达但返回空表」：后者不算通过
        if warnings.is_empty() {
            println!("✅ 全部端点存活且均返回数据");
        } else {
            println!(
                "⚠️  全部端点可达，但 {} 个端点返回空列表 —— 不算通过，请人工确认上游版本号 / 登录态门槛",
                warnings.len()
            );
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// 打真实上游，验证端点存活（未启用 smoke 特性时的占位实现）
#[cfg(not(feature = "smoke"))]
fn smoke() -> anyhow::Result<()> {
    anyhow::bail!("smoke 任务需要真实上游依赖，请用：cargo run -p xtask --features smoke -- smoke")
}

/// 工作区根目录（不依赖当前工作目录）
///
/// `CARGO_MANIFEST_DIR` 指向 `crates/xtask`，工作区根目录在其上两级。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// 构建与打包 Windows 安装程序
#[cfg(windows)]
fn package() -> anyhow::Result<()> {
    let root = workspace_root();
    let release_dir = root.join("target").join("release");
    let dist_dir = root.join("target").join("dist");

    println!("📦 开始构建 Release 二进制...");
    let output = cargo(&["build", "--release", "--locked", "-p", "lyra"])?;
    anyhow::ensure!(
        output.status.success(),
        "cargo build --release 执行失败：{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    std::fs::create_dir_all(&dist_dir)?;

    let exe_src = release_dir.join("lyra.exe");
    anyhow::ensure!(exe_src.exists(), "未找到 {}", exe_src.display());

    println!("✅ Release 二进制构建完成: {}", exe_src.display());

    // 获取工作区版本号
    let version = env!("CARGO_PKG_VERSION");

    // 生成便携版压缩包（Compress-Archive 是 Windows 自带的，不依赖外部 tar）
    let zip_dest = dist_dir.join(format!("Lyra-v{version}-windows-x64-portable.zip"));
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
                exe_src.display(),
                zip_dest.display()
            ),
        ])
        .status()?;
    anyhow::ensure!(status.success(), "便携包生成失败（Compress-Archive 返回 {status}）");
    println!("✅ 绿色便携包生成完成: {}", zip_dest.display());

    // 寻找 Inno Setup 编译器 ISCC
    let mut iscc_paths = vec![
        "iscc".to_owned(),
        "ISCC.exe".to_owned(),
        r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe".to_owned(),
        r"C:\Program Files\Inno Setup 6\ISCC.exe".to_owned(),
    ];
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        iscc_paths.push(format!(r"{local_appdata}\Programs\Inno Setup 6\ISCC.exe"));
    }

    let mut iscc_found = None;
    for path in &iscc_paths {
        if let Ok(output) = Command::new(path).arg("/?").output()
            && (output.status.success() || !output.stdout.is_empty() || !output.stderr.is_empty())
        {
            iscc_found = Some(path.clone());
            break;
        }
    }

    if let Some(iscc) = iscc_found {
        println!("🚀 找到 Inno Setup 编译器: {iscc}，开始打包安装程序...");
        let source_dir = release_dir.to_string_lossy().to_string();
        let output_dir = dist_dir.to_string_lossy().to_string();
        let script = root.join("installer").join("lyra.iss").to_string_lossy().to_string();
        let iscc_status = Command::new(&iscc)
            .args([
                format!("/DAppVersion={version}"),
                format!("/DSourceDir={source_dir}"),
                format!("/DOutputDir={output_dir}"),
                script,
            ])
            .status()?;
        anyhow::ensure!(iscc_status.success(), "ISCC 打包失败");
        println!(
            "✅ 安装包生成完成: {}",
            dist_dir
                .join(format!("Lyra-v{version}-windows-x64-setup.exe"))
                .display()
        );
    } else {
        println!("⚠️  未在系统 PATH 或默认路径中检测到 Inno Setup (ISCC.exe)。");
        println!("   提示: 可通过 `winget install JRSoftware.InnoSetup` 安装后再次运行以生成安装包。");
    }

    write_checksums(&dist_dir)?;
    Ok(())
}

/// 为产物目录生成 SHA256 校验清单
///
/// 清单与产物放在同一目录：CI 与本地打包的产物集合因此完全一致。
fn write_checksums(dist_dir: &Path) -> anyhow::Result<()> {
    let manifest = dist_dir.join("checksums.sha256");
    let _ = std::fs::remove_file(&manifest);
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dist_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "exe" || ext == "zip"))
        .collect();
    entries.sort();
    let mut text = String::new();
    for path in &entries {
        let hash = sha256_hex(&std::fs::read(path)?);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let _ = writeln!(text, "{hash}  {name}");
    }
    std::fs::write(&manifest, text)?;
    println!("✅ 校验清单生成完成: {}", manifest.display());
    Ok(())
}

/// 计算文件的 SHA-256（十六进制小写）
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 构建与打包 Windows 安装程序（非 Windows 平台没有对应的安装器）
#[cfg(not(windows))]
fn package() -> anyhow::Result<()> {
    anyhow::bail!("package 任务只支持 Windows（安装器为 Inno Setup 的 .exe），当前平台无法打包")
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
        assert_eq!(report.unique, 4);
    }

    #[test]
    fn openssl_and_native_tls_are_rejected() {
        assert!(!classify(&names(&["openssl-sys", "ring"])).is_ok());
        assert!(!classify(&names(&["native-tls", "schannel"])).is_ok());
        assert!(!classify(&names(&["openssl"])).is_ok());
    }

    #[test]
    fn schannel_alone_is_accepted_because_it_only_reads_the_cert_store() {
        // 验证允许 schannel 用于系统证书枚举
        assert!(classify(&names(&["schannel", "rustls-native-certs", "ring"])).is_ok());
        assert!(!classify(&names(&["schannel", "native-tls"])).is_ok());
    }

    #[test]
    fn unexpected_c_bindings_are_reported() {
        // 没人预料到的 C 依赖必须被报出来，而不是被静默放行
        let report = classify(&names(&["libz-sys", "ring", "serde"]));
        assert!(!report.is_ok(), "未确认的 C 绑定必须让审计失败");
        assert_eq!(report.unapproved_ffi, vec!["libz-sys".to_owned()]);
        assert_eq!(report.non_rust, vec!["ring".to_owned()]);
    }

    #[test]
    fn ordinary_crates_are_not_reported() {
        let report = classify(&names(&["serde", "tokio", "windows", "ring"]));
        assert!(report.unapproved_ffi.is_empty(), "{:?}", report.unapproved_ffi);
        assert!(report.is_ok());
    }

    #[test]
    fn duplicates_are_collapsed_but_nodes_are_counted() {
        // 审计路径传的是 cargo tree 的原始行（同一 crate 会重复出现）
        let report = classify(&names(&["ring", "ring", "serde"]));
        assert_eq!(report.non_rust, vec!["ring".to_owned()]);
        assert_eq!(report.nodes, 3, "节点数包含重复项");
        assert_eq!(report.unique, 2, "去重后只有两个不同的 crate");
    }

    #[test]
    fn workspace_root_is_resolved_from_the_manifest_not_the_cwd() {
        let root = workspace_root();
        assert!(root.join("Cargo.toml").exists(), "工作区根目录应包含 Cargo.toml");
        assert!(root.join("crates").is_dir(), "工作区根目录应包含 crates/");
        assert!(
            root.join("installer").join("lyra.iss").exists(),
            "安装脚本应在工作区根目录的 installer/ 下"
        );
    }
}
