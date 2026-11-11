# Windows 本机验收执行记录（按 `docs/windows-system-test-manual.zh-CN.md`）

执行日期：2026-11-11（+08:00）
结论：**仍不满足手册 §12.11 的完整验收门槛**：PASS 30 项、FAIL 1 项、BLOCKED 100 项（95 F + 27 W + 9 S = 131）。本轮已修复上轮报出的 5 个代码缺陷中的 4 个（D-1/D-2/D-3/D-4）与 D-6（源码修复，运行期未验证），剩下 F-I01（安装器未签名）需正式证书。补测轮：v2rayN 退出后解除 `B-OTHER-PROXY`，**W04 系统代理真实接管转为 PASS**，并修掉本次补测暴露的 D-9、D-10、D-11。PAC/故障恢复矩阵/安装/长稳仍未验收，见 §11。

- 逐项矩阵：`matrix.md`
- 证据根目录：`test-results/windows-20261111-173020/`（下称 `$EV`）

## 1. 版本边界（先读）

| 对象 | 事实 |
|---|---|
| **本机安装版** | `C:\Users\richen\AppData\Local\v2rayR\`（per-user），`v2rayr-desktop.exe` 文件版本 0.1.0、界面自报 `BUILD 0.1.0 · M4`、写入时间 2026-11-11 00:54:50，SHA-256 记录于 `$EV/hashes-and-versions.txt`。**这是旧构建**，与当前源码 HEAD 不等价；凡标注 `安装版` 的结论仅对该构建成立。 |
| 安装方式差异 | 安装目录在 `%LOCALAPPDATA%`（per-user NSIS），而仓库 `apps/desktop/tauri.windows.conf.json` 现配置为 `installMode: perMachine` → 进一步说明安装包来自更早的构建设置。 |
| **本轮功能结论来源** | 当前源码构建：`target/debug/v2rayr.exe` + `cargo test`（HEAD 的 `git status` 除本报告与新增测试外干净） |
| 自带内核 | `v2rayr-core-sing-box.exe` = **sing-box 1.14.4**；`v2rayr-core-xray.exe` = **Xray 26.11.9**（本轮未下载，来自安装版 sidecar）；mihomo 未提供 |
| 其他接管工具 | 补测时 `v2rayN.exe`/`xray.exe` **均已退出**（`tasklist` 无匹配）；v2rayN 退出时未还原系统代理，留下 `ProxyEnable=1` + `http/https=127.0.0.1:10809;socks=127.0.0.1:10808` 指向死端口，原值已导出 `$EV/w04-system-proxy/00-v2rayN-leftover-before-cleanup.reg` 后清理为 `ProxyEnable=0` |

## 2. 环境（§2）

| 项 | 值 |
|---|---|
| OS | Windows 10 Pro / 2009 / Build 26300 / x64 |
| 账户/权限 | `RICHENSMINIPC\richen`，**非管理员**（安装/TUN/卸载未提权） |
| 网络 | DNS + ICMP 正常，**出站 TCP 443 无响应**（github/google/crates.io 全部超时） |
| 工具链 | rustc 1.95.0 (msvc)、cargo 1.95.0、node v25.8.1、pnpm 10.33.0 |
| 隔离数据目录 | `%LOCALAPPDATA%\v2rayR-QA\20261111-173020`（`V2RAYR_HOME`）；桌面另用 `$EV/desktop-home`，**未触碰用户真实数据目录 `%APPDATA%\v2rayR`** |

## 3. 构建与基础回归（§2）——PASS

| 步骤 | 结果 | 证据 |
|---|---|---|
| `cargo test --workspace --locked` | 300 passed / 0 failed / 5 ignored（首轮与加入新测试后各跑一次均绿） | `$EV/workspace-tests.log`、`workspace-tests-2.log` |
| ↳ `real_connection.rs` | **0 项执行**（Unix 专用），未计入通过 | 同上 |
| `pnpm --dir ui install/check/build` | PASS | `$EV/ui-check.log`、`ui-build.log` |
| `node ui/scripts/verify-lifecycle.mjs` | PASS（boot/refresh 竞态、重开页与筛选、监听清理、草稿恢复） | `$EV/ui-lifecycle.log` |
| `cargo build --locked -p v2rayr-cli` | PASS | `$EV/cli-build.log` |
| 桌面重建 | **BLOCKED**：`apps/desktop` 独立 workspace 依赖未缓存 + 离线，无法编译 → 桌面用例改用**已安装旧构建**执行并明确标注 | — |

## 4. 采证工具修复 + WinINet 原生适配器（§3–§4）——PASS

`scripts/capture-windows-lifecycle.ps1` 原实现在 `INTERNET_PER_CONN_BYPASS_LIST` 返回 `ERROR_INVALID_PARAMETER(87)` 时直接 `throw`，导致四字段全部采不到。已改为逐字段容错并输出 `queryErrors`/`bypassQueryRefused`（保留错误，不丢字段）。

| 用例 | 结果 |
|---|---|
| `real_wininet_round_trip`（`V2RAYR_TEST_WININET=1`） | **PASS**，真实执行 1 项（0.13 s）：手动代理 10808 与 PAC 写入→读回→恢复 |
| `unsafe_bypass_round_trip_keeps_the_other_fields` | PASS（1 项执行）；基线 `ProxyOverride` 含 CIDR/IPv6，但测试未打印值 → 特殊分支覆盖仍属**推断** |
| 测试后与收尾采证 | 与基线逐字段 **EQUAL: true**（flags=5、server=127.0.0.1:1080、pac 不变） |
| 基线注意 | 本轮开始时即为**脏基线**（`ProxyEnable=0` 但残留 server/AutoConfigURL，flags=5=直连+PAC）——由正在运行的 v2rayN 造成 |

## 5. 桌面实机（`安装版 0.1.0 · M4`）——渲染与单实例 PASS

隔离数据目录启动 `v2rayr-desktop.exe`：

- 首启 PID 136300，主窗口标题 `v2rayR`，1216×859，WebView2 子树（140976 → 7 进程）建立；
- **界面真实渲染**：深色主题、左侧五项导航（连接/节点/订阅/测速/设置）、"未连接"概览、"还没有节点"空态、右上"本地运行"、`BUILD 0.1.0 · M4`；截图 `$EV/desktop-window.png`；
- 默认接入方式显示"系统代理"、分流策略"规则"（与 W27 缺陷直接相关）；
- 第二次启动 PID 152544 **立即退出**（`second_exited=True`），系统里只有 1 个主进程 → 单实例激活成立；
- 未连接就不会写系统代理，测试期间未改动用户的代理设置。

未覆盖：逐页导航、导入/筛选/主题/托盘等需要人工点击的交互（无 UI 自动化）；启动耗时未精确计时。

## 6. 真实转发（本轮新增，核心结论）

新增 `crates/app/tests/windows_loopback_flow.rs`（`#![cfg(windows)]` + `#[ignore]`，需两个环境变量指向真实内核）：用**本机 Xray 起 VLESS 入站 + freedom redirect** 作为受控"远端"，本机 HTTP 后端返回 204，探测 URL 与业务请求都不出机（不违反"不得用 mock 代替验收"：是真实内核、真实转发、真实字节统计）。

```
cargo test -p v2rayr-app --locked --test windows_loopback_flow -- --ignored --nocapture
# 实测 1 passed，21.05 s   （$EV/windows-loopback-flow.log）
```

断言全部通过：

1. 无内核时 `connect` → `core_missing`，阶段 `Failed`，无入站端口，系统代理接管调用 **0** 次；
2. sing-box：连接成功（探测真实到达后端 `hits>0`）、经 HTTP 入口请求返回 **204**、`poll_traffic()` 上下行字节增长且与快照 totals 一致；
3. 切换到坏 UUID 节点失败，原会话保持 `Connected` 且仍是原节点；
4. 断开后入站端口释放（再次 connect 失败）；
5. 强制 Xray：连接成功、HTTP/SOCKS **端口分离**、再次真实转发并累计字节；
6. 全程 `manual` 模式对系统代理的 `apply` 调用为 **0**。

对应矩阵：F-C01、F-P01、F-P16、F-Q02、F-Q03、F-Q04、F-Q05、W03 由 BLOCKED 转为 PASS（含子项缺口说明）；F-N09 的连接中切换失败保留会话亦由此用例证明。

## 7. CLI 功能用例（当前源码构建，隔离数据目录）

命令与原始输出：`$EV/cli/out/*.txt`、`$EV/cli/out2/*`。

**PASS**：F-N01（单条导入）、F-N02（重复/改序去重 `duplicates:1`）、F-N04（混合输入逐条报错，已存数据不变）、F-N05（URL 归入订阅、不建假节点）、F-S03（订阅列表含失败原因）、F-Q01（版本/schema）、F-V09（日志空态）、F-B01/B02/B03（备份导出/预览/应用）、F-K01（内核发现：sing-box 与 Xray"已安装"并给出路径）、F-K03（缺 SHA-256 的自定义内核被拒绝，负向 PASS）、F-T01（单节点 TCP 测速 `3 ms`）。

**其他实测数据**：1000 节点导入 536 ms、1015 节点列表 131 ms；`sub update` 如实报 `error.network_error`（未伪造成功）；批量测速对 1016 个不可达节点 60 s 超时（无结论）。

### 7.1 系统代理真实接管与还原（W04，v2rayN 退出后补测）—— PASS

补测前置：`tasklist` 已无 `v2rayN`/`xray`；把 v2rayN 留下的无效代理清理为干净基线（`ProxyEnable=0`、`ProxyServer=""`，`ProxyOverride` 保持 `<local>;localhost,127.0.0.0/8,::1`）。

用例：`cargo test -p v2rayr-app --locked --test windows_loopback_flow windows_system_proxy_takeover -- --ignored --nocapture`（真实 `wininet` 适配器 + 真实 Xray 内核 + 本机受控后端，日志 `$EV/w04-system-proxy/01-test-run.log`，`1 passed` / 33.80 s）。

| 断言 | 结果 |
|---|---|
| 干净基线下连接 → 写入真实 WinINet | PASS：`ProxyEnable=1`、`ProxyServer=http=127.0.0.1:10809;https=127.0.0.1:10809;socks=127.0.0.1:10808`、`ProxyOverride` 写成 WinINet 通配形式、残留 `AutoConfigURL` 被清空（`02-registry-during-takeover.txt`） |
| 应用读回 == 写入 | PASS：`capture().pointing_at(入站端点)` 成立，且与基线不同 |
| **另一个进程**用系统代理访问 | PASS：`Invoke-WebRequest`（.NET `DefaultWebProxy` → WinINet）请求受控目标返回 `204`（`03-system-proxy-client.txt`），同时经代理入口的 `curl` 亦 `204` |
| 断开逐字段还原 | PASS：`ProxyEnable=0`、`ProxyServer=""`、`ProxyOverride`/`AutoConfigURL` 与基线一致（`04-registry-after-restore.txt`） |
| 恢复日志与端口 | PASS：`disconnect()` 后 `takeover.json` 已删除、入站端口不可连、探测请求确已到达后端 |
| 无残留进程 | PASS：测试结束 `tasklist` 无 `xray`/`sing-box` |

补测暴露并修复的缺陷见 **D-9**、**D-11**（接管写后读回）；稳定性：修复后同一用例连续 3 次通过。测试夹具本身也有两处需注意（`#[tokio::test]` 单线程运行时下不能在运行线程里同步等子进程；Xray 的 HTTP 入站会把 `204` 转回却保留 keep-alive，.NET 客户端要 `-DisableKeepAlive`）。

## 8. FAIL 与缺陷

| 编号 | 严重度 | 现象 | 证据 |
|---|---|---|---|
| **D-1** F-N07 | 高 | ~~导出分享链接后重新导入**未去重**~~ **已修复**：`tls_from` 不再把 `encryption` 当安全别名（导出先写 `encryption=none`，旧代码取到它后把 TLS 关掉，SNI 随之丢失→指纹变化→重复节点） | `fix-out/1-import.json`…`4-nodes.json`；单测 `round_trip_keeps_tls_and_the_dedupe_fingerprint`、`an_exported_link_re_imports_as_a_duplicate` |
| **D-2** F-N03 | 中 | ~~`import` 不识别 Clash YAML / sing-box JSON~~ **已修复**：`import` 改用共享的 `v2rayr_engine::preview_from_body`（与订阅刷新同一嗅探/转换路径） | `fix-out/5-clash.json`（`detectedFormat: clash`）、`6-singbox.json`（`detectedFormat: sing-box`）；单测 `a_clash_yaml_file_imports_like_a_subscription`、`a_singbox_json_file_imports_like_a_subscription` |
| **D-3** W13 | 高 | ~~不可解析的 `takeover.json` 被启动时直接删除~~ **已修复**：改为原字节保留为 `takeover.corrupt-<ms>.json` 并写 warn 日志；重命名失败时保持原文件不动 | `fix-out/8-corrupt-status.txt`；单测 `a_corrupt_takeover_journal_is_preserved_instead_of_deleted` |
| **D-4** F-K03 | 低 | ~~错误码错乱（`core_failed` + `exited during startup`）~~ **已修复**：新增 `EngineError::InvalidRequest` → `invalid_value`，HTTPS 校验与缺摘要两处改用该错误 | `fix-out/7-install.txt`；单测 `a_custom_url_without_a_digest_is_a_request_error` |
| **D-5** 环境 | 中 | ~~基线系统代理为脏状态，由运行中的 v2rayN 造成~~ **补测时已消除**：v2rayN 退出后仍留下指向死端口的有效代理，说明"每例结束恢复基线"在多接管工具并存时不成立；原值留档后清理，基线恢复为 `ProxyEnable=0` | `$EV/w04-system-proxy/00-v2rayN-leftover-before-cleanup.reg` |
| **D-6** W27 | 高 | ~~`Nodes.tsx:149` 断开态"切换"硬编码 `connect(node.id,"system-proxy","rule")`~~ **源码已修复**：改为沿用当前接入方式/策略；类型检查与构建通过，**运行期未验证**（离线无法重建 Tauri 外壳） | `ui/src/pages/Nodes.tsx`、`pnpm check/build` |
| **D-7** F-I01 | 中 | `dist` 两个安装器 `Get-AuthenticodeSignature` 均为 `NotSigned`，无正式证书 | `$EV/env.txt`、`hashes-and-versions.txt` |
| **D-9** W04 | 中 | ~~`wininet::target()` 只在与目标一致时清 PAC：机器上残留"未启用但存在"的 `AutoConfigURL` 时，写入手动代理会清掉该值，读回校验却仍按旧值比对 → 接管被误判失败（实测首轮 `windows_system_proxy_takeover_*` 报"读回不一致"，第二次起因值已被清空而通过）~~ **已修复**：`target()` 在关闭 PAC 位时把期望 `pac` 置空，并补单测 `a_stale_auto_config_url_is_part_of_the_expected_state` | `crates/platform/src/proxy/wininet.rs`；`cargo test -p v2rayr-platform --lib`（28 passed） |
| **D-11** W04 | 高 | 写后读回用 `INTERNET_OPTION_PROXY` 取例外列表，WinINet 会先返回缓存/规范化过的旧列表：`ProxyServer` 已写入成功却报 `takeover_failed`（基线 `ProxyOverride=<local>;localhost,127.0.0.0/8,::1` 时稳定复现，早前偶发通过），`capture()` 同样可能拿到旧值。**已修复**：`read()` 改读实际生效的连接记录（`effective_bypass()`，仅在记录缺失时回退 `ProxyOverride`） | `crates/platform/src/proxy/wininet.rs`；接管用例连续 3 次 `1 passed`；`cargo test --workspace --locked` 308 passed |
| **D-10** 订阅 | 中 | `Fetcher` 用 reqwest 默认配置，会继承 shell 里的 `HTTP_PROXY`/`HTTPS_PROXY`：本机该变量指向未运行的 `127.0.0.1:1080`，订阅更新会绕开应用自己的路由被悄悄改道（同一原因让 `subscriptions` 单测在命 127.0.0.1 时必失败）。**已修复**：客户端加 `.no_proxy()`，订阅一律按应用自己的决策直连 | `crates/engine/src/fetcher.rs`；`cargo test -p v2rayr-app --lib`（40 passed，且**未**清除 `HTTP_PROXY`） |
| **D-8** 版本一致性 | 中 | 本机安装版为旧构建 0.1.0·M4（00:54 写入、per-user 安装），与当前源码 HEAD 及仓库 `perMachine` 打包配置不一致；不得用其结论代替当前源码验收 | §1、`hashes-and-versions.txt` |

## 9. 阻塞清单（100 项）

1. `B-NET` 离线：出站 443 不通 → 订阅拉取、内核下载、PAC 规则、更新检查、远程 WebDAV、多数协议 FLOW（缺服务端）全部不可执行。
2. ~~`B-OTHER-PROXY`：v2rayN + xray 正在接管真实系统代理~~ **已解除**：两者已退出、残留代理已留档清理。W04 转 PASS；W05（PAC）、W09（托盘退出清理）、W17–W20（接管被外部改写/注入失败）、F-C04 现为可执行但本轮未做的 `B-NOT-RUN`。
3. `B-GUI-INTERACT`：桌面可启动可截图，但无 UI 自动化 → 逐页导航、导入/筛选、主题、托盘 20 次重建、草稿撤销、Ctrl+C 控制台语义等需人工点击的项未执行。
4. `B-VM`：卸载/重装、`taskkill /F` 故障矩阵（W10–W12、W14、W15）、登录启动（W25）、`test-windows-tun-package.ps1` 需一次性可回滚环境。
5. `B-TUN`：TUN 需特权 helper + UAC；`B-LAN`：认证 LAN 需第二台真实设备；`B-UPD`：无受信更新源与签名密钥。

## 10. 复现命令

```bash
cargo test --workspace --locked   # D-10 修复前需先 `unset HTTP_PROXY HTTPS_PROXY`（本机 shell 设有 http://127.0.0.1:1080），现已不需要
pnpm --dir ui install --frozen-lockfile && pnpm --dir ui check
node ui/scripts/verify-lifecycle.mjs && pnpm --dir ui build
cargo build --locked -p v2rayr-cli

# 采证 + 原生适配器往返（会短暂改写系统代理，测试后自恢复）
powershell -NoProfile -File scripts/capture-windows-lifecycle.ps1 -EvidenceDir "<dir>" -TestRoot "<隔离目录>"
V2RAYR_TEST_WININET=1 cargo test -p v2rayr-platform --locked real_wininet_round_trip -- --ignored --nocapture
V2RAYR_TEST_WININET=1 cargo test -p v2rayr-platform --locked unsafe_bypass_round_trip_keeps_the_other_fields -- --ignored --nocapture

# 系统代理真实接管（会短暂改写系统代理，测试结束自恢复；需独占机位）
export V2RAYR_TEST_XRAY='C:\Users\richen\AppData\Local\v2rayR\v2rayr-core-xray.exe'
cargo test -p v2rayr-app --locked --test windows_loopback_flow windows_system_proxy_takeover -- --ignored --nocapture

# 真实转发（真实内核 + 本地隔离服务，不写系统代理）
export V2RAYR_TEST_SING_BOX='C:\Users\richen\AppData\Local\v2rayR\v2rayr-core-sing-box.exe'
export V2RAYR_TEST_XRAY='C:\Users\richen\AppData\Local\v2rayR\v2rayr-core-xray.exe'
cargo test -p v2rayr-app --locked --test windows_loopback_flow -- --ignored --nocapture

# 桌面实机（安装版，隔离数据目录）
V2RAYR_HOME="<dir>" "C:\Users\richen\AppData\Local\v2rayR\v2rayr-desktop.exe"

# CLI 功能套件与 W13 复现
bash "test-results/windows-<stamp>/cli/run-cli-suite.sh"
mkdir -p "$EV/cli/home-corrupt/run" && printf '{"garbage":' > "$EV/cli/home-corrupt/run/takeover.json"
./target/debug/v2rayr.exe --home "<win path>\home-corrupt" status && ls "$EV/cli/home-corrupt/run"
```

## 11. 五组结论（§12.11 第 5 条）

| 组 | 结论 |
|---|---|
| 正常功能 | **部分通过，整体不通过**：VLESS 在两个真实内核上的 CONFIG/FLOW、探测、断开释放、测速、导入导出（含 Clash/sing-box 格式与导出去重）、备份、内核发现、**系统代理真实接管与还原（W04，补测）**在当前源码实机通过；但 PAC/TUN/LAN、订阅、WebDAV、更新未验收，仅剩 F-I01 为 FAIL |
| 故障恢复 | **部分通过**：W13 修复后损坏 journal 被保留；W10–W12、W14–W20 仍因需一次性环境/系统接管而阻塞 |
| 平台安装 | **未验收**：安装版是旧构建，卸载/重装（W21/W22）需一次性环境与管理员；两个本地安装器未签名 |
| 长稳性能 | **未验收**：仅 1000 节点导入 536 ms / 列表 131 ms；批量测速超时；W08 与长时采样未做 |
| 未支持与阻塞 | 离线、其他接管工具占用系统代理、无 UI 自动化、无第二设备/快照、无受信更新源；F-N03（Clash/sing-box 格式导入）、W27（切换节点改接入方式）为确认缺口 |

## 12. 需负责人决策

1. F-I01：安装器/应用需正式证书签名后重跑（本地构建无法签名）；
2. D-8：是否以"安装版 0.1.0·M4"为验收对象重新出包（当前源码与安装版不一致）；
3. W27 修复需在可重建外壳的环境中做一次交互复现（本机离线无法重建）；
4. 需要人工交互的 UI 子项（F-UI03/04/05、W06/W07、F-N06/N08/N11、S01–S08）与系统接管/PAC 项（需先退出 v2rayN）由谁在何时补跑。

## 13. 本轮修复记录（响应 §8 缺陷）

| 缺陷 | 改动 | 回归证据 |
|---|---|---|
| D-1 | `crates/domain/src/sharelink/mod.rs`：`tls_from` 的 security 别名去掉 `encryption`；`crates/domain/src/sharelink/export.rs` 新增 2 个回归单测 | `cargo test --workspace` 307 passed；CLI 实测 `duplicates:1 / added:0`、`nodes=1 tls=true` |
| D-2 | `crates/engine/src/fetcher.rs`：抽出 `pub fn preview_from_body`；`crates/engine/src/lib.rs` 导出；`crates/app/src/importer.rs` 改用嗅探路径（空输入保持原行为）；新增 3 个单测 | CLI 实测 `detectedFormat: clash / sing-box` 并建节点 |
| D-3 | `crates/app/src/lib.rs`：损坏 journal 改为保留为 `takeover.corrupt-<ms>.json` + warn；新增单测 | CLI 实测 run 目录保留 `takeover.corrupt-1794391772557.json` |
| D-4 | `crates/engine/src/lib.rs` 新增 `InvalidRequest → invalid_value`；`crates/engine/src/dist.rs` 两处校验改用它；新增单测 | CLI 实测 `error.invalid_value 自定义内核地址必须提供有效的 SHA-256` |
| D-6 | `ui/src/pages/Nodes.tsx`：断开态改用 `connection?.mode/policy` | `pnpm check`、`pnpm build`、`verify-lifecycle.mjs` 均通过；运行期未验证 |

验证命令（本轮实际执行，exit 0）：

```bash
cargo test --workspace --locked                                   # 307 passed / 0 failed（原 300 + 新增 7）
cargo test -p v2rayr-app --locked --test windows_loopback_flow -- --ignored --nocapture   # 1 passed（真实内核 FLOW）
pnpm --dir ui check && pnpm --dir ui build && node ui/scripts/verify-lifecycle.mjs
```
