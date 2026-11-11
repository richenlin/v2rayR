# 结果矩阵（131 项）

状态仅取 `PASS / FAIL / BLOCKED / NOT-RUN`。证据根目录：`test-results/windows-20261111-173020/`（下称 `$EV`）。

**版本边界（重要）**：本机安装的是**旧构建** `v2rayR 0.1.0 · M4`（`C:\Users\richen\AppData\Local\v2rayR\v2rayr-desktop.exe`，写入时间 2026-11-11 00:54，SHA-256 见 `$EV/hashes-and-versions.txt`），与当前源码 HEAD 不同。凡标注 `安装版` 的结论只对该构建成立；功能/协议/存储结论来自当前源码构建的 `target/debug/v2rayr.exe` 与 `cargo test`。

阻塞原因代码：

| 代码 | 含义 |
|---|---|
| `B-NET` | 外网 HTTPS 不可达（DNS/ICMP 正常、TCP 443 无响应）：无法下载内核/订阅/更新源/远程 WebDAV |
| `B-OTHER-PROXY` | ~~本机 `v2rayN.exe`(PID 96828) + `xray.exe`(158676) 正在运行并接管系统代理~~ **已解除**：v2rayN/xray 已退出，残留的无效系统代理已留档（`$EV/w04-system-proxy/00-v2rayN-leftover-before-cleanup.reg`）并清理为 `ProxyEnable=0` |
| `B-NOT-RUN` | 阻塞原因已解除（机位可用），但本轮未执行该用例 |
| `B-GUI-INTERACT` | 桌面实例可启动、可截图，但无 UI 自动化：需人工点击的交互子项未执行 |
| `B-TUN` | TUN 需特权 helper + 管理员授权 |
| `B-LAN` | 需第二台真实设备（R-LAN） |
| `B-VM` | 需可回滚快照/一次性环境（安装、卸载、故障注入、TUN 破坏性用例） |
| `B-UPD` | 无受信更新源与签名密钥（R-UPDATE） |
| `B-GUI-REBUILD` | 源码已修复并通过类型检查/构建，但离线无法重建 Tauri 外壳，运行期无法复现验证 |

## 12.2 原生外壳、节点与导入导出

| ID | 状态 | 说明 / 证据 |
|---|---|---|
| F-UI01 | PASS（`安装版`，子项缺口：逐页导航） | 隔离数据目录首启：窗口 1216×859 正常渲染（深色主题、五个导航项、空态"还没有节点"、`BUILD 0.1.0 · M4`、右上"本地运行"），无白屏/崩溃；截图 `$EV/desktop-window.png`；版本/schema 由 `status --json` 核对（api 1 / settingsSchema 1 / dbSchema 2） |
| F-UI02 | PASS（`安装版`，子项缺口：双数据目录） | 同一数据目录第二次启动：新进程 PID 152544 立即退出（`second_exited=True`），仅保留主进程 136300 与其窗口 → 单实例激活成立 |
| F-UI03 | BLOCKED `B-GUI-INTERACT` | 缩放/最小窗口/键盘可达性需人工交互 |
| F-UI04 | BLOCKED `B-GUI-INTERACT` | 主题/语言切换需人工交互 |
| F-UI05 | BLOCKED `B-GUI-INTERACT` | 托盘/关闭语义/20 次重建需人工交互 |
| F-N01 | PASS | 单条 VLESS 导入成功；空态无幽灵节点（`cli/out/F-N01-single.txt`、`F-N09-empty-nodes.txt`） |
| F-N02 | PASS | 同链接重复导入 `duplicates:1`；参数顺序变化仍 `duplicates:1`（`F-N02-dup.txt`、`F-N02-reorder.txt`） |
| F-N03 | **PASS**（已修复 D-2） | 链接列表、base64、**Clash YAML、sing-box JSON** 均可导入；`import` 已改用 `preview_from_body` 嗅探（`fix-out/5-clash.json`：`detectedFormat: clash` + 节点 `clash-vless`；`6-singbox.json`：`detectedFormat: sing-box` + `sb-vless`） |
| F-N04 | PASS | 2 条合法入库；`not-a-link`、未知 scheme、缺 UUID、端口 99999 各自单独报错，已存数据不受影响（`F-N04-mixed.txt`） |
| F-N05 | PASS（负向） | 普通 HTTPS URL 计入 `subscriptionUrls`，不建假节点；重复订阅 `error.duplicate_node`（`F-N05-url.txt`、`F-S02-add.txt`） |
| F-N06 | BLOCKED `B-GUI-INTERACT` | 搜索/筛选需人工交互 |
| F-N07 | **PASS**（已修复 D-1） | 导出后再导入现为 `duplicates:1 / added:0`，节点数保持 1、`tls=true`（修复前会生成第二个 `tls=false` 节点）；`fix-out/1-import.json`…`4-nodes.json` |
| F-N08 | BLOCKED `B-GUI-INTERACT` | 删除确认流为桌面交互；CLI 无 `removeNode` |
| F-N09 | PASS（子项缺口：UI 点击） | 连接中切换到坏节点失败并保留原会话、原节点仍 Connected（`windows_loopback_flow` 用例第 3 步）；同节点重复点击/快速连点未做 |
| F-N10 | BLOCKED（部分 PASS） | CLI/存储层：1000 条导入 536 ms、1015 条列表 131 ms（`cli/out2/F-N10-bulk.json`）；UI 虚拟列表未验 |
| F-N11 | BLOCKED `B-VM` | 启动参数与 URL scheme 需安装/卸载验证 |

## 12.3 协议、传输、TLS 及 core 兼容性

本机真实内核：sing-box **1.14.4**、Xray **26.11.9**（安装版自带 sidecar，未下载）。FLOW 通过 `crates/app/tests/windows_loopback_flow.rs` 的本地隔离服务验证（Xray VLESS 入站 + freedom redirect → 本机 HTTP 后端，探测与业务请求均不出机）。

| ID | 状态 | 说明 |
|---|---|---|
| F-P01 | **PASS** | VLESS TCP：sing-box 真实转发（连接探测到达后端、经 HTTP 入口请求返回 204、字节累计增长）；强制 Xray 后再次真实转发且 HTTP/SOCKS 端口分离；断开后端口释放 |
| F-P02 | BLOCKED `B-NET` | Reality/TLS 组合需真实服务端证书与参数；PARSE 通过 |
| F-P03 | BLOCKED `B-NET` | xhttp/mKCP 需对应服务端 |
| F-P04 | BLOCKED `B-NET` | PARSE 通过（VMess+ws）；真实转发需 VMess 服务端 |
| F-P05 | BLOCKED `B-NET` | PARSE 通过（Trojan+ws）；需服务端 |
| F-P06 | BLOCKED `B-NET` | PARSE 通过（AEAD/2022）；需 SS 服务端 |
| F-P07 | BLOCKED `B-NET` | PARSE 通过（SOCKS）；需服务端 |
| F-P08 | BLOCKED `B-NET` | 未构造 http(s)-proxy 节点 |
| F-P09–F-P13 | BLOCKED `B-NET` | HY2/TUIC/AnyTLS/WireGuard/naive 仅 PARSE 通过，缺服务端 |
| F-P14 | BLOCKED `B-NET` | 仅 vless+raw 有 CONFIG/FLOW；其余传输未验 |
| F-P15 | BLOCKED `B-NET` | IPv6/域名/边界端口未构造 |
| F-P16 | PASS（子项缺口：mihomo） | 自动选择 sing-box 与强制 Xray 均与日志中实际进程一致（`windows_loopback_flow`）；三种内核发现正确（`F-K01-cores.txt`）；缺内核时 `core_missing` 拒绝 |

## 12.4 连接、分流、DNS、PAC、LAN、本地转发

| ID | 状态 | 说明 / 证据 |
|---|---|---|
| F-C01 | **PASS**（子项缺口：system-proxy 接入） | 干净数据目录导入→`connect --mode manual`→真实访问（204 + 字节累计）→断开端口释放；无内核时报 `core_missing` 且不接管（`F-C01-connect-id.txt`、`windows-loopback-flow.log`） |
| F-C02 | BLOCKED `B-NET` | global 策略已验证；rule/direct 与 12 格矩阵需真实分流目标 |
| F-C03 | BLOCKED `B-NET` | 分流/DNS 需可区分出口的受控目标 |
| F-C04 | BLOCKED `B-NOT-RUN` | PAC 需接管（`B-OTHER-PROXY` 已解除）；本轮只补测了系统代理接管（W04），PAC 未执行 |
| F-C05 | BLOCKED `B-LAN` | 认证 LAN 需第二台设备 |
| F-C06 | BLOCKED `B-NET` | Dokodemo/本地转发需真实目标 |

## 12.5–12.6 订阅、测速、流量、存储、日志

| ID | 状态 | 说明 / 证据 |
|---|---|---|
| F-S01/F-S02 | BLOCKED `B-NET` | 订阅拉取依赖外网；重复添加被拒（`F-S02-add.txt`） |
| F-S03 | PASS（本地部分） | `sub list` 正确输出 host/节点数/`上次失败: error.network_error` |
| F-S04/F-S05/F-S07 | BLOCKED `B-NET` | 需可切换内容的订阅源与长时运行 |
| F-S06 | BLOCKED `B-NET` | `sub update` 如实报 `error.network_error`，未伪造成功 |
| F-S08 | BLOCKED | 订阅删除路径未构造第二订阅 |
| F-T01 | **PASS** | 单节点 TCP 测速：`QA-LAT 3 ms tcp`（对本地受控监听器，`cli/out2/F-T01-latency.txt`） |
| F-T02 | BLOCKED `B-NET` | 批量测速在 1016 个不可达节点上 60 s 超时（`F-T02-batch-latency.txt`，rc=124），未形成结论 |
| F-T03 | BLOCKED `B-NET` | 并发/取消行为未验 |
| F-Q01 | PASS | 版本/schema/快照字段完整（`F-UI01-status.txt`） |
| F-Q02 | PASS（子项缺口） | 状态机：Failed（无内核）→ Connected（manual）→ Failed（坏节点切换被拒但保持 Connected）→ Disconnected 均按预期流转；system-proxy/PAC 状态未验 |
| F-Q03 | **PASS** | 入站端口、核心类型、Xray HTTP/SOCKS 分离端口与真实监听一致（`windows_loopback_flow`） |
| F-Q04 | **PASS** | 内核发现与版本：sing-box 1.14.4 / Xray 26.11.9 已安装、mihomo 未找到（`F-K01-cores.txt`、`$EV/hashes-and-versions.txt`） |
| F-Q05 | PASS | 真实流量统计：`poll_traffic()` 上下行字节在真实请求后增长，与快照 totals 一致（`windows_loopback_flow`） |
| F-Q06 | BLOCKED `B-GUI-INTERACT` | UI 侧状态呈现需交互 |
| F-Q07 | BLOCKED `B-NET` | 测速与 PAC 刷新在受限网络下不可完成 |
| F-V01–F-V05 | BLOCKED `B-GUI-INTERACT` | 需桌面长时采样（W08） |
| F-V06–F-V08 | BLOCKED `B-NET` | 长时采样需真实流量 |
| F-V09 | PASS（空态） | `logs 50` 空态正常（`F-Q09-logs.txt`） |
| F-V10/F-V11 | BLOCKED `B-NET` | 长时性能/数据库口径需真实流量 |

## 12.7b LAN 认证（F-L01–F-L04）

| ID | 状态 | 说明 |
|---|---|---|
| F-L01 | BLOCKED `B-LAN` | 需第二台真实设备才能验证 LAN 绑定与凭据拒绝/接受 |
| F-L02 | BLOCKED `B-LAN` | 同上（HTTP 认证 407/204 需跨设备） |
| F-L03 | BLOCKED `B-LAN` | 同上（SOCKS 无认证拒绝、认证接受） |
| F-L04 | BLOCKED `B-LAN` | 断开后端口关闭与快照不含密码需跨设备用例 |

## 12.7 备份、WebDAV、内核维护、更新、签名

| ID | 状态 | 说明 / 证据 |
|---|---|---|
| F-B01/B02/B03 | PASS | 导出→预览（明确"仅预览"）→应用（合并 0 条规则、恢复 5 个偏好组） |
| F-B04/B05/B06 | BLOCKED `B-NET` | 无 HTTPS WebDAV 端点 |
| F-K01 | **PASS** | 安装版 sidecar 与 `data/cores` 均被正确识别为"已安装"并给出路径（`F-K01-cores.txt`）；未知/缺内核不误报 |
| F-K02 | BLOCKED `B-NET` | 缺失内核的下载安装不可达 |
| F-K03 | **PASS**（已修复 D-4） | 自定义 URL 缺 SHA-256 → `error.invalid_value 自定义内核地址必须提供有效的 SHA-256`，不再报 `core_failed / exited during startup`，且未落盘（`fix-out/7-install.txt`） |
| F-K04 | BLOCKED | 优先级/Defender 隔离需真实升级场景 |
| F-U01–F-U03 | BLOCKED `B-UPD` | 无受信更新源与签名包 |
| F-I01 | FAIL（本地构建） | `dist` 两个安装器 `Get-AuthenticodeSignature` = `NotSigned`；安装版本体亦无 Authenticode 记录（`$EV/env.txt`、`$EV/hashes-and-versions.txt`） |

## 12.8–12.10 设置字段、CLI、故障注入与救援

| ID | 状态 | 说明 |
|---|---|---|
| F-R01–F-R08 | BLOCKED `B-GUI-INTERACT`/`B-NET` | 规则集/自定义规则/PAC 为桌面与网络能力 |
| F-D01/F-D02 | BLOCKED `B-GUI-INTERACT` | 需桌面界面用例 |
| S01–S08 | BLOCKED `B-GUI-INTERACT` | 逐字段设置检查需人工交互；CLI 快照已确认 settings 结构与默认值（schema 1） |
| S09 | PASS（部分） | `paths` 正常，sqlite/settings 在隔离目录生成并持久（1000 节点重启仍在）；UI 重建未验 |

## 第 5–9 节 系统与故障专项（W01–W27）

| ID | 状态 | 说明 / 证据 |
|---|---|---|
| W01 | PASS（`安装版`，子项缺口：逐页导航） | 首启渲染正常、二次启动单实例激活；启动耗时未精确计时，页面未逐页点击；进程树：主应用 136300 → msedgewebview2 140976 → 7 个子进程 |
| W02 | BLOCKED `B-GUI-INTERACT` | 导入/收藏/筛选为桌面交互 |
| W03 | PASS（子项缺口：system-proxy） | manual 模式下 sing-box 与 Xray 均真实转发；两 core 的入口端口分别核实 |
| W04 | **PASS** | 真实接管实测：干净基线（`ProxyEnable=0`）下连接后 `ProxyEnable=1`、`ProxyServer=http/https=127.0.0.1:10809;socks=127.0.0.1:10808`、bypass 写入 WinINet 通配形式、残留 `AutoConfigURL` 被清空；应用读回与本应用入站一致；**另一个进程**用 .NET `DefaultWebProxy`（WinINet）请求受控目标得 `204`；断开后逐字段回到基线、journal 删除、入站端口释放。证据 `$EV/w04-system-proxy/`（`01-test-run.log`、`02-registry-during-takeover.txt`、`03-system-proxy-client.txt`、`04-registry-after-restore.txt`） |
| W05 | BLOCKED `B-NOT-RUN` | PAC 接管：`B-OTHER-PROXY` 已解除，本轮未执行 |
| W06 | BLOCKED `B-GUI-INTERACT` | 托盘 20 次重建需交互 |
| W07 | BLOCKED `B-GUI-INTERACT` | 草稿恢复/撤销需交互 |
| W08 | BLOCKED `B-NET` | 需 ≥60 s 持续下载 |
| W09 | BLOCKED `B-GUI-INTERACT` | 托盘退出清理需先接管系统代理并点击托盘退出；`B-OTHER-PROXY` 已解除，登录启动/托盘交互仍无自动化 |
| W10 | BLOCKED `B-VM` | `taskkill /F` 主进程 → guardian 接手，需可回滚环境 |
| W11 | BLOCKED `B-VM` | 强杀 core 需可回滚环境 |
| W12 | BLOCKED `B-VM` | 强杀 guardian 需可回滚环境 |
| W13 | **PASS**（已修复 D-3） | 不可解析的 `takeover.json` 不再被删除，而是原字节保留为 `takeover.corrupt-<ms>.json` 并写 warn 日志（`fix-out/8-corrupt-status.txt`：run 目录内 `takeover.corrupt-1794391772557.json` 内容仍为 `{"garbage":`） |
| W14 | BLOCKED `B-VM` | 20 次连接/强杀循环需可回滚环境 |
| W15 | BLOCKED `B-VM` | 端口竞态需可回滚环境 |
| W16 | BLOCKED `B-GUI-INTERACT` | Ctrl+C/控制台结束语义需交互式控制台 |
| W17–W20 | BLOCKED `B-NOT-RUN` | 需真实接管后再由外部改写/注入失败（现已可执行，本轮未做） |
| W21 | BLOCKED `B-VM` | 卸载/重装需一次性环境；安装版本为 per-user（`%LOCALAPPDATA%`），与仓库 `perMachine` 配置不一致（见 README 版本边界） |
| W22 | BLOCKED `B-VM` | `test-windows-tun-package.ps1` 需管理员 + 会破坏 Wintun |
| W23/W24 | BLOCKED `B-TUN` | TUN 需 helper 授权与 UAC |
| W25 | BLOCKED `B-VM` | 登录启动需真实注销/登录 |
| W26 | BLOCKED `B-UPD` | 在线升级无源；备份子项已 PASS |
| W27 | BLOCKED `B-GUI-REBUILD`（**源码已修复**） | `ui/src/pages/Nodes.tsx` 断开态已改为沿用当前接入方式与策略（`connection?.mode ?? "system-proxy"` / `connection?.policy ?? "rule"`），不再硬编码；`pnpm check`/`build`/`verify-lifecycle` 均通过，但离线无法重建 Tauri 外壳，运行期复现待补 |

## 计数

| 状态 | 数量 |
|---|---|
| PASS | 30 |
| FAIL | 1（F-I01） |
| BLOCKED | 100 |
| NOT-RUN | 0 |
| 合计 | 131 |
