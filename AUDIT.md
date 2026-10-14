# Lyra 全仓代码审计报告

- **审计对象：** 整个 workspace（8 个 crate，约 1 万行 Rust，197 个测试）@ `8485f8d`
- **方法：** 按 crate 分工逐行读完 `proto` / `api` / `store` / `audio` / `app` / `ui` / `desktop` / `xtask`、测试、CI、安装脚本与工具配置；每个结论都回到调用方复核，死代码逐项在整仓（含测试、脚本、字符串引用）grep 确认。
- **假定的负载：** 单个桌面进程长期运行（数小时），用户连续点歌/切歌、偶尔拖进度条；一个 tokio 运行时里并发的十几次请求；音频持续流式落盘。不是多用户服务端，但"本机长期积累"和"用户点一下必须正确"是硬要求。
- **状态：** 只读审计，本文件不附带任何代码改动。

---

## 摘要

底层质量不错：签名/加密有黄金向量钉死、单飞去重无丢唤醒、凭据与库分离、环形缓冲消费无锁无分配，这些都经得起看。问题集中在两处：

1. **`api` 层的缓存键不含请求体** —— 直接导致"点第二首歌播第一首"（第 1 条）和"删歌/收藏静默失效"（第 2 条），这是最要紧的一处。
2. **长期运行与故障路径没人管** —— 磁盘缓存永不淘汰（第 4 条）、下载停滞永久卡死（第 5 条）、并发下载互相截断（第 6 条）。

另外有约 400 行死代码（第 14 条）与 CI 门禁自相矛盾（第 9 条）。

各组的严重度按"正确 → 安全 → 负载 → 测试 → 速度 → 精简"排序。

---

## 必须修

### 1. 点第二首歌实际播放第一首（音质权益串数据）

`crates/api/src/client.rs:146-149`、`crates/api/src/transport/mod.rs:117-140`、`crates/api/src/endpoint/privilege.rs:190`

- **这是什么：** 每个请求的缓存键和"单飞"键只由 `路径 + query` 拼成，**不含请求体**。
- **问题：** `/v2/get_res_privilege/lite` 的歌曲身份全在 body（`resource[].hash`、`album_id`），query 只有会话级默认参数。30 秒内先播 A 再播 B，B 的权益请求直接命中 A 的缓存；`service.rs:295-305` 又优先用 `rights.quality(chosen).hash`（A 的 ogg 变体 hash）去取播放地址 → **点 B 播出来的是 A**，连"最佳音质/缺失音质/歌手名"也全是 A 的。同因还导致：换电台/翻页 10 分钟内拿回第一个电台的第一页（`catalog.rs:96-131`）、"下一首"5 分钟内永远同一页同一游标（`flow.rs:96-105`）。
- **修复：** 在 `Request::cache_key()`（`transport/mod.rs:117`）里并入 body 摘要，先剔除每次变化的字段（`clienttime/clienttime_ms/visit_time/key/p/last_time/_lyra_t`）再哈希；一处改完，上面全部端点同时修好。
- **不修的后果：** 最常见的"列表里连点两首歌"就会播错歌，且用户没有任何提示。

### 2. 两次删歌/收藏被合并，第二次静默失效

`crates/api/src/endpoint/playlist.rs:74-81`、`crates/api/src/client.rs:157-171`

- **这是什么：** `delete_songs` 的身份全在 body，query 只有默认参数；而"单飞"去重对所有请求都生效（包括写操作）。
- **问题：** 快速删两首不同的歌（UI 每次点击各起一个任务），两个请求键完全相同 → 第二个请求根本不发出，却返回第一个的 `Ok`：界面弹"已移除"，刷新后歌还在。`add_songs` 同理（窗口限同一秒，因为 `last_time=now_secs()`）。写操作被静默丢弃是最难排查的一类 bug。
- **修复：** 同第 1 条的修法（body 进键）。
- **不修的后果：** 用户以为删掉了/收藏了，实际没生效，且没有任何错误提示。

### 3. 快速切换歌单会让列表错位，并可能删掉别的歌

`crates/ui/src/views/playlists.rs:96-113`、`118-125`

- **这是什么：** 选歌单立即写 `listid/heading`，但响应回来时 `on_tracks` 不校验这条响应属于哪个歌单。
- **问题：** 先点 A（慢）再点 B（快）→ B 的数据先到、A 的后到，于是 `songs` 是 A 的歌而 `listid` 是 B。此时点任意一行的"移除"，发出的是 `listid=B + A 那首歌的 fileid` → 若该 fileid 在 B 里存在就会**误删 B 里另一首歌**；`search.rs`、`radio.rs` 同样无归属校验，只是显示错列表。
- **修复：** `Msg::Tracks` 携带发起时的 `global_id`，`on_tracks` 与当前 `listid` 不一致就丢弃（其余视图同样带一个自增序号）。
- **不修的后果：** 误删用户歌单里的歌，属于数据破坏。

### 4. 磁盘缓存永不淘汰，设置里的"缓存上限"是假的

`crates/api/src/blob.rs:215-234`、`crates/app/src/service.rs:70`

- **这是什么：** `BlobCache` 有 `max_bytes`（UI 可设 512MiB–4GiB），也有 `evict_lru`/`total_bytes`，但**没有任何生产调用者**（全仓只有测试调用）。
- **问题：** 每首听过的歌（FLAC 约 40MB）永久留在磁盘上，一路涨到把盘写满；`settings.max_cache_bytes` 改了也没用，必须重启才读到新值。
- **修复：** 下载成功 `rename` 之后（`blob.rs:278-285`）判 `total_bytes() > max_bytes` 则 `evict_lru(max_bytes)`；删掉设置项也行（那就别给用户假控制）。
- **不修的后果：** 长期使用后磁盘被静默吃满，用户唯一的办法是手动删目录。

### 5. 下载一旦停滞，播放永久卡死，关窗也退不掉进程

`crates/api/src/blob.rs:64-70`、`341-345`、`crates/audio/src/engine.rs:58-64`

- **这是什么：** 边下边播的读取端用 `while downloaded < wanted && !complete` 死等；下载请求显式 `timeout = None`。
- **问题：** CDN 只连上不发数据（不 RST）时，这个循环永不退出 → 解码线程永久阻塞，后面的 `Play` 命令排在同一线程的队列里也永远轮不到：**播放功能彻底失效直到重启**。此时关窗，`Drop for EngineInner` 的 `send(Shutdown)+join()` 等的是这个卡死的线程 → 进程变僵尸只能任务管理器杀（release 是 `panic="abort"`，也逃不掉）。
- **修复：** 给 `wait_for` 加"无新字节超时"（如 30 秒无进展返回 `io::Error`，顺带把永久 Loading 变成 `Event::Failed`）；`Drop` 里不要无条件 join。
- **不修的后果：** 用户遇到弱网就"点歌没声音且再点也没反应"，退出时程序还关不掉。

### 6. 同一首歌并发打开会互相截断，损坏文件还会被当成品永久缓存

`crates/api/src/blob.rs:259-275`、`336-340`、`274-286`

- **这是什么：** `open_stream` 每次都 `truncate(true)` 同一个 `{key}.{ext}.part`，没有按 key 的在途去重。
- **问题：** 第一路下载到 N 字节时第二路把它截成 0，第一路的文件游标仍在 N 处 → `[0,N)` 成了空洞，而读取端按 `progress.downloaded` 判定"可读"，把 N 字节零当音频送进解码器（爆音/静音）；先完成者 `rename` 成功后，后者的 `rename` 必然失败并被写成 `progress.error`，正在播的那一路读完数据后把该错误当 IO 错误抛出 → 播放中断。重复点同一首歌即可触发。
- **修复：** 按 key 记录在途 `Arc<Progress>`（`DashMap`），已存在就复用只读视图 + 空 `Download`，只有第一个调用者起下载。
- **不修的后果：** 偶尔出现爆音/播放中断，且损坏文件被 rename 成正式文件后**永久缓存**，以后每次播这首歌都坏。

### 7. 切歌时新歌开头混入上一首最多 200ms 的残留

`crates/audio/src/engine.rs:380-398`、`400-407`、`crates/audio/src/output.rs:37-45`

- **这是什么：** 输出回调只在 `Playing` 时消费环形缓冲；`start()`/`stop()` 从不请求清空缓冲。
- **问题：** 播放 A 时 ring 是满的（200ms 容量），点 B → 状态变 `Loading`，回调只写静音但**不消费**，A 的样本留在 ring 里；切成 `Playing` 后先播完 A 的残音再播 B，stop 后再 play 更明显。`seek` 走了 `request_flush()`，切歌没走。
- **修复：** `start()` 的 Ok 分支与 `stop()` 各加一行 `self.ctx.shared.request_flush();`。
- **不修的后果：** 每次切歌都有一声"前一首的尾巴"，被当成玄学问题。

### 8. 令牌会跟着报错文案进界面、终端和日志

`crates/api/src/http.rs:62`（流式分支同处）、`crates/ui/src/views/tracks.rs:86`、`crates/ui/src/shell.rs:227`、`crates/desktop/src/main.rs:37`

- **这是什么：** 传输错误用 `reqwest::Error::to_string()` 直接进 `Error::Transport`；所有登录态请求的 query 里都有 `token/userid/signature`（`spec.rs:317-323`）。
- **问题：** 已核对本地依赖源码（reqwest 0.12.28 `src/error.rs:267`）：`Display` 会输出 `error sending request for url (https://…?token=<账号令牌>&…)`。于是断网点"收藏/播放"时，**令牌明文出现在 UI 气泡里**，CLI 打印到终端，`tracing` 写进日志——截图、录屏、贴 issue 即泄露。
- **修复：** `http.rs` 两个 `map_err` 改成 `Error::Transport(e.without_url().to_string())`（或只保留 host），UI 侧只显示"操作失败，请检查网络"。
- **不修的后果：** 账号令牌（等同账号）可能被用户自己无意公开。

### 9. CI 的门禁自相矛盾，`cargo deny check` 必失败

`deny.toml:23-28`、`.github/workflows/ci.yml:50-52`、`Cargo.lock`（schannel 在树里）、`crates/xtask/src/main.rs:118-131`

- **这是什么：** CI 跑 `cargo deny check`；`deny.toml` 把 `schannel` 列进 `[bans] deny`。
- **问题：** `Cargo.lock` 里确实有 `schannel`（由 `rustls-native-certs` 在 Windows 上引入，因为 reqwest 开了 `rustls-tls-native-roots`）。它命中禁用名单 → 该步骤永远非零退出，PR 永远红；而 `xtask audit` 里还专门断言"schannel 只用于枚举系统根证书、不算违规"。两处必有一个是错的。
- **修复：** 删 `deny.toml` 里 `{ name = "schannel" }` 这一行（xtask 已用 `cargo tree -i schannel` 钉死来源）。
- **不修的后果：** 质量门禁形同虚设——反正一直是红的，没人再看它。

---

## 应当修

### 10. 播放失败时界面完全没反应

`crates/ui/src/shell.rs:166-172`、`crates/ui/src/views/player.rs:31,48`

- **这是什么：** 取播放地址失败、解码失败都只写进 `PlayerModel::status`。
- **问题：** 全仓 grep 该字段**只有写入点、没有任何渲染点**：点一首版权受限的歌，用户看到的是播放条毫无变化，以为程序卡了（`Msg::AudioFailed` 同样静默）。
- **修复：** 删掉 `status` 字段，两处改成 `Msg::Toast(format!("播放失败: {error}"))`（净删代码）。
- **不修的后果：** 最容易出现的交互故障对用户不可见，只能被当成"这软件坏了"。

### 11. 音量设置重启后失效，而且界面上有三份互相矛盾的音量

`crates/app/src/service.rs:92-105`、`crates/audio/src/state.rs:43-45`、`crates/ui/src/views/settings.rs:44,208-222`、`crates/ui/src/views/player.rs:190-212`

- **这是什么：** 引擎音量初值硬编码 `1.0`，`attach_audio`/`with_transport` 都不把 `settings.volume` 写进引擎；设置页滑块只在构造时读一次，播放条滑块每 180ms 跟随引擎。
- **问题：** 把音量调到 20% 保存 → 重启后**以 100% 出声**；设置页显示 0.8、播放条显示 1.0，用户随便一动滑块就把 1.0 覆盖回磁盘。
- **修复：** `attach_audio` 成功后一行 `engine.set_volume(self.store.settings()?.volume.clamp(0.0,1.0))`；设置页音量行删掉（播放条已是权威 UI）。
- **不修的后果：** 用户的音量偏好每次重启都丢，还可能被静音惊吓或者半夜满音量。

### 12. 下载成品不做长度/内容校验，失败还残留 .part

`crates/api/src/blob.rs:325-346`、`274-286`、`200-202`

- **这是什么：** 只要 `execute_streaming` 返回 `Ok` 就 `rename` 成正式文件；"已缓存"只判断文件是否存在；失败分支不删 `.part`。
- **问题：** 音频 CDN 是 **http**，校园网/热点门户拦截会返回 200 + HTML 登录页 → 这个 HTML 被当音频**永久写死**，之后每次播这首歌都用它，永不重下；无 `Content-Length` 时对端提前断开，短文件被当完整文件缓存（尾部静音/提前结束）。失败一次就整首白下（下次仍是 `truncate` 重来），残留 `.part` 也无人回收。
- **修复：** 把期望长度交给 sink（或结束时比对 `response.content_length()`），`written != expected` 就报错且不 rename；错误分支加 `let _ = std::fs::remove_file(&part_path);`。
- **不修的后果：** 坏文件被永久缓存，用户遇到"这首歌永远播不了/永远爆音"，重装才能修。

### 13. 电台的错误提示接反了（拆分字段时赋值对调）

`crates/ui/src/views/radio.rs:88-105`（消费点 `185`、`253`）

- **这是什么：** 拆成 `catalog_error`（目录）与 `songs_error`（歌曲）两个字段，但赋值接反了。
- **问题：** 频道歌曲加载失败 → 错误写进 `catalog_error`，而歌曲区渲染的是 `songs_error` → 用户看到"选择一个频道后在此显示歌曲"，看不到任何失败；目录加载失败 → 错误被塞进 `songs_error` 显示在歌曲区，频道网格永远停在"正在获取频道…"。`on_songs` 成功时清的还是 `catalog_error`。
- **修复：** 两组赋值对调（更好的做法是合并成一个 `Option<(Area, SharedString)>`）。
- **不修的后果：** 网络一抖就显示成"没有内容"，用户以为歌单是空的。

### 14. 死代码与重复实现：约 400 行可以删，3 个依赖可以去掉

（逐项都在整仓 grep 过，含测试、脚本、字符串引用）

- **整块没接线的功能：**
  - `crates/store`：`push_history/history/clear_history` + `HISTORY` 表 + `HistoryEntry`（`lib.rs:281,293,314`、`:24`、`types.rs:53`）——**"播放历史"整个功能是空壳**，UI 无入口（约 -80 行）。
  - `crates/audio`：`Engine::pause/resume/stop`（`engine.rs:74,79,89`）+ `Command::{Pause,Resume,Stop}` 三个分支不可达（UI 只用 `toggle`）；`Fetcher::prefetch`（`types.rs:41` + `fetcher.rs:35`）**从未被调用**，"切歌预取"没接线（约 -40 行）。
  - `crates/api`：`Api::cloud`（`client.rs:127-143`）全仓无调用者（`cloud.rs` 只在测试里用）；`search::other`/`SearchKind`（`search.rs:17-40,73`）、`month_vip_record`（`privilege.rs:407`）、`user_info`（`session.rs:283`）、`library::app_id`（`library.rs:105`）、`find_array_or_empty`（`codec.rs:199`）、`freshness_of`（`app/src/service.rs:440`）、`fetcher::shared`（`app/src/fetcher.rs:55`）。
  - 仅测试或完全无人使用的工具项：`VecSink`（`transport/mod.rs:206`）、`Response::json`（`:187`）、`Error::Json` 及 `From<serde_json::Error>`（`error.rs:91,106`，从无构造点）、`is_idempotent_success`/`is_ignorable`（`error.rs:41,47`，而 `claim_daily_vip` 自己另写了一份 `error.code()==Some(Code::AlreadyClaimed)`）、`remove_blob`/`drop_expired`/`clear_ns`/`open_temp`（`store/lib.rs:218,229,256,71`）、`TtlCache::len/is_empty`、`BlobCache::is_cached`、`sha256_hex`（`cipher.rs:38`）、`cache_ns`（`flow.rs:34`）、`Freshness::IMAGE`（`cache.rs:27`）、`sign_mode`（`spec.rs:248`）、`is_playable`（`model.rs:119`）、`Params: FromIterator`（`params.rs:164`）、`aes_decrypt_with_key_iv`（`cipher.rs:193`）。
- **重复实现（必须一起改，否则会静默分叉）：** `aes_encrypt_with_key_iv_derived`（`cipher.rs:165`）与 `aes_encrypt_derived_with`（`:153`）逐字相同，只差返回类型——删前者、把 `tests/vectors.rs:85,98` 改成用后者。
- **死配置：** `tools/vectors.json` 全仓无消费者（`vectors.rs` 的值内联硬编码），且其中 `fp_key` 由 `slice(0,0)` 生成、恒为空串（`gen-vectors.js:87`）→ JS 侧重新生成后 Rust 断言仍拿旧字面值跑绿，两份"黄金"能静默分叉；`Cargo.toml` 的 `[workspace.dependencies]` 里 `im`、`smallvec` **没有任何 crate 使用**，`sha1` 只被待删的 `sha1_hex` 用。
- **修复：** 按上面清单删；如果要保留播放历史，就把它接进 UI（`.history()` 一行调用），别留半成品。
- **不修的后果：** 每次改动都要读、要测这些永远不会被执行的代码，而且"看起来有这功能"会误导后来人。

### 15. `xtask` 的自检有一半是摆设，打包会打错版本号

`crates/xtask/src/main.rs:54-58,133-141`、`:99-105`、`:246-317`

- **这是什么：** 依赖审计里 `schannel_is_only_a_cert_reader()` 那条断言**永远不可能失败**（它要求 native-tls 在禁用名单里，而上一条 `report.is_ok()` 已经先 return 了）；`package()` 里版本号硬编码 `0.1.0`，`tar` 失败不告警。
- **问题：** 版本 bump 到 0.2.0 后 `cargo xtask package` 仍产出 `Lyra-v0.1.0-…zip` 和 `AppVersion=0.1.0`，与 `release.yml` 用 `$VERSION` 的逻辑各漂移一份；便携包生成失败时还会打印"✅ 安装包生成完成"。另外"非 Rust 工件"只打印不拦截（`README` 却宣称"验证不存在未允许的 C 依赖"）。
- **修复：** 删那条不可达断言与其单测；版本从 `Cargo.toml`/`cargo metadata` 读；`tar` 失败至少打一行 warn。
- **不修的后果：** 发布出去的安装包版本号对不上 tag，自检给人虚假的安全感。

---

## 可选

### 16. 歌单/搜索没有分页，第 101 首之后无法到达

`crates/ui/src/views/playlists.rs:104`、`crates/app/src/service.rs:271`

- **这是什么：** `tracks()` 固定取第 1 页 100 首，搜索取 30 首，界面没有任何"加载更多"。
- **问题：** `model::Page::cursor` 在 `crates/ui` 里零引用（grep 只匹配到 `cursor_pointer`）。超过 100 首的歌单，第 101 首起用户永远点不到，而页头"N 首"会让人以为这就是全部。
- **修复：** 列表末尾按键 `cursor` 追加"加载更多"，或至少标明"仅显示前 100 首"。
- **不修的后果：** 大歌单的用户以为歌丢了。

### 17. 入参校验只按字节长度，且 `mask` 会 panic

`crates/app/src/service.rs:172`、`crates/ui/src/views/login.rs:60-90`、`crates/app/src/cli.rs:291-297`

- **这是什么：** 手机号校验是 `mobile.trim().len() < 11`（**字节**数），`submit()` 完全不校验手机号；`mask()` 用 `&token[..3]` 做字节切片。
- **问题：** 11 个汉字（33 字节）能通过校验直接发上游请求；`mask` 一旦遇到含多字节字符的 token 就会 panic，而 release 是 `panic="abort"` → 整个 CLI 进程直接退出。
- **修复：** 抽一个 `is_mobile()`（11 位纯数字，用 `chars().count()` + `is_ascii_digit`）给两处共用；`mask` 改用 `chars().take(3)`。
- **不修的后果：** 用户拿到看不懂的上游英文错误；极小概率下 CLI 直接崩。

### 18. PKCS7 容错解密从不校验填充，和代码自己的契约矛盾

`crates/proto/src/cipher.rs:96,100-109`、`crates/api/src/codec.rs:213-226`、`crates/api/src/endpoint/session.rs:122-137`

- **这是什么：** 解密用 `NoPadding`，然后 `unpad_pkcs7_tolerant` 只看末字节数值就截断，从不校验填充字节。
- **问题：** 任何 16 字节对齐的**非密文**输入都会"解密成功"并返回乱码（`NoPadding` 只要求长度是 16 的倍数）。于是 `decode_session_body` 的"解密失败 ⇒ 上游返回的是明文"这条兜底在错误体长度恰好 16 对齐时失效（约 1/16），真实业务错误码被乱码顶掉；`parse_login` 的 `_ => result.token = text` 更会把乱码当 token **落盘到凭据管理器**，表现为"登录成功但什么都请求不了"。
- **修复：** `unpad_pkcs7_tolerant` 改成严格校验（末 n 字节全等于 n 才截断，否则 `Err`），返回 `Result`。
- **不修的后果：** 偶发、无法复现的乱码错误与假登录态。

### 19. 单声道设备下会丢掉右声道

`crates/audio/src/ring.rs:44-57`

- **这是什么：** 声道数不匹配时按 `input[base + ch % src]` 取样本。
- **问题：** 输出设备是单声道（部分 USB 耳麦）而音源是立体声时，`dst=1` 只会取第 0 声道，右声道的内容整体丢弃（现有测试把 `[1,2,3,4] → [1,3]` 钉成了"预期"）。
- **修复：** `dst == 1 && src > 1` 时按声道求均值，测试期望改成 `[1.5,3.5]`。
- **不修的后果：** 这类设备上乐器/人声会缺一半。

### 20. 解压与指纹原语缺入参上限

`crates/proto/src/cipher.rs:303-315`、`:292-298`、`crates/proto/src/fingerprint.rs:117,182-185`

- **这是什么：** KRC 用 `read_to_end` 无上限解压上游 Base64 内容；`fingerprint_edt` 不校验 `key16` 长度（传 32 字节会**静默升级成 AES-256**，产出的 EDT 服务端永远解不开，风控重试死循环）；`edt_events`/`bezier_path` 是 pub API 但负数/零入参会 panic 或产出全 NaN 轨迹。
- **修复：** `.take(8 * 1024 * 1024)` 限流；`fingerprint_edt` 开头 `if key16.len() != 16 { return Err(...) }`；`bezier_path` 内 `let n = n.max(1)`。
- **不修的后果：** 一份精心构造的歌词能让进程 OOM（release 是 abort）；原语的错误用法没有任何提示。

*另有 4 条更小的未列入（`PlayerBar` 180ms 轮询与 `AppShell` 50ms 排空循环的固定空转、`play_index` 在 UI 线程同步读 redb、`format_bytes` 的边界、`clip/mask` 的半脱敏），都不影响正确性。*

---

## 结论

- **Verdict：** 底层质量不错，但第 1 条会让最常见的操作给出错误结果（播错歌），第 2/3 条会静默丢掉用户的写操作甚至误删歌曲，第 4/5/6 条是长期运行下必然出现的磁盘与播放故障。建议先修 1、2、3、5、6（都在 `api` 层，改动集中），再顺手删第 14 条的约 400 行。
- **Lean：** 可行的是 **-约 400 行、-3 个依赖声明（`im`、`smallvec`、`sha1`）**。其中"播放历史"整块、`Engine::pause/resume/stop`、`Fetcher::prefetch`、`Api::cloud`、`tools/vectors.json` 是净删除。
- **未核对：** 本次没有编译或运行任何东西（未跑 `cargo test/clippy/fmt/deny`、未启动 GUI 或音频）。因此：
  - 第 9 条"CI 必红"是按配置与 `Cargo.lock` 推出的结论，请实跑一次 `cargo deny check` 确认；
  - 第 5 条的"关窗卡住"依赖 GPUI 退出时确实 drop 掉 `Engine`；
  - `PLAN.md`（55KB 设计文档）、`crates/proto/keys/*.pem` 的实际模数、`Cargo.lock` 全量依赖未审；
  - `decoder.rs` 里 `is_seekable()` 恒为 true 而 `byte_len()` 恒为 None 对 symphonia 精确 seek 的影响（可能导致未缓存 mp3 拖进度条失败）需要拿一首未缓存的 mp3 实测一次。
