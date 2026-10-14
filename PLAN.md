# 酷狗概念版桌面客户端 · Rust 实施方案（v2）

> 状态：**实施中**（M0 已完成并通过黄金向量对拍；见文末「实施记录与偏差」）
> v2 变更：① 范围收窄为**仅概念版**，删除全部双平台分支；② 技术栈做**纯 Rust 审计**；③ 目录结构改为**惯用 Rust workspace 布局**
> 基线参考：`D:\YZB\KuGouMusicApi`（JS 实现，关键路径已实测验证）

---

## 1. 摘要

| 项 | 定版 |
|---|---|
| 目标 | 酷狗**概念版（lite）**桌面客户端，单平台 |
| UI | `gpui-kit 0.7.3` + `gpui-pre 0.3.9`（crates.io 精确 pin） |
| 语言边界 | 应用代码 **100% Rust**；唯一非 Rust 工件是 TLS 密码学 crate 内置的 C/asm（见 §3） |
| 结构 | 7 crate workspace + `xtask`，核心层不含 GPUI，可无头测试 |
| 性能杠杆 | ① 复用 GPUI 内置 HTTP 客户端，不自建运行时 ② 音频三段分离、回调零锁零分配 ③ 三级缓存 + 单飞去重 ④ 只解析需要的字段 ⑤ 全列表虚拟化 |

---

## 2. 范围：只做概念版

### 2.1 硬编码的 lite 常量（不再运行时切换）

```rust
// crates/lyra-proto/src/consts.rs
pub const APPID: u32 = 3116;
pub const CLIENTVER: u32 = 11440;
pub const SRAPPID: u32 = 2919;
pub const APIVER: u32 = 20;

pub const SALT_MD5: &str   = "LnT6xpN3khm36zse0QzvmgTZ3waWdRSA"; // android 签名 + signParamsKey + cloudlist appkey
pub const SALT_SIGNKEY: &str = "185672dd44712f60bb1736df5a377e82"; // signKey
pub const SALT_WEB: &str   = "NVPh5oo715z5DIWAeQlhMDsWXXQV4hwt";  // web 签名

pub const RSA_PUBLIC: &str  = include_str!("../keys/lite_public.pem");       // 业务 RSA（无填充）
pub const FP_RSA_PUBLIC: &str = include_str!("../keys/lite_fingerprint.pem"); // 指纹 RSA-OAEP-SHA256

pub const WX_APPID: &str = "wx72b795aca60ad321";
pub const QQ_APPID: &str = "101706348";

pub const PAGE_ID: u64 = 967177915;       // song_url / song_url_auth
pub const P_PAGE_ID: &str = "356753938,823673182,967485191";
pub const PID: u32 = 411;
pub const SEARCH_CLIENTVER: u32 = 201;    // 概念版 Young version_code
```

### 2.2 相比 JS 参考实现删掉的分支

| 位置 | 原双平台逻辑 | 收窄后 |
|---|---|---|
| `signatureAndroidParams` / `signParamsKey` | `isLite ? saltA : saltB` | 单盐值常量 |
| `signKey` | 双盐值 | 单盐值 |
| `cryptoRSAEncrypt` / `rsaEncrypt2` | 双公钥选择 | 单公钥常量 |
| `createRequest` 默认参数 | `isLite ? liteAppid : appid` | 常量 |
| `createCloudRequest` | `CLOUDLIST_CONF{lite, default}` | 单配置 |
| `song_url` / `song_url_auth` | page_id / ppage_id / pid 三处三元表达式 | 常量 |
| `search` / `search_mixed` | `if (isLite) clientver = 201` | 常量 |
| `login_cellphone` / `login_token` / `login_openplat` / `login_qq*` | `isLite ? t1 : 0` 等 | 恒带 t1/t2 |
| `privilege_lite` | `freeListen = isLite && ...` | 恒成立 |
| `import_playlist` / `user_fans` / `album_songs` | appid 切换 | 常量 |
| `home_discover` | 强制 clientver 20809 覆盖 liteClientver | 直接写 20809 |

**净效果**：协议层分支减少约 1/3，`consts.rs` 一个文件即承载全部平台差异，且**每个常量都有实测来源**。

### 2.3 首版不做

MV / 云盘 / 一起听（音乐室）/ 乐库楼层 / `/pc/diantai` 电台 banner（实测 TLS 不通）/ 扫码登录（保留为短信失败时的降级路径，P2）。

---

## 3. 技术栈：纯 Rust 审计

### 3.1 逐项判定

| 依赖 | 用途 | 源码语言 | 判定 |
|---|---|---|---|
| `gpui-pre` 0.3.9 | UI/GPU | Rust（Windows 经 `windows` crate 绑定 Win32/DX） | ✅ |
| `gpui-kit` 0.7.3 | 组件库 | Rust | ✅ |
| `md-5` `sha1` `aes` `cbc` `rsa` `num-bigint` | 全部密码学 | Rust（RustCrypto / pure-Rust num-bigint） | ✅ |
| `base64` `hex` `flate2` | 编解码 | Rust（`flate2` 用默认 `miniz_oxide` 后端） | ✅ |
| `symphonia` 0.5 | 音频解码 | Rust | ✅ |
| `cpal` 0.16 | 音频输出 | Rust（WASAPI 经 `windows` crate） | ✅ |
| `rubato` `ringbuf` | 重采样 / 无锁环形缓冲 | Rust | ✅ |
| `serde` `serde_json` | 序列化 | Rust | ✅ |
| `moka` `redb` `dashmap` `im` `smallvec` | 缓存/存储/容器 | Rust | ✅ |
| `tracing` `thiserror` `anyhow` | 可观测性/错误 | Rust | ✅ |
| `reqwest`(gpui fork) `hyper` `tokio` | HTTP | Rust | ✅（传递依赖，见 3.3） |
| `windows` 0.62 | Win32 绑定 | Rust | ✅ |
| **`ring` 或 `aws-lc-sys`** | **TLS 密码学** | **C + 汇编** | ⚠️ **唯一非 Rust 工件** |

### 3.2 对唯一「非纯 Rust」项的处置

- **来源**：`rustls` 的密码学 provider。GPUI 的 reqwest fork 走 `__rustls-ring`（即 `ring`），`ring` 内含 C 与汇编。
- **Windows 上的实际影响：零额外负担**。Rust 的 `x86_64-pc-windows-msvc` target 本身已要求 MSVC 工具链（`link.exe` + `cl.exe`），`cc` crate 直接复用同一个 VS 安装。**不需要装任何新东西。**
- **不采用** `native-tls` / `default-tls`：会引入 OpenSSL 或 SChannel 的 C 库。已确认 gpui-kit 对 reqwest 的声明是 `default-features = false` + `rustls-tls-native-roots`，**默认就走 rustls 路径**，与我们的目标一致。
- **不采用** `rustls-rustcrypto`（纯 Rust provider）：上游 README 明确写着 *"USE THIS AT YOUR OWN RISK! DO NOT USE THIS IN PRODUCTION"*，且仅覆盖约 70% 的 TLS 套件。**方案明确排除**。
- **落地约定**：`deny.toml` 中禁止 `native-tls`、`openssl`、`openssl-sys`；`xtask audit` 用 `cargo tree -e normal --prefix none` 扫描并输出「非 Rust 工件清单」，CI 里守住。

> 结论：**应用层与全部业务逻辑 100% Rust；构建工具链只有 `cargo`；唯一 C/asm 是 TLS 密码学实现内部，且不引入任何额外安装步骤。** 若你的「完全 Rust」标准包含「依赖树里不允许有任何 C 源码」，那唯一出路是自实现 TLS provider —— 本方案不推荐，风险远大于收益。

### 3.3 HTTP 栈与运行时的真实情况（修正 v1 的错误结论）

v1 我写了「不引入 tokio」，经核实**不准确**，更正如下：

| 事实 | 证据 |
|---|---|
| `gpui-pre` 0.3.9 核心**不含 tokio** | 依赖表：`async-channel` / `async-task` / `futures` / `pollster` / `parking_lot` / `scheduler`，无 tokio |
| `gpui-pre-reqwest-client` 0.3.9 **直接依赖 `tokio ^1`** | crates.io 依赖表 |
| `gpui-pre-reqwest` 0.12.15 是 hyper 系，features 含 `tokio-rustls`、`tokio/io-util`、`tokio/sync` | crates.io features |
| `gpui-pre` 已自带 `image 0.25` / `resvg`+`usvg`+`ttf-parser` / `taffy` / `lyon` / `accesskit` / `windows 0.62` | 依赖表 → **封面解码、SVG 图标、布局、无障碍都不用重复引入** |

**结论与策略**：tokio 会作为**库**进入依赖树（因为 reqwest/hyper 需要它），我们**不自己创建 tokio 运行时**，只使用 GPUI 的 `HttpClient` 抽象。
**M0 必须核实**：`ReqwestClient` 是否内部自建 tokio 运行时与线程。两种结果都可行：
- 若自建 → 架构为「UI 线程(smol) + HTTP 运行时(内部) + 音频线程」，各司其职，无需跨运行时编组 —— 这就是我们要的形态。
- 若不自建 → 需要显式提供一个 tokio 运行时句柄，同样只在 `lyra-app` 一处配置。

**备选（仅在确认需要彻底去 tokio 时评估）**：基于 `gpui-pre-http-client` 的 trait 自实现 Transport，用 `async-io` 驱动 `hyper`。成本高、收益仅为依赖树洁癖，**列为 P2**。

---

## 4. 目录结构（Rust 惯例）

### 4.1 布局

```
D:\YZB\Kugou\
├─ Cargo.toml                 # [workspace] + [workspace.package] + [workspace.dependencies] + [workspace.lints]
├─ Cargo.lock                 # 二进制项目 → 入库
├─ rust-toolchain.toml        # 固定 toolchain，保证可复现
├─ rustfmt.toml
├─ clippy.toml
├─ deny.toml                  # cargo-deny：许可证 / 安全公告 / 依赖禁令
├─ .cargo/config.toml         # 别名（xtask 等）
├─ PLAN.md
├─ README.md
├─ .github/workflows/ci.yml
├─ assets/
│  ├─ icons/                  # SVG（GPUI 自带 resvg，可直接渲染）
│  └─ fonts/
└─ crates/
   ├─ proto/                  # lyra-proto  —— 协议原语（纯函数，零 IO / 零 async / 零 GPUI）
   │  ├─ Cargo.toml
   │  ├─ keys/{lite_public.pem, lite_fingerprint.pem}
   │  ├─ src/{lib,sign,cipher,identity,fingerprint,consts,error}.rs
   │  ├─ tests/vectors.rs     # 黄金向量：与 JS 实现逐字节对拍
   │  └─ benches/{sign,cipher}.rs
   ├─ api/                    # lyra-api    —— 传输 + 端点 + 缓存（依赖 proto，不依赖 GPUI）
   │  ├─ src/
   │  │  ├─ lib.rs
   │  │  ├─ transport/{mod.rs, mock.rs}      # trait Transport + 测试实现
   │  │  ├─ cache.rs  single_flight.rs  retry.rs  codec.rs
   │  │  └─ endpoint/{session,library,playlist,audio,lyrics,flow,catalog,privilege}.rs
   │  └─ tests/              # 用 MockTransport 的集成测试
   ├─ store/                  # lyra-store  —— redb 持久化（schema / 迁移 / 仓库）
   ├─ audio/                  # lyra-audio  —— 音频引擎（自有 OS 线程，不依赖 GPUI）
   ├─ app/                    # lyra-app    —— 状态机 / 用例 / GPUI Entity  ★唯一接触 GPUI 运行时处
   ├─ ui/                     # lyra-ui     —— 视图与组件（gpui-kit）
   ├─ desktop/                # lyra        —— 二进制 crate：薄 main.rs + 资源嵌入
   └─ xtask/                  # xtask       —— 构建自动化（跨平台，替代 shell 脚本）
```

### 4.2 为什么这样分（每条都有具体理由）

| 决定 | 理由 |
|---|---|
| `proto` 零 IO、零 async | 签名/指纹是**最容易出错**的部分，必须能用纯函数单测对拍 JS，不需要任何 mock |
| `api` 不依赖 GPUI | 通过 `trait Transport` 反转依赖 → 可无头跑全部端点测试；`MockTransport` 支持离线回归 |
| `app` 是唯一接触 GPUI 运行时的库 | tokio/reqwest/executor 配置收敛在一处，将来换框架只动这里和 `ui` |
| `audio` 独立于 `app` | 音频是 OS 线程 + 实时约束，与 GPUI 生命周期无关，必须能脱离 UI 测试 |
| `desktop` 是**薄**二进制 | 惯例：`src/main.rs` 只做装配与启动，逻辑全在 lib，便于集成测试 |
| `xtask` 用 Rust 而非 shell | Rust 项目惯例，跨平台一致，不依赖 PowerShell/bash |
| 不设 `utils` / `common` / `helpers` crate | Rust 社区明确反模式；共享代码放回它所属的领域 crate |
| 7 个库 crate | 分片依据是「依赖方向」与「测试边界」，不是「架构分层」。再细分只会拉长编译、增加样板 |

### 4.3 workspace 骨架

```toml
# 根 Cargo.toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version      = "0.1.0"
edition      = "2024"
rust-version = "1.99"
license      = "MIT"
authors      = ["…"]
repository   = "…"

[workspace.dependencies]
lyra-proto = { path = "crates/proto" }
lyra-api   = { path = "crates/api" }
lyra-store = { path = "crates/store" }
lyra-audio = { path = "crates/audio" }
lyra-app   = { path = "crates/app" }
lyra-ui    = { path = "crates/ui" }

# UI（整组同步升级，= 为精确 pin，勿单独 bump）
gpui-kit       = { version = "0.7.3", features = ["component", "base"] }
gpui           = { package = "gpui-pre",          version = "=0.3.9" }
gpui_platform  = { package = "gpui-pre-platform", version = "=0.3.9" }
gpui_macros    = { package = "gpui-pre-macros",   version = "=0.3.9" }
reqwest_client = { package = "gpui-pre-reqwest-client", version = "=0.3.9" }

# 协议原语
md-5 = "0.10"   sha1 = "0.10"    aes = "0.8"    cbc = "0.1"
rsa  = "0.9"    num-bigint = "0.4"  base64 = "0.22"  hex = "0.4"
flate2 = "1"    rand = "0.8"

# 音频
symphonia = { version = "0.5", features = ["flac","mp3","vorbis","ogg","aac","isomp4","pcm"] }
cpal    = "0.16"
rubato  = "0.15"
ringbuf = "0.4"

# 数据 / 通用
serde = { version = "1", features = ["derive"] }   serde_json = "1"
moka = "0.12"   redb = "2"   dashmap = "6"   im = "15"   smallvec = "1"
thiserror = "2" anyhow = "1" tracing = "0.1"

[workspace.lints.rust]
unsafe_code    = "forbid"
missing_docs   = "warn"
unreachable_pub = "warn"

[workspace.lints.clippy]
all      = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
unwrap_used  = "warn"
expect_used  = "warn"
todo         = "deny"
dbg_macro    = "deny"

[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
opt-level = 3
```

```toml
# 根 rust-toolchain.toml
[toolchain]
channel    = "1.99.0"
components = ["rustfmt", "clippy", "rust-src"]
```

**每个 crate 的惯例**：`Cargo.toml` 里 `[lints] workspace = true`；库 crate `<crate>/src/lib.rs` 首行 `#![forbid(unsafe_code)]`；集成测试放 `<crate>/tests/`，单测内联 `#[cfg(test)]`；性能敏感模块配 `benches/`（criterion）。

### 4.4 测试策略（对应 Rust 惯例）

| 层级 | 位置 | 内容 |
|---|---|---|
| 单元 | `crates/*/src/**` 内联 | 纯函数、解析、状态机转移 |
| 契约 | `crates/proto/tests/vectors.rs` | **黄金向量**：签名/AES/RSA/指纹的输出与 JS 实现逐字节一致 |
| 集成 | `crates/api/tests/` | 用 `MockTransport` 跑完整端点流程，离线可重复 |
| 冒烟 | `xtask smoke` | 打真实上游，验证端点存活（手动/定时，不进 PR CI） |
| 基准 | `crates/proto/benches/`、`crates/audio/benches/` | 签名吞吐、JSON 解析、解码速度，配阈值回归 |

---

## 5. 协议层规格（lite 专用）

> 以下全部为**本机实测确认**，非推测。与 §2.1 的常量表配套使用。

### 5.1 请求签名

```rust
// android（默认）
md5(format!("{SALT_MD5}{sorted_kv}{body}{SALT_MD5}"))
//   sorted_kv：参数按 key 字典序，"k=v" 拼接；值为对象先 JSON 序列化

// web
md5(format!("{SALT_WEB}{sorted_kv}{body}{SALT_WEB}"))

// register（设备注册）
md5(format!("1014{sorted_values}1014"))   // 只取值、排序、拼接
```

**性能**：`sorted_kv` 是热路径 —— 用 `SmallVec<[(&str,&str);16]>` 就地排序；用 `md5::Md5` **流式三段 `update`**（盐 / 参数 / body），不构造完整中间字符串。

### 5.2 密码学原语

| 原语 | 用途 | 关键参数 |
|---|---|---|
| MD5 → hex32 | 全部签名、密钥派生 | — |
| AES-CBC-PKCS7 | 通用加解密 | key = `md5(temp)[0..32]` **按 UTF-8 当字节用** → **实为 AES-256**；iv = `key[16..32]` |
| AES-128-CBC | 指纹 EDT | iv = `"kugousecurity123"`（固定） |
| RSA 无填充 | 业务公钥加密 | `modPow`，输出 hex 左补零至密钥字节长 |
| RSA PKCS1v1.5 | cloudlist `p` 字段 | 输出 hex **大写** |
| RSA-OAEP-SHA256 | 指纹 SID | MGF1-SHA256，输出 Base64 |
| XOR + zlib inflate | KRC 歌词 | 跳过前 4 字节 → 16 字节密钥循环异或 → 解压 |

⚠️ **最易错点**：`cryptoAesEncrypt` 的 key 是 `md5(temp)` 的 **32 个 hex 字符**，作为 UTF-8 使用 → **32 字节 = AES-256**，不是 AES-128。逐字节对齐时务必确认。

**KRC XOR 密钥**：`[64,71,97,119,94,50,116,71,81,54,49,45,206,210,110,105]`

### 5.3 设备身份（首启生成一次，永久固定）

| 字段 | 生成 |
|---|---|
| `guid` | UUID v4 |
| `mid` | `md5(guid)` 的 hex 视为 16 进制大整数 → **十进制字符串** |
| `mac` / `dev` | `02:00:00:00:00:00` / 10 位大写字母数字 |
| `webgl` | 随机 uint64 十进制串 |
| `dfid` | `/register/dev` 返回 |

**随机化会导致风控命中**，必须持久化。

### 5.4 行为指纹（风控单点）

上游判定可疑时返回 `ssa-code` 响应头 + 业务码，后续请求必须带 `sid`/`edt`，否则实测得到 `20028 本次请求需要验证`。

```
AES key = md5(random_string(16))[0..16]          // 16 hex 字符 = 16 字节
明文    = "mid={mid};userid={uid};dfid={dfid};webgl={h};webdriver=0;ts={ms};data={events}"
EDT     = Base64( AES-128-CBC(明文, key, iv="kugousecurity123", PKCS7) )
SID     = Base64( RSA-OAEP-SHA256(key, FP_RSA_PUBLIC) )
```

`events` 为 `:` 分隔的时序记录：

| type | 含义 | 格式 |
|---|---|---|
| 3 | 鼠标/触摸 | `3,Δt,子索引,x,y` |
| 5 | 滚动 | `5,Δt,事件索引` |
| 6 | 窗口 | `6,Δt,事件索引,w,h` |
| 哨兵 | 段结束 | `Δt` 换成接近 `0xFFFFFFFF` 的随机值 |

轨迹必须用**三阶贝塞尔 + 递减抖动**（3px → 0.5px），`Δt` 取 8–50ms 随机，每 12 帧插一次滚动。**不可简化为直线** —— 这是能否稳定调通的单点。

### 5.5 双加密通道（cloudlist，歌单增删走这条）

```
aes  = random(6 字符 [0-9a-zA-Z])
key  = md5(aes)[0..16],  iv = md5(aes)[16..32]
p    = hex(RSA-PKCS1v1.5({"aes":aes,"uid":uid,"token":token})).to_upper()
签名  = md5(APPID + SALT_MD5 + CLIENTVER + clienttime).to_lower()   // appkey 即 SALT_MD5
body = AES-CBC(JSON, key, iv, PKCS7)
POST gateway.kugou.com  +  x-router: cloudlist.service.kugou.com
响应 AES-CBC 解密 → JSON；失败则按 UTF-8 原样解析
```

### 5.6 公共请求头与默认参数

```
query  : dfid, mid, uuid, appid, clientver, clienttime  [+ token, userid 登录后]
header : dfid, clienttime, mid,
         kg-rc: 1, kg-thash: 5d816a0, kg-rec: 1, kg-rf: B9EDA08A64250DEFFBCADDEE00F8F25F,
         User-Agent: Android15-1070-11083-46-0-DiscoveryDRADProtocol-wifi
```

### 5.7 错误码表（实测）

| 码 | 含义 | 处置 |
|---|---|---|
| `status=0` / `error_code≠0` | 业务失败 | → `Error::Upstream` |
| `20002` | userid 类型错 / 未登录 | 见 §6.2 特例 |
| `20010` | 参数缺失（歌词未带 id/accesskey） | 补参数 |
| `20028` | 需要验证 | 触发 `Fingerprint` 重试一次 |
| `130012` | 听歌上报失败 | 忽略，不阻断 |
| `131001` | 每日领取失败 | 见 §6.7 |
| `297002` | 已领取过 | **幂等成功**，不报错 |

---

## 6. 功能实现逻辑

### 6.1 登录（短信）

```
POST http://login.user.kugou.com/v7/send_mobile_code   { businessid:5, mobile, plat:3 }
POST login.user.kugou.com                             （恒定带 t1/t2，见 §2.2）
```
`t1 = AES("|{ts}", T1_KEY, T1_IV)`、`t2 = AES({key:"|{ts}"}, T2_KEY, T2_IV)`。

**实测**：验证码接口工作正常（`count` 9→8），但 155 号段短信延迟数分钟。**产品必须有重发倒计时 + 失败降级扫码**。

### 6.2 推荐流（`Flow` 统一抽象）

实测四条流结构不同，不是参数差异：

| 流 | 端点 | 结构 | 游标 |
|---|---|---|---|
| 每日推荐 | `/everyday/recommend` | `data.song_list[]` × 30，带 `creation_date` | 无 |
| 推荐歌曲 | `/recommend/songs` | **与上者完全同源**（data 键、元素键、前 3 个 hash 全等） | 无 |
| 个人电台 | `/personal/fm` | `data.song_list[]` × 5 + `sync_point`/`fresh_mode`/`mode` | **有** |
| 首页发现 | `/home/discover` | `data.items[]` + `item_type`/`alg_path` | 有 |

```rust
enum FlowKind { Daily, Personal, Discover, Card(CardId) }

trait Flow {
    fn kind(&self) -> FlowKind;
    fn hydrate(&mut self) -> Task<Page>;
    fn next_page(&mut self) -> Option<Task<Page>>;   // 无游标的流返回 None
}
```
差异通过 `Page::meta` 暴露，UI 用同一套列表组件渲染。

⚠️ **上游缺陷（必须绕过）**：`/home/discover` 用 GET 必失败 ——
```
{"errcode":20002,"errmsg":"...cannot unmarshal string into Go struct field ReqRecommendBody.userid of type int64"}
```
原因：实现把 cookie 里的 `userid`（字符串）直接塞进请求体，而上游要求 `int64`；`?userid=` 也无效（query 仍是字符串）。
**唯一解**：`POST` + JSON body（数字）。Rust 端**强制 POST + 数字 userid**，不暴露该参数。

### 6.3 电台（`StationCatalog`）

两段式结构，与推荐流是**不同产品概念**：
```
GET /fm/class     → class_list[14] { classid, classname, class_count, fmlist[] }
GET /fm/recommend → [44] { fmid, fmname, classid, imgurl, banner, heat, description }
GET /fm/songs?fmid=&fmtype=&offset=&size=  → 20 首/页
```
UI 语义差异：**电台 = 网格卡片选频道**；**推荐 = 列表直接出歌**。

### 6.4 歌单

```
列表  POST /cloudlist.service/v7/get_all_list  { userid, token, total_ver:979, type:2 }
      → data.info[] { listid, name, count, type(0=自建/我喜欢, 1=收藏), global_collection_id }
详情  GET  /pubsongs/v2/get_other_list_file_nofilt?global_collection_id=<gcid>
      → data.songs[] { name, hash, album_id, audio_id, mixsongid, fileid, timelen }
加歌  POST /cloudlist.service/v6/add_song   { listid, data:[{name,hash,album_id,mixsongid}] }
删歌  POST /v4/delete_songs                 { listid, data:[{fileid}] }
```
**实测**：「我喜欢」= `listid=2`；增删实测 `count 102→103→102` 净零。

**缓存陷阱**：上游对成功响应缓存 2 分钟（key = host + 完整 URL）。`lyra-api` 给 `Idempotency::SideEffect` 的端点**自动追加毫秒扰动参数**，不依赖调用方记得。

### 6.5 播放（`AudioSource`）

```
GET /song/url/auth?hash=&quality=&album_audio_id=&album_id=
```
**实测结论**：
- `/song/url`（非 auth）在 lite 下**不返回 url**（`status:3`）→ **只用 auth 版**
- 返回 URL 形如 `http://fs.youthandroid2.kugou.com/.../ap3116_us2043841403_..._quflac_pa967177915...flac`，**含签名且有时效**（路径内含分钟级时间戳）→ 缓存必须带过期
- 下载验证：`HTTP 206`，文件头 `66 4C 61 43` = `fLaC`，**确为真无损**
- ⚠️ `/privilege/lite` 的 `obtainable` **不可信**（领 VIP 后仍返回 `false` 但 flac 可播）→ 可用性以「auth 是否返回 url」为准，`capabilities()` 仅供 UI 显示

**音质 × 权益（实测）**：

| 音质 | 需要 |
|---|---|
| 128 | 概念版免费听（`best_free_quality = 128`） |
| 320 / flac / high | **概念版 VIP**（`require_vip_type = "concept"`） |
| viper_tape / viper_clear / viper_atmos | **超级 VIP**，概念版 VIP 与 tvip **均不可播** |

### 6.6 歌词

必须两步（直接传 hash 会 `20010`）：
```
GET /search/lyric?hash=&duration=&album_audio_id=   → candidates[0] { id, accesskey }
GET lyrics.kugou.com/download?id=&accesskey=&fmt=krc&decode=true → content(Base64)
KRC：跳过 4 字节 → 16 字节密钥循环 XOR → zlib inflate → UTF-8
```
解析为 `Vec<LyricLine{ time, text }>` 供逐行滚动。实测 46 行正常。

### 6.7 会员（`Privilege`）

```
状态  /user/vip/detail        → data.busi_vip[]   ← 畅听 VIP 只在这里
领取  /youth/day/vip/upgrade  → { recharge_hours: 24 }
历史  /youth/month/vip/record → list[] { day, vip_type, receive_vip }
```
**实测关键结论**：
1. **`/youth/day/vip/upgrade` 单独调用即领到 24 小时畅听 VIP**，不依赖 `/youth/day/vip`（后者报 `131001` 但结果不受影响）
2. 成功判据 = `busi_vip` 出现 `{ busi_type:"concept", product_type:"svip", vip_begin_time, vip_end_time }`；**顶层 `vip_type`/`is_vip` 对概念版恒为 0**，用错字段会误判「没领到」
3. **每天限一次**，重复返回 `297002` → 按幂等成功处理
4. `/youth/listen/song` 返回 `130012`，当前不可用 → **不作为领取前置**

---

## 7. 性能设计

### 7.1 网络层

| 措施 | 目标 |
|---|---|
| 全应用单一 `HttpClient` 实例，连接池共享 | 复用 TCP/TLS |
| 启用 `http2`，对 `gateway.kugou.com` 走 h2 | 省重复握手 |
| 启动时对 3 个高频 host 发 HEAD 预热 | 首屏 −100ms |
| 单飞：`dashmap<Key, OnceCell<..>>` 合并同 key 并发 | 重复请求归零 |
| 分级 TTL：推荐流 5min / 歌单 30s / 歌曲 URL 60s / 图片 7d | 二次进入瞬时 |
| `SideEffect` 端点自动去缓存扰动 | 消除缓存误判 |
| 两档优先级队列（用户可见 > 预取） | 交互优先 |

### 7.2 音频管线（延迟关键路径）

```
[CDN] ──bytes──▶ [BlobCache 磁盘] ──▶ [Decoder 线程] ──f32──▶ [SPSC ring ~200ms] ──▶ [cpal 回调]
 async              <hash>.<ext>        std::thread              ringbuf           WASAPI shared
```

| 设计点 | 做法 |
|---|---|
| 边下边播 + 落盘 | Range 流式写入磁盘缓存并喂解码器，重播/seek 命中本地 |
| 三段分离 | 抓取(async) / 解码(OS 线程) / 输出(实时线程) 互不阻塞 |
| 无锁环形缓冲 | `ringbuf` SPSC ≈200ms，回调零锁零分配 |
| 回调精简 | 只 `pop` + 格式转换，无分支 |
| 预取下一首 | 播到 60% 启动下一首抓取+首帧解码 → **切歌 0 间隙** |
| 重采样 | `rubato` 仅在设备率 ≠ 源率时启用，跑在解码线程 |
| 线程优先级 | 解码线程提升；回调走实时档 |

**延迟预算**：缓存命中 **<100ms**；冷启动 **<400ms**；切歌 **0ms**。

**不支持 WASAPI 独占**：`cpal` 的 exclusive 模式上游 PR 未合并。需要 bit-perfect 时用 `windows` crate 直接接 `IAudioClient`（P2，不影响首版）。

### 7.3 缓存分层

| 层 | 载体 | 内容 | 淘汰 |
|---|---|---|---|
| L1 | `moka` (TinyLFU) | API 响应 | TTL + 容量 |
| L2 | `redb` | 歌单元数据、历史、设置、登录态 | 手动/版本迁移 |
| L3 | 文件系统 | 音频 blob（内容寻址）、封面 | LRU，容量可配 |
| L4 | GPUI 图片缓存 | 已解码封面纹理 | 随窗口 |

封面按 CDN 的 `{size}` 占位符请求目标分辨率（如 `/stdmusic/240/`），**不下载原图再缩放**。

### 7.4 UI

- 全列表走 gpui-kit `VirtualList`（支持不等高行）：实测歌单 102 首、电台 44 项、分类 348 项全部虚拟化
- 状态变更后只 `cx.notify()` 受影响的 Entity，禁止整窗重绘
- `render` 内不构造 `String`（用 `SharedString` / 预格式化）
- 队列用 `im::Vector`，避免每帧 clone 整表

### 7.5 解析与线程预算

- 只声明需要的字段（`#[serde(default)]`），避免 `Value` 中转的双重分配
- 大响应（`/fm/class` 实测 803 KB）在后台线程裁剪成精简 `Vec<Station>` 再进 UI
- 上游字段类型不稳（`status` 时 int 时 string）→ 宽容反序列化，契约收敛在 `lyra-api::codec` 一处
- 线程预算：UI 1 + GPUI background = CPU 核数 + 音频解码 1 + cpal 1；**不自建 tokio 运行时**

---

## 8. 里程碑

| 阶段 | 交付 | 验收 |
|---|---|---|
| **M0** 协议对拍 | `lyra-proto` + `xtask` | 签名/`dfid`/`sid`/`edt` 与 JS **逐字节一致**；同时核实 `ReqwestClient` 的 tokio 运行时形态（§3.3） |
| **M1** 链路打通 | `lyra-api` + `lyra` | CLI：登录 → 歌单 → 取 URL → 下载 flac，文件头 `fLaC` |
| **M2** 播放可用 | `lyra-audio` | 冷启动出声 <400ms，切歌无间隙，seek 正确 |
| **M3** 核心 UI | `lyra-ui` | 播放/暂停/切歌/进度/音量/歌单列表，60fps |
| **M4** 内容页 | 每日推荐 + 个人电台 + 电台目录 | 四条流 + 电台两段式全通 |
| **M5** 账号功能 | 登录页 / 歌单增删 / VIP 领取 | 领取后 `busi_vip` 出现 `concept/svip`；增删净零 |
| **M6** 打磨 | 歌词滚动、封面缓存、设置、打包 | 安装包产出，首屏 <1s |

**M0 是成败关键**：任何字节偏差都会导致后续全部返工。完成后立即跑一次全量端点冒烟。

---

## 9. 风险

| # | 风险 | 影响 | 对策 |
|---|---|---|---|
| R1 | 上游风控（SID/EDT） | **高** | 必须真实贝塞尔轨迹，禁止简化；全局限速 ≤1 req/s 突发，批量操作加间隔 |
| R2 | 上游接口变更 | **高** | 端点收敛在 `lyra-api`，宽容解析 + 错误码表，`xtask smoke` 保留冒烟 |
| R3 | `gpui-pre` 精确 pin | 中 | workspace 集中声明；整组同步 bump，禁止单独升级 |
| R4 | Zed 降低 GPUI 投入 | 中 | 备选 `gpui-ce`；`ui`/`app` 两层隔离框架 API |
| R5 | 短信延迟/丢失 | 中 | 重发倒计时 + 扫码降级（实测 155 号段延迟数分钟） |
| R6 | 版权与合规 | **高** | 个人研究用途；不分发音频内容；UI 明示非官方；不内置账号 |
| R7 | 账号安全 | **高** | token 走系统钥匙串（Windows DPAPI），不落日志、不落明文 |
| R8 | `/privilege/lite` 的 `obtainable` 不可信 | 中 | 以实际取 URL 为准，UI 不硬性置灰 |
| R9 | cpal 无 WASAPI 独占 | 低 | 首版 shared；P2 自接 WASAPI |

---

## 10. 决策点

| # | 决策 | 建议 | 状态 |
|---|---|---|---|
| D1 | 项目代号（crate 前缀） | `lyra-`（天琴座），二进制 `lyra` | ⬜ 待定 |
| D2 | GPUI 路线 | `gpui-pre 0.3.9` + `gpui-kit 0.7.3` | ⬜ 待定 |
| D3 | HTTP | 复用 GPUI `HttpClient`（接受 tokio 作为传递依赖） | ⬜ 待定 |
| D4 | 音频解码 | `symphonia`（纯 Rust，覆盖 FLAC/MP3/OGG-Vorbis） | ⬜ 待定 |
| D5 | 缓存后端 | `moka` + `redb` + 文件 blob | ⬜ 待定 |
| D6 | token 存储 | Windows DPAPI（`windows` crate 直接调，零额外依赖） | ⬜ 待定 |
| D7 | 「完全 Rust」的验收线 | 应用层 100% Rust + 仅 `cargo` 构建；接受 TLS 内部 C/asm | ⬜ **需你明确** |

---

## 附录：实测通过的端点

| 域 | 端点 | 结果 |
|---|---|---|
| 会话 | `/captcha/sent` | ✅ count 9→8 |
| 会话 | `/login/cellphone` | ✅ userid `2043841403` |
| 库 | `/user/playlist` | ✅ 4 歌单 |
| 库 | `/playlist/track/all` | ✅ 30 / 102 首 |
| 库 | `/playlist/tracks/add` · `del` | ✅ 102→103→102 净零 |
| 播放 | `/privilege/lite` | ✅ 音质权限矩阵 |
| 播放 | `/song/url/auth` | ✅ flac 真无损（`fLaC` + 206） |
| 播放 | `/song/url` | ❌ lite 下无 url |
| 歌词 | `/search/lyric` · `/lyric` | ✅ 46 行 |
| 推荐 | `/everyday/recommend` · `/recommend/songs` | ✅ 30 首（同源） |
| 推荐 | `/personal/fm` | ✅ 5 首 + 游标 |
| 推荐 | `/top/card/youth` | ✅ 28 首 |
| 推荐 | `/home/discover` | ⚠️ 仅 POST |
| 电台 | `/fm/class` · `/fm/recommend` · `/fm/songs` | ✅ 14 / 44 / 20 |
| 会员 | `/youth/day/vip/upgrade` | ✅ 24h svip 真实生效 |
| 会员 | `/user/vip/detail` · `/youth/month/vip/record` | ✅ |
| 会员 | `/youth/day/vip` · `/youth/listen/song` | ⚠️ 131001 / 130012 |

**测试环境**：`D:\YZB\KuGouMusicApi` @ `ba3645f`，`platform=lite`；账号 `userid=2043841403`；Windows / rustc 1.99.0。

---

# 实施记录与偏差（实施期追加）

本节的每一条都是**实测/编译期确认**的，不是推测。凡与上文表格冲突，以本节为准。

## A. 计划中写错、实施时必须改的

| # | 位置 | 计划里写的 | 实际是 | 影响 |
|---|---|---|---|---|
| A1 | §4.3 | `gpui-kit = { features = ["component","base"] }` | **gpui-kit 0.7.3 的 features 是空的**，`component` 与 `assets` 就是它的 `default`；`base` 不是 feature | 照抄会直接解析失败 |
| A2 | §6.3 | 电台用 `GET /fm/class` / `/fm/recommend` / `/fm/songs` | **全是 POST**：`/v1/class_fm_song`、`/v1/rcmd_list`、`/v1/app_song_list_offset`，且都带 `x-router: fm.service.kugou.com` | 端点写错全线 404 |
| A3 | §6.4 | 歌单列表 `POST /cloudlist.service/v7/get_all_list` | `POST /v7/get_all_list` + `x-router: cloudlist.service.kugou.com`（路由头，不是路径） | 同上 |
| A4 | §6.4 | 歌单详情 `?global_collection_id=<gcid>` | 路径是 `/pubsongs/v2/get_other_list_file_nofilt`，且需要 `begin_idx`/`mode`/`personal_switch`/`extend_fields` | 少参数取不到数据 |
| A5 | §6.5 | `GET /song/url/auth` | `GET /tracker/v5/url`，带 `encryptKey`（signKey）与 19 个固定参数 | 端点名错 |
| A6 | §6.7 | `/user/vip/detail` / `/youth/day/vip/upgrade` | `GET kugouvip.kugou.com/v1/get_union_vip?busi_type=concept`、`POST /youth/v1/listen_song/upgrade_vip_reward`（`{kugouid, ad_type:1}`） | 端点名错 |
| A7 | §2.1/§5.2 | RSA 无填充「输出 hex 左补零至密钥字节长」 | 输出补零是对的，但**输入是右补零**：参考实现是 `padded.set(buffer)`（从下标 0 写入），数值上等于 `m << 8k`，**不等于 `m^e mod n`** | 改成左补会让登录直接失败；已用 node-forge 逐字节确认并按参考实现实现 |
| A8 | §4.3 | `rust-toolchain.toml` 固定 `1.99.0` | 本机默认工具链已是 1.99.0。写死版本号会让 rustup 再下一份同版本工具链 | 改为 `channel = "stable"`，MSRV 由 `rust-version` 守 |
| A9 | §4.3 | `moka = "0.12"` | 0.12 必须显式选 `sync`/`future`，默认两者都没开 | 不写会 `compile_error!` |
| A10 | §10 D6 | token 用 Windows DPAPI（`windows` crate 直接调） | 与 §4.3 的 `unsafe_code = "forbid"` 直接冲突（DPAPI 是 unsafe FFI） | 改用 `keyring`（Windows 凭据管理器），安全等价、无 unsafe、零额外安装 |

## B. 与计划不同、但是主动决策的

| # | 决策 | 理由 |
|---|---|---|
| B1 | **只用 `gpui-kit`，不再单独依赖 `gpui-pre` / `gpui-pre-platform` / `gpui-pre-macros` / `gpui-pre-reqwest-client`** | `gpui-kit` 是官方门面：`use gpui_kit::*` 就是 GPUI，各层走 `gpui_kit::base/platform/component/assets`。上游注释明确写着「用户不应需要知道是哪个 GPUI crate 实现它」。这样 §4.3 的「整组同步 bump」风险从源头消失 |
| B2 | **HTTP 传输统一用 `reqwest`，不做 GPUI `HttpClient` 适配层** | ① 音频必须**流式落盘**，我们的 `Transport::execute_streaming` 已实现，GPUI 客户端要另写一条；② `BlobFetcher` 必须 spawn 下载任务，本来就依赖 tokio 运行时；③ 按 §3.3 自己的发现，gpui-pre 的 reqwest fork 内部也是 tokio。**结论修正：tokio 作为库进入依赖树，我们显式建一个多线程运行时并全进程复用**，这是「一个运行时」而不是「两个」 |
| B3 | 单飞、L1 缓存都在 `lyra-api` 内部，L2（redb）只管设备/登录态/设置/历史 | §7.3 本来就把 L2 的职责写成「歌单元数据、历史、设置、登录态」，API 响应缓存留在 L1 |
| B4 | `lyra-api` **不依赖** `lyra-store` | 保持 `api` 可无头测试、无持久化副作用；两者在 `lyra-app` 汇合 |
| B5 | `Transport` 增加 `execute_streaming`（默认退化实现） | 边下边播的前提；`MockTransport` 等实现无需改动 |
| B6 | **暂缓 §4.4 的 `benches/`（criterion）层** | 没有稳定基线的微基准在 CI 里只会变成噪声；真正的性能验收（冷启动 <400ms、切歌 0 间隙）用 M2 的端到端场景测，比函数级吞吐有意义。签名吞吐如需回归，再加 criterion |
| B7 | **不建自有 `assets/` 目录** | `gpui-kit` 的 `assets` feature（默认开）已经提供图标与字体资源，重复造一份只会增加维护面。真需要自定义图标时再建 |

## C. 实测确认（原计划结论正确，但这次拿到了证据）

| 结论 | 证据 |
|---|---|
| **`ring` 是依赖树里唯一的非 Rust 工件** | `cargo xtask audit`：451 个 crate，非 Rust 清单只有 `ring`。§3.2 成立 |
| `schannel` 会出现在 Windows 依赖树里，但**不是 TLS 后端** | 由 `rustls-native-certs` 引入，仅用于**枚举** Windows 受信任根证书；纯 Rust FFI，不编译 C。审计里因此不把它当违规，而是断言 `native-tls` 必须缺席 |
| `freetype-sys`（C 库）**不在 Windows 构建图里** | `cargo fetch` 会下载全平台依赖造成误判；`cargo tree`（host target）里没有它。font-kit 的 freetype 后端只在 Linux/macOS 生效 |
| 行为指纹可以逐字节对拍 | 用 mulberry32 同时替换 Rust 与 JS 的随机源后，**整条轨迹串（3000+ 字符）、EDT、t1/t2、cloudlist body、RSA 无填充输出、KRC 解码、mid 十进制** 全部逐字节一致 |
| 参考实现的 `generate_simulate.js` 有一处隐藏缺陷 | 它只 import 了 `randomString`，**没有** import `generateWebGLHash`，所以 `generateSimulate` 不传 `webglHash` 会抛 `ReferenceError`。因此 Rust 侧把 `webgl` 定为必填 |
| `cryptoAesEncrypt` 是 AES-**256** | key 是 `md5(temp)` 的 32 个 hex 字符按 UTF-8 当字节用 = 32 字节。§5.2 的警示正确 |
| **`gpui-kit` 的 `test-support` feature 会激活实验性的 `gpui-fast` 后端，并让 Cargo 直接无法解析依赖** | `gpui-kit-assets` / `gpui-base` / `gpui-component` 把 `gpui-fast*` 声明成 **dev-dependency**，于是解析图里同时出现两套后端：`gpui-fast-platform` 精确 pin `cocoa =0.26.0` / `unicode-properties =0.1.3`，而 `gpui-pre-platform` 锁的是 `0.26.1` / `0.1.4`，报 `failed to select a version for cocoa`。**结论：不要开 `test-support`**（也不用 GPUI 的 UI 测试框架，纯函数单测用普通 `#[test]` 即可） |
| **缓存键必须排除"每次都变"的参数** | 实施期发现：`clienttime`（秒级）进了缓存键，于是任何缓存条目**下一秒就失效** —— §7.1 的分级 TTL（推荐流 5 分钟、歌单 30 秒）形同虚设。现已把 `clienttime` / `signature` / `_lyra_t` 从键里剔除，并加了"时间流逝后键不变"的测试。这个 bug 是靠一个偶发失败的缓存命中测试暴露出来的 |
| 上游响应的列表位置高度不稳定 | 同一份数据会在 `data.song_list` / `data.songs` / `data.info` / `data.items` 之间变，标量（`sync_point`/`has_next`）也会被 `data` 包一层。故 `lyra-api::codec` 用**有界 BFS 按候选键名查找**，而不是写死路径 |

## D. 交付状态

| 里程碑 | 状态 |
|---|---|
| **M0** 协议对拍 | ✅ 完成。`lyra-proto` 契约测试 17 条黄金向量 + 23 条单测全绿 |
| **M1** 链路打通 | ✅ 完成。`lyra-proto` / `lyra-api`（96 测）/ `lyra-store`（6 测）/ `lyra` CLI：登录 → 歌单 → 取 URL → `ensure()` 落盘并校验文件头（`fLaC` / `OggS`）。设备身份首启生成后跨进程稳定 |
| **M2** 播放可用 | ✅ 引擎完成（`lyra-audio`，11 测：解码/seek/44.1k→48k 重采样/环形缓冲 FIFO 与溢出）；`BlobFetcher` 已把「增长中文件视图」接进解码线程，支持边下边播与预取 |
| **M3** 核心 UI | ✅ 播放/暂停/切歌/进度/音量/歌单列表，长列表走 `v_virtual_list`（支持不等高行） |
| **M4** 内容页 | ✅ 每日推荐 / 个人电台 / 电台两段式（分类 + 频道）；另有卡片推荐与首页发现 |
| **M5** 账号功能 | ✅ 登录页（含重发倒计时）/ 歌单增删 / VIP 领取与状态 |
| **M6** 打磨 | ✅ 歌词逐行高亮、封面 `{size}` 占位符替换、设置页（音质/缓存/音量/登出）、窗口标题与最小尺寸 |

### 测试分布（全部离线，不需要账号）

| 层 | 位置 | 条数 | 内容 |
|---|---|---|---|
| 契约（黄金向量） | `crates/proto/tests/vectors.rs` | 17 | 签名 ×4、AES ×4、RSA 无填充、KRC、SHA1、mid、**指纹轨迹逐字节**、EDT 逐字节 |
| 单元 | `crates/proto/src` | 23 | 参数渲染/排序、AES/RSA/KRC 往返、设备身份、贝塞尔轨迹 |
| 端点契约 | `crates/api/src/endpoint/*` | 约 40 | 每个端点的**请求参数与请求头**都对参考实现断言，响应解析宽容性 |
| 基础设施 | `crates/api/src/{cache,codec,client,blob,single_flight,spec}` | 约 56 | TTL 过期、前缀失效、单飞合并、宽容解码、有界 BFS、磁盘缓存与 LRU、断点续传视图、签名与 `signKey` 顺序 |
| 持久化 | `crates/store/src` | 6 | 设备/设置/blob TTL/历史去重/凭据往返 |
| 音频 | `crates/audio/src` | 11 | 解码、seek、44.1k→48k 重采样、环形缓冲 FIFO 与溢出 |
| 用例集成 | `crates/app/tests/end_to_end.rs` | 5 | **登录 → 歌单 → 歌曲 → 取地址**全链路；音质回落；登出清凭据；上游报错不 panic；设备身份跨进程稳定 |
| 界面 | `crates/ui/src` | 6 | 歌词行定位、时长/字节格式化、音质标签、搜索判空 |
| 构建 | `crates/xtask/src` | 4 | 依赖分类与禁用判定、schannel 语义 |

合计 **191 条**，`cargo test --workspace` 全绿（连跑 3 次无抖动）。
GUI 实测可启动：窗口句柄非零、标题 `lyra · 酷狗概念版`、stderr 为空。

---

# 代码审查与修复记录

对全量工作树做了一次两轴审查（Standards 轴按 crate 分 4 片并行；Spec 轴按要求跳过），
父代理逐条复验后**驳回 2 条**（有参考实现取证），其余全部修复。修复后的状态：
`cargo fmt --all --check` 干净、`cargo clippy --workspace --all-targets` **零告警**、
`cargo test --workspace` **191 条全绿**、GUI 仍可启动。

## 修掉的真 bug（按影响排序）

| # | 问题 | 影响 | 修法 |
|---|---|---|---|
| 1 | `codec::check_business` 用 `pick_i64(..) == 0` 判定，而**键缺失也返回 0** | 歌词服务（`/download`、`/v1/search`）的响应没有 `status` 字段，参考实现 `lyric.js:25` 只读 `res.body?.content` —— 原实现会把**成功的歌词响应整片判成业务失败**，歌词功能在真实上游上直接失效。离线测试测不出（桩里都写了 `"status":1`） | 改成 `pick(..).is_some_and(\|s\| as_i64(s) == 0)`；补 `missing_status_is_not_a_failure` 与**整条两步歌词链路**的离线回归测试 |
| 2 | `vip_token` 明文落 redb | 与风险 R7「不落明文」冲突。`token` 走了凭据管理器，`vip_token` 漏了 | 两个凭据分开存（`{userid}` / `{userid}#vip`），redb 里都置空；测试**直接去数据库文件里搜明文**断言搜不到 |
| 3 | `Ended` 后立刻置 `Idle` | 回调随即停止消费，ring 里最多 ~200ms 尾音被丢弃（听感上最后一个字被切），且残留在 ring 里、下次 `play` 开头重放 | 先等 `occupied_len()` 归零再 emit/置 Idle（上限 2s 防止设备异常时卡死解码线程） |
| 4 | seek 静默失败 + 不清 ring | 失败被吞掉，UI 位置与实际声音悄悄不一致；旧位置的残响会先播一段 | 失败上报 `Event::Failed`；清 ring 走 `Shared` 的 flush 信号（`ringbuf` 的 `Producer` 没有 `pop`/`clear`，只有 Consumer 能清，而 Consumer 在 cpal 回调里） |
| 5 | `single_flight` leader 被 cancel | 等待者**永久挂起**，后续同 key 调用也一起挂 | `LeaderGuard` 的 Drop 里清登记 + 给等待者明确失败；补「取消 leader 后 follower 不挂起」测试 |
| 6 | 缓存键含 `clienttime`（秒级） | 任何缓存条目**下一秒就失效**，§7.1 的分级 TTL 形同虚设 | 剔除 `clienttime`/`signature`/`_lyra_t`，并加「时间流逝后键不变」测试 |
| 7 | 歌词视图每帧每行 `SharedString::from(&str)` | 违反 §7.4「render 内不构造 String」，且被每 180ms 的位置轮询重渲染 | 在 `set_lines` 里预拼 `Arc<Vec<SharedString>>`，render 只 clone 引用 |
| 8 | 磁盘缓存做断点续传 | 一旦 CDN 忽略 `Range` 头照样返回 200 + 完整体，追加写会把同一段字节写两遍 → **能播但时间轴错乱的音频** | 去掉续传，每次都从 0 重下（内容寻址的半个文件本来也没用） |

## 按 smell 基线清理的

- **Duplicated Code**：7 个端点测试模块各抄一份 `session()/api()`（其中 5 份逐字相同、各 20 行）→ 收敛到 `lyra-api/src/testing.rs`；store 的 `begin_write → open_table → commit` 样板 8 处 → 收敛到 `Store::write`（漏 commit 在 redb 里是**静默丢写**，值得只留一处）；播放条与歌词页重复的「曲目变了才重建标题」→ 提到数据拥有者 `PlayerModel::current_labels`。
- **Repeated Switches + Primitive Obsession**：`privilege.rs` 对 `quality: &str` 的三处 match → 收敛成 `Quality` 枚举，三个方法**穷举**变体（不留 `_ =>` 兜底），新增音质会直接编译不过。
- **失实依赖声明**：17 处未使用依赖（ui 3 / desktop 5 / app 4 / xtask 2 / api 1 / store 2）。`cargo` 默认不报这类问题，只能人工清。
- **注释与代码不符**：`params.rs` 的 `sorted_key_value_eq` 注释称「`k=v` 保序、不排序」，实际既无 `=` 又**有**排序，且全仓无调用者 → 删除；`app/src/lib.rs` 称「唯一接触运行时的库」而 `ui` 自建 tokio 运行时 → 改成陈述事实；`ui/src/lib.rs` 称「`Sender` 不是 `Sync`」—— **实测是假的**（编译期断言 + 阴性对照证实 `mpsc::Sender` 自 1.72 起是 `Sync`），顺带去掉多余的 `Mutex`。
- **其它**：`xtask` 的 `&dfid[..len.min(8)]` 按字节切 String（非 ASCII 会 panic）→ 改按字符；`cli.rs` 为读 4 字节文件头而读整份音频（flac 可达 40MB）→ 只读 4 字节；`xtask audit` 里「schannel 由 rustls-native-certs 引入」原本只是**口头断言** → 改成真的跑 `cargo tree -i schannel` 核实，否则报错；radio 的目录错误混进歌曲区 → 拆成 `catalog_error`/`songs_error`；login 的「登录成功」文案被紧接着的 `reset()` 清掉（永远看不到）→ 只保留失败提示。

## 驳回的审查意见（有取证）

| 意见 | 驳回理由 |
|---|---|
| `fingerprint.rs` 注释「webgl 缺失会抛错」与参考相反，参考会回退 `generateWebGLHash()` | 参考实现 `generate_simulate.js` **只** `require('./util')` 的 `randomString`，第 304 行调用的 `generateWebGLHash` 根本不在作用域 → `ReferenceError`。有实测证据：跑向量脚本时正是被这个错误打断的。注释是对的 |
| 两个「6 位会话串」生成器应统一 | 参考实现**真的有两套**：`request.js:299`（createCloudRequest）用 62 字符表**含大写**，`crypto.js:274`（playlistAesEncrypt）用 `randomString(6).toLowerCase()` 即仅 `[0-9a-z]`。两处都按各自参考实现写的，统一会**破坏字节级保真** |

## 仍未验证的

- 上游是否真有无 `status` 的响应 —— 需要真实账号打一次歌词接口才能最终确认（旁证是参考实现对该端点不检查 `status`）。
- audio 的尾音/seek 修复需要**有声卡实机**听自然结尾才能确认听感。

### 验证命令

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p xtask -- audit
cargo run -p lyra          # 无参数 → 图形界面
```
