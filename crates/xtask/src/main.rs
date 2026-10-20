//! 工程维护任务：依赖审计、接口冒烟与打包发布。

#![forbid(unsafe_code)]

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
/// 允许引入的非纯 Rust 密码学依赖（唯一被认可的非 Rust 工件）
const KNOWN_NON_RUST: &[&str] = &["ring", "aws-lc-sys", "aws-lc-rs"];
/// 纯 Rust 系统接口绑定
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

/// 已确认的**纯 Rust**依赖白名单
///
/// 依赖树里的每个 crate 都必须在此列中，否则视为「未分类」，审计直接失败。
/// 这样新增一个没人预料到的 C 依赖（如 `libz-sys` / `zstd-sys`）时能被立刻发现，
/// 而不是像「非 Rust 白名单」那样被静默放行。
const KNOWN_PURE_RUST: &[&str] = &[
    "accesskit",
    "accesskit_consumer",
    "accesskit_windows",
    "adler2",
    "aes",
    "aho-corasick",
    "annotate-snippets",
    "anstyle",
    "anyhow",
    "arc-swap",
    "arraydeque",
    "arrayref",
    "arrayvec",
    "async-channel",
    "async-executor",
    "async-fs",
    "async-io",
    "async-lock",
    "async-net",
    "async-process",
    "async-task",
    "async-trait",
    "atomic-waker",
    "backtrace",
    "base62",
    "base64",
    "base64ct",
    "bit_field",
    "bitflags",
    "block-buffer",
    "blocking",
    "block-padding",
    "bstr",
    "bytemuck",
    "byteorder",
    "byteorder-lite",
    "bytes",
    "cbc",
    "cfg-if",
    "chrono",
    "cipher",
    "color_quant",
    "concurrent-queue",
    "const-oid",
    "convert_case",
    "core_detect",
    "core_maths",
    "cpal",
    "cpufeatures",
    "crc32fast",
    "crossbeam-channel",
    "crossbeam-deque",
    "crossbeam-epoch",
    "crossbeam-utils",
    "crypto-common",
    "ctor",
    "dashmap",
    "dasp_sample",
    "data-url",
    "der",
    "derive_more",
    "derive_more-impl",
    "digest",
    "dirs",
    "dirs-sys",
    "displaydoc",
    "dunce",
    "dyn-clone",
    "either",
    "encoding_rs",
    "encoding_rs_io",
    "enum-iterator",
    "enum-iterator-derive",
    "enumn",
    "equivalent",
    "errno",
    "etagere",
    "euclid",
    "event-listener",
    "event-listener-strategy",
    "exr",
    "extended",
    "fastrand",
    "fax",
    "fdeflate",
    "flate2",
    "float_next_after",
    "float-cmp",
    "fluent-uri",
    "flume",
    "fnv",
    "foldhash",
    "fontdb",
    "form_urlencoded",
    "futf",
    "futures",
    "futures-channel",
    "futures-core",
    "futures-executor",
    "futures-io",
    "futures-lite",
    "futures-macro",
    "futures-sink",
    "futures-task",
    "futures-util",
    "generic-array",
    "getrandom",
    "gif",
    "glob",
    "globset",
    "globwalk",
    "gpui-base",
    "gpui-component",
    "gpui-component-macros",
    "gpui-kit",
    "gpui-kit-assets",
    "gpui-pre",
    "gpui-pre-collections",
    "gpui-pre-derive-refineable",
    "gpui-pre-http-client",
    "gpui-pre-macros",
    "gpui-pre-platform",
    "gpui-pre-refineable",
    "gpui-pre-scheduler",
    "gpui-pre-shared-string",
    "gpui-pre-sum-tree",
    "gpui-pre-util",
    "gpui-pre-windows",
    "gpui-pre-ztracing",
    "gpui-pre-ztracing-macro",
    "granit-parser",
    "h2",
    "half",
    "hash32",
    "hashbrown",
    "heapless",
    "heck",
    "hex",
    "html5ever",
    "http",
    "httparse",
    "http-body",
    "http-body-util",
    "hybrid-array",
    "hyper",
    "hyper-rustls",
    "hyper-util",
    "icu_collections",
    "icu_locale_core",
    "icu_normalizer",
    "icu_normalizer_data",
    "icu_properties",
    "icu_properties_data",
    "icu_provider",
    "idna",
    "idna_adapter",
    "ignore",
    "image",
    "imagesize",
    "image-webp",
    "indexmap",
    "inout",
    "instant",
    "inventory",
    "ipnet",
    "itertools",
    "itoa",
    "keyring",
    "kurbo",
    "lazy_static",
    "lebe",
    "libc",
    "libm",
    "link-section",
    "linktime-proc-macro",
    "litemap",
    "lock_api",
    "log",
    "lsp-types",
    "lyon",
    "lyon_algorithms",
    "lyon_geom",
    "lyon_path",
    "lyon_tessellation",
    "lyra",
    "lyra-api",
    "lyra-app",
    "lyra-audio",
    "lyra-proto",
    "lyra-store",
    "lyra-ui",
    "mac",
    "markdown",
    "markup5ever",
    "markup5ever_rcdom",
    "matchers",
    "md-5",
    "memchr",
    "memmap2",
    "mime",
    "mime_guess",
    "miniz_oxide",
    "mio",
    "moka",
    "moxcms",
    "multiversion_no_op",
    "new_debug_unreachable",
    "nohash-hasher",
    "normpath",
    "notify",
    "notify-types",
    "nu-ansi-term",
    "num_cpus",
    "num-bigint",
    "num-bigint-dig",
    "num-complex",
    "num-integer",
    "num-iter",
    "num-traits",
    "once_cell",
    "option-ext",
    "parking",
    "parking_lot",
    "parking_lot_core",
    "paste",
    "pem-rfc7468",
    "percent-encoding",
    "phf",
    "phf_shared",
    "pico-args",
    "pin-project",
    "pin-project-internal",
    "pin-project-lite",
    "piper",
    "pkcs1",
    "pkcs8",
    "png",
    "polling",
    "pollster",
    "polycool",
    "portable-atomic",
    "potential_utf",
    "ppv-lite86",
    "precomputed-hash",
    "primal-check",
    "proc-macro2",
    "proc-macro-crate",
    "profiling",
    "profiling-procmacros",
    "pulp",
    "pulp-wasm-simd-flag",
    "pxfm",
    "qoi",
    "quick-error",
    "quote",
    "rand",
    "rand_chacha",
    "rand_core",
    "raw-cpuid",
    "raw-window-handle",
    "rayon",
    "rayon-core",
    "realfft",
    "reborrow",
    "redb",
    "ref-cast",
    "ref-cast-impl",
    "regex",
    "regex-automata",
    "regex-syntax",
    "reqwest",
    "resvg",
    "rgb",
    "ringbuf",
    "ropey",
    "roxmltree",
    "rsa",
    "rubato",
    "rustc-demangle",
    "rustc-hash",
    "rust-embed",
    "rust-embed-impl",
    "rust-embed-utils",
    "rustfft",
    "rust-i18n",
    "rust-i18n-macro",
    "rust-i18n-support",
    "rustix",
    "rustls",
    "rustls-native-certs",
    "rustls-pki-types",
    "rustls-webpki",
    "rustversion",
    "rustybuzz",
    "ryu",
    "same-file",
    "schannel",
    "schemars",
    "schemars_derive",
    "scopeguard",
    "seahash",
    "serde",
    "serde_core",
    "serde_derive",
    "serde_derive_internals",
    "serde_json",
    "serde_repr",
    "serde_spanned",
    "serde_urlencoded",
    "serde-saphyr",
    "sha1_smol",
    "sha2",
    "sharded-slab",
    "shellexpand",
    "signature",
    "simd-adler32",
    "simdutf8",
    "similar",
    "simplecss",
    "siphasher",
    "slab",
    "slotmap",
    "smallvec",
    "smol",
    "smol_str",
    "socket2",
    "spin",
    "spki",
    "stable_deref_trait",
    "static_assertions",
    "str_indices",
    "strength_reduce",
    "strict-num",
    "string_cache",
    "strum",
    "strum_macros",
    "subtle",
    "svg_fmt",
    "svgtypes",
    "symphonia",
    "symphonia-bundle-flac",
    "symphonia-bundle-mp3",
    "symphonia-codec-aac",
    "symphonia-codec-pcm",
    "symphonia-codec-vorbis",
    "symphonia-core",
    "symphonia-format-isomp4",
    "symphonia-format-ogg",
    "symphonia-format-riff",
    "symphonia-metadata",
    "symphonia-utils-xiph",
    "syn",
    "sync_wrapper",
    "synstructure",
    "taffy",
    "tagptr",
    "tendril",
    "thiserror",
    "thiserror-impl",
    "thread_local",
    "tiff",
    "tiny-skia",
    "tiny-skia-path",
    "tinystr",
    "tinyvec",
    "tokio",
    "tokio-macros",
    "tokio-rustls",
    "tokio-util",
    "toml",
    "toml_datetime",
    "toml_edit",
    "toml_parser",
    "toml_write",
    "tower",
    "tower-http",
    "tower-layer",
    "tower-service",
    "tracing",
    "tracing-attributes",
    "tracing-core",
    "tracing-log",
    "tracing-subscriber",
    "transpose",
    "triomphe",
    "try-lock",
    "ttf-parser",
    "typenum",
    "unicase",
    "unicode-bidi",
    "unicode-bidi-mirroring",
    "unicode-ccc",
    "unicode-id",
    "unicode-ident",
    "unicode-linebreak",
    "unicode-properties",
    "unicode-script",
    "unicode-segmentation",
    "unicode-vo",
    "unicode-width",
    "untrusted",
    "url",
    "usvg",
    "utf-8",
    "utf8_iter",
    "uuid",
    "waker-fn",
    "walkdir",
    "want",
    "web-time",
    "weezl",
    "which",
    "winapi-util",
    "windows",
    "windows_x86_64_msvc",
    "windows-collections",
    "windows-core",
    "windows-future",
    "windows-implement",
    "windows-interface",
    "windows-link",
    "windows-numerics",
    "windows-registry",
    "windows-result",
    "windows-strings",
    "windows-sys",
    "windows-targets",
    "windows-threading",
    "winnow",
    "writeable",
    "xml5ever",
    "xmlwriter",
    "xtask",
    "yoke",
    "yoke-derive",
    "zerocopy",
    "zerocopy-derive",
    "zerofrom",
    "zerofrom-derive",
    "zeroize",
    "zerotrie",
    "zerovec",
    "zerovec-derive",
    "zmij",
    "zune-core",
    "zune-inflate",
    "zune-jpeg",
    // 构建脚本助手与纯 Rust 实现，不含 C 源码
    "cc",
    "blake3",
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
    /// 纯 Rust 系统接口绑定
    system_bindings: Vec<String>,
    /// 未分类依赖（既不在纯 Rust 白名单，也不在非 Rust 白名单）
    unclassified: Vec<String>,
}

impl Report {
    /// 校验是否合规（无禁用依赖、无未分类依赖）
    fn is_ok(&self) -> bool {
        self.banned.is_empty() && self.unclassified.is_empty()
    }
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
    // 未分类 = 既不在纯 Rust 白名单，也不是已确认的非 Rust 工件
    let unclassified: Vec<String> = unique
        .iter()
        .filter(|name| !KNOWN_PURE_RUST.contains(name) && !KNOWN_NON_RUST.contains(name))
        .map(|name| (*name).to_owned())
        .collect();
    Report {
        nodes: nodes.len(),
        unique: unique.len(),
        banned: hit(BANNED),
        non_rust: hit(KNOWN_NON_RUST),
        system_bindings: hit(SYSTEM_BINDINGS),
        unclassified,
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
    let output = cargo(&["tree", "--edges", "normal", "--prefix", "none", "--workspace"])?;
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

    println!("\n系统库绑定（纯 Rust FFI，编译期不产生 C）：");
    if report.system_bindings.is_empty() {
        println!("  （无）");
    } else {
        for name in &report.system_bindings {
            println!("  - {name}");
        }
    }

    if report.system_bindings.iter().any(|name| name == "schannel") {
        // 核查 schannel 的依赖来源
        let who = cargo(&["tree", "-i", "schannel", "--edges", "normal", "--prefix", "none"])?;
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

    // 未分类依赖 = 有人引入了白名单之外的新 crate，必须人工确认是不是纯 Rust
    anyhow::ensure!(
        report.is_ok(),
        "依赖树里有 {} 个未分类依赖：{:?}。\n\
         请确认它们是纯 Rust；确认后把名字加入 crates/xtask/src/main.rs 的 KNOWN_PURE_RUST 白名单。",
        report.unclassified.len(),
        report.unclassified
    );

    println!("\n✅ 未发现被禁用的 C 实现依赖，且所有依赖均已分类");
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
    let output = cargo(&["build", "--release", "-p", "lyra"])?;
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

    // 生成便携版压缩包
    let zip_dest = dist_dir.join(format!("Lyra-v{version}-windows-x64-portable.zip"));
    let tar_status = Command::new("tar")
        .args([
            "-a",
            "-c",
            "-f",
            &zip_dest.to_string_lossy(),
            "-C",
            &release_dir.to_string_lossy(),
            "lyra.exe",
        ])
        .status();
    match tar_status {
        Ok(status) if status.success() => println!("✅ 绿色便携包生成完成: {}", zip_dest.display()),
        // 检查归档退出码
        Ok(status) => println!("⚠️  tar 退出码 {status}，便携包可能未生成: {}", zip_dest.display()),
        Err(error) => println!("⚠️  无法执行 tar（{error}），已跳过便携包: {}", zip_dest.display()),
    }

    // 寻找 Inno Setup 编译器 ISCC
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

    Ok(())
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
        assert_eq!(report.system_bindings, vec!["windows".to_owned()]);
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
    fn unknown_crates_are_reported_as_unclassified() {
        // 没人预料到的 C 依赖必须被报出来，而不是被静默放行
        let report = classify(&names(&["libz-sys", "ring", "serde"]));
        assert!(!report.is_ok(), "未分类依赖必须让审计失败");
        assert_eq!(report.unclassified, vec!["libz-sys".to_owned()]);
        assert_eq!(report.non_rust, vec!["ring".to_owned()]);
    }

    #[test]
    fn known_pure_rust_crates_are_not_reported() {
        let report = classify(&names(&["serde", "tokio", "windows", "ring"]));
        assert!(report.unclassified.is_empty(), "{:?}", report.unclassified);
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
