# 架构说明

这份文档记录代码里真正生效的约束；`docs/solution.md` 是设计与取舍的原始讨论（设置项的取舍见 `docs/config.md`），两者冲突时以实现为准，
并且应当在同一个 PR 里更新本文。

## 分层

```
apps/desktop ── 只做三件事：建窗口、把命令转发给 App、把 App 的事件转发给 webview
apps/cli     ── 只做两件事：参数解析、把 App 的结果格式化成人读/JSON
crates/ipc   ── 线上契约：DTO + Problem + 事件名 + 命令名常量
crates/app   ── 唯一的编排层：会话状态机、串行操作、设置保存、订阅刷新、测速、导入
      │
      ├── crates/config    SessionPlan → sing-box / Xray / mihomo 配置
      ├── crates/domain    协议模型、分享链接、指纹、合并计划、订阅嗅探
      ├── crates/store     SQLite 仓储（节点/订阅/规则/流量/会话记忆）
      ├── crates/engine    内核进程、配置自检、Clash API、TCP 延迟、HTTP 抓取
      └── crates/platform  网络设置、端口、LaunchAgent、路径
```

依赖方向只能向下：`ipc` 不依赖 `app`，`config` 不读数据库，`store` 不认识 HTTP。
UI 只通过 `ui/src/lib/api.ts` 与命令层通信，页面里不出现 `invoke`。

`apps/desktop` 有自己的 `[workspace]` 表、不属于根 workspace：Tauri 的依赖树很大，
把它隔离出去之后 `cargo test --workspace` 在没有桌面工具链的机器上仍然可跑（CI 另一个 job 构建它）。

## 会话状态机

```
Disconnected ──connect──► Connecting ──ok──► Connected ──disconnect──► Disconnecting ──► Disconnected
                              │                  ▲                       
                              └──err──► Failed ──retry───────────────────┘
```

- `Connected` 的前提是**内核端口已就绪且接管成功**（`takeoverApplied == true`）；
  `ConnectionState::headline()` 会在 `phase == connected && !takeover_applied` 时输出失败文案，
  前端也因此永远不会在接管失败时看到「已连接」。
- 所有会改变会话的操作（connect / disconnect / switch / set_mode / set_policy / retry）都在
  `App::op_lock`（`tokio::sync::Mutex`）内串行执行；等待期间 UI 用 `busy` 禁用按钮，
  避免出现「点了两次断开」这种把系统代理恢复两次的竞态。
- 失败路径统一走 `cleanup_after_failure`：停内核 → 恢复系统代理快照 → 清空接管标记 → 抛出原始 Problem。
  原始错误不会被后续清理错误覆盖，清理失败只写日志与 warnings。

## connect 的完整顺序

1. `choose_core(profile, settings.core_override)`：显式指定优先，但必须在内核能力表里；
   没有可用内核直接返回 `error.unsupported_protocol`，而不是启动后失败。
2. 内核可执行文件必须存在（`data/cores/<name>` 或 PATH），否则 `error.core_missing` 并带安装指引。
3. **端口前置检查**：入站端口被占用 → `error.port_in_use`，能拿到占用者 PID 时写进 detail；
   控制端口（10812，TUN 时 10813）与 Xray 的额外 HTTP 端口同样检查。绝不静默换端口。
4. 接入方式分四种：系统代理、PAC（本机 10811 提供脚本，按域名绕行）、手动端口、TUN。
   TUN 需要特权 helper：helper 不存在时直接返回 `error.permission_denied` 并说明替代方案。
5. 读取启用中的自定义规则，组装 `SessionPlan`，`config::emit` 产出配置 JSON。
6. `write_atomic` 写 `run/<core>-config.json`（临时文件 + fsync + rename + 目录 fsync），保证内核永远读不到半个文件。
7. `check_config` 用内核自己的校验命令（`sing-box check` / `xray -test` / `mihomo -t`）过一遍；
   校验失败把内核输出的前几行拼进 `error.config_invalid`，**不启动**。
8. `KernelProcess::launch` + `wait_ready(inbound_port, 20s)`：轮询进程存活与端口可连，
   超时带上日志尾部；启动失败时补 `error.core_failed`。
9. `SystemProxy::apply`：先 `capture` 每个网络服务的当前状态（含 auto proxy URL），逐服务写入后把快照
   落盘到 `run/takeover.json`；任一服务写入失败就用刚捕获的快照回滚。PAC 与手动代理互斥：应用 PAC 时先关掉
   manual 的三个槽，应用 manual 时先关掉 auto url；恢复时两者都按快照还原。失败 → 停内核 → 返回
   `error.takeover_failed`。
10. 记录会话记忆（上次节点、上次模式、最近使用时间），广播 `session://changed`。

断开是反向流程：按快照恢复代理（只恢复自己改过的字段）→ 删除快照 → `SIGTERM` 内核（5s 宽限后 kill）→
状态回到 `Disconnected`。应用异常退出后下次启动 `recover_takeover()` 会读快照并把代理恢复回去。

## 选核与协议边界

`crates/config/src/capability.rs` 是唯一的选核入口，规则按「显式指定 → 协议/传输硬约束 → sing-box 兜底」执行：

- VLESS + `encryption=mlkem768x25519plus.*`（Xray 25.x 的 VLESS Encryption）→ 只能 Xray；
  sing-box 不认识该字段，仍会启动失败。实测链路：Reality + `xtls-rprx-vision` + 0-RTT。
- VLESS + xhttp / mKCP / QUIC → Xray；其余 VLESS/REALITY 默认 sing-box，可显式切 Xray。
- Hysteria2 / TUIC / AnyTLS / WireGuard / Shadowsocks 2022 → sing-box；naive 与 Chain 明确拒绝并说明原因。

Cargo 无法在未装 geoip/geosite 的目录里启动 Xray：`xray -test` 会报 `failed to open geoip.dat`，
应用会把该输出翻译成「规则集、证书或内核路径可能没准备好」，提示把 `geoip.dat`/`geosite.dat`
与内核放同一目录，而不是丢一段原始栈。

## 端口约定

| 用途 | 值 |
| --- | --- |
| 本地混合入站（socks+http） | `settings.inbound.mixedPort`，0 表示 10808 |
| Xray 的 HTTP 入站 | 入站端口 + 1 |
| 控制 API（sing-box/mihomo 为 Clash API；Xray 为 `metrics.listen` 的 expvar 端点） | 10812，TUN 时 10813 |
| PAC 脚本服务 | 10811，被占用时顺延到下一个空闲端口并在日志与设置里说明 |
| 测速 | 内核无关的 TCP 连接，无额外端口 |

UI 展示的端口一律来自 `ConnectionState`，不自行推算；内核实际端口可能因设置改变而不同。

## 数据与迁移

- `settings.json`：`schema` 字段 + `migrate(value, from)`，迁移必须幂等；遇到更高版本时备份为
  `settings.json.bak.<ts>` 并回退默认值，**不降级、不猜测**。
- SQLite：`PRAGMA user_version` 记录 schema，`SCHEMA_VERSION` 落后时报 `SchemaTooNew` 而不是删库。
- 订阅刷新失败：`mark_subscription_failed` 只写 `last_error` 与下次重试时间，**保留 `last_updated_at` 与旧节点**。
- 合并：`plan_merge(existing, incoming, salt)` 按连接指纹（忽略备注/标签等展示字段）匹配；
  同一批里的重复项折叠；收藏节点即使上游消失也只 `kept` 不删。

## 错误模型

- 内部 crate 用 `thiserror` 定义错误类型并映射到 `v2rayr_domain::ErrorCode`。
- 跨边界统一成 `Problem { code, key, detail, retryable }`（`crates/ipc/src/error.rs`）：
  `key` 是 `error.<code>`，前端查表拿本地化文案，`detail` 只放人类可读补充，不放堆栈。
- `retryable` 只对网络、存储、端口占用、内核启动、接管失败这类瞬时问题为真；
  参数不合法、协议不支持一律 `false`，UI 据此决定是否显示「重试」。

## 发射器的约束

- `SessionPlan::check()` 在生成任何文本之前执行：端口冲突、监听地址与「允许局域网」自相矛盾、
  TUN 与额外 HTTP 入站同时存在、以及节点自身的 `Profile::validate`（Reality 缺公钥、SS 配 gRPC 等），
  都在这里失败（`error.invalid_value`）。
- 每个内核的发射器各自负责「结构形状」，共享的语义（分流模式、私有网段、自定义规则顺序）在 plan 层统一。
- 输出必须是**确定性的**：同样的输入产生逐字节相同的配置；测试里直接断言关键字段而不是整份 JSON，
  这样新增字段不会把测试全打红。
- YAML 由 `crates/config/src/yaml.rs` 自己写（不引入 serde_yaml），规则：首键与 `- name:` 同行、
  需要引号的标量按 YAML 规则加引号、空 `Option` 不输出。
- 不可表达的能力在 `EmittedConfig.notes` 里说明（例如 FakeIP 需要 TUN、规则集缺失），
  这些 notes 会进日志，UI 不假装它们生效了。

## 线程与异步

- 内核进程管理用 `std::process::Command` + 非阻塞 `try_wait()`（不引入 tokio 的 process feature），
  自检、版本探测、延迟测量都用 `spawn_blocking` 包住再套超时。
- 会话与数据库句柄用 `std::sync::Mutex`（短临界区、无 await 持锁）；跨 await 的串行化用 `tokio::sync::Mutex`。
- 事件出口是 `App::events: Arc<dyn EventSink>`：Tauri 壳实现为 `emit`，CLI 与测试用空实现/记录实现，
  这样 app 层不依赖任何 UI 运行时。

## 常驻资源约束

应用大部分时间在托盘里跑，下面这些是刻意的取舍，改动前先确认没有把常驻开销加回来：

- **UI 不轮询快照**。前端订阅 `session://changed`（完整快照）与 `traffic://tick`（只含流量）；
  另有一个 30 秒、窗口不可见时暂停的兜底刷新。流量单独存放在 store 的 `traffic` 字段，
  每秒的 tick 只重渲染流量面板，不会让节点列表等页面跟着重算。
- **关闭到托盘即销毁 webview**（`PARKED`），再次显示时按 `tauri.conf.json` 里 `create: false` 的窗口配置重建，
  页面加载完成后才显示窗口，避免空白帧；`startMinimized` 且有托盘时启动阶段根本不建窗口。
  销毁窗口触发的 `ExitRequested { code: None }` 在 `PARKED` 时被拒绝，真正的退出（托盘「退出」、更新重启）带 `code`。
- **流量泵**：有可见窗口时 1 秒一次，否则 5 秒一次（内核计数是累计值，总量不受影响）；窗口打开时立即唤醒。
  每个会话持有一个 `TrafficProbe`（单个回环 HTTP 客户端，keep-alive），`/connections` 只解析总量并计数连接、
  不把连接明细反序列化成 JSON 树；Xray 用 `metrics.listen` 的 `/debug/vars`，不再每秒派生 `xray api` 子进程。
  不支持 `metrics.listen` 的旧版 Xray 仍能连接，只是没有流量数据（日志里记一次警告）。
- **SQLite**：每个 tick 一个事务，零流量的 tick 不写库，过期样本每分钟清理一次。
- **内核日志**：默认级别 `warn`（`info` 会逐连接记日志）；启动内核前日志超过 4 MiB 就轮转为 `<name>.log.1`。
- **guardian** 进程用阻塞等待（stdin 租约 + 子进程退出各一个线程），会话期间不产生定时唤醒。

## 安全边界

- 节点密钥不进入 `NodeView`，日志里只出现脱敏后的 URL（`https://***@host`）。
- SQLite 里的凭据载荷经 `Sealer` 处理：默认实现是带前缀的 base64（防日志/截图泄漏），
  `sealed-payloads` feature 打开后是 AES-256-GCM + 0600 密钥文件。威胁模型写得很明确：不防能读你文件的本地攻击者。
- helper 的每个请求都要带 token（文件 0600、常量时间比较），不匹配直接退出 77；token 缺失即拒绝服务。
- 默认只监听回环地址；开启「允许局域网设备接入」才绑定 `0.0.0.0`。

## 分流规则

规则存在 SQLite（`rules` 表），`kind ∈ {domain, domain-suffix, ip-cidr, process, geoip, geosite}`，
`action ∈ {proxy, direct, block}`。编辑走 `App::{rules, add_rule, set_rule_enabled, remove_rule}`，
校验在 app 层完成（类型解析、trim 后非空、≤256 字符、无换行），非法输入返回 `error.invalid_value`。
规则在**下次 connect** 时被读进 `SessionPlan.rules`，因此保存规则不会打断正在运行的会话。

发射器把规则排在私有网段直连之后、规则集之前：自定义规则优先于内置分流，符合「用户显式写的优先」。

## 内核安装

`engine::dist` 固定每个内核的版本、平台 slug 与下载地址，`download_core` 的顺序是：
下载到 `.<name>.download` → 有 SHA-256 就校验（不匹配立即删除并报 `error.core_failed`，没有校验和时只警告）
→ 解压到 `.<name>.unpack` → 找到同名可执行文件 → 复制成 `.<name>.new` 并 chmod 755 → rename 覆盖
→ 跑一次 `version` 记日志。任何中间文件都以 `.` 开头，崩溃后不会留下一个看似可用的半截内核。

## 单实例与托盘（桌面壳）

- 单实例用 `run/v2rayr.lock`：文件里是 pid，第二次启动看到活着的 pid 就尝试激活已有窗口并退出；
  看到死 pid 就接管锁。测试覆盖「第二个实例看到第一个」和「陈旧锁被接管」两种情况。
- 托盘菜单：显示主窗口 / 重新连接（沿用上次模式）/ 断开连接 / 退出。左键点击托盘图标显示窗口。
- `startup.closeToTray = true` 时关闭窗口会销毁 webview、只留托盘，代理继续运行；`startup.startMinimized = true` 时启动不建窗口。
  macOS 上再次启动应用或点 Dock 图标会通过 `reopen` 重建窗口。
- 命令行里的分享链接与 `v2rayr://import/<base64>` 会排队导入，导入结果在下一次 snapshot 里出现。
