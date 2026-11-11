# 本机功能验证记录

日期：2026-11-11，Asia/Shanghai。环境：macOS 15.7.4 / Apple arm64；Rust 1.98.0，Node 22.23.1。源码HEAD为 `a741f3ee8ba741ddf360cbd0c942bd97a5a0b412`，包含未提交改动。本记录针对本轮实际构建/运行的工作区，不是该提交或正式安装包的发布验收。

结论：本机可安全执行的回归、真实core链路、守护进程及原生桌面手动模式测试已完成，下面通过项均有输出证据。发现一个未修复的节点页模式覆盖问题，因此不能标记整个产品“全功能通过”。真实系统代理写入/恢复、TUN接管、Windows安装与运行、故障注入有界退出仍未验收。

## 1. 自动检查

| 检查 | 结果 | 证据与范围 |
|---|---|---|
| `cargo test --workspace` | 309通过、0失败、6忽略 | `workspace-tests.log`；含单元、CLI、守护集成；不能把所有测试都当守护路径测试。 |
| 本地真实VLESS连接测试，显式启用ignored用例 | 1通过 | `real-connection.log`；真实sing-box/Xray、真实本地VLESS服务和HTTP后端；验证错误节点回滚、端口冲突、手动/系统接管编排及恢复journal，但系统代理适配器为FixtureProxy，不写系统代理；未启用可选LAN分支。 |
| 导入协议生成配置由真实内核校验 | 1通过 | `native-config-checks.log`；不代表所有协议都建立了真实远端连接。 |
| TUN桥接配置由真实sing-box校验 | 1通过 | `native-tun-config.log`；不代表创建了接口、改路由或测试了真实TUN流量。 |
| UI类型检查、生命周期确定性回归、生产前端构建 | 通过 | `ui-check.log`、`ui-lifecycle.log`、`ui-build.log`；生命周期脚本使用mock，并非原生UI证据。 |
| 桌面debug构建，启用`tauri/custom-protocol` | 通过 | `desktop-build.log`；未进行正式安装、签名/公证或升级验收。 |

workspace中的6个忽略项有3个已单独启用并通过：真实连接、协议配置校验、TUN桥接配置校验。另外3个为原生TUN运行、GNOME GSettings写入、守护子进程测试夹具。夹具由父进程死亡测试显式调用，不能作为独立有效测试计数。

## 2. 真实守护进程与core

运行仓库脚本`scripts/verify-local-guardian.py`（准确命令见下方），分别使用当前CLI二进制、桌面二进制作为guardian入口。每例用真实sing-box 1.14.4、本机临时端口HTTP服务，发出HTTP代理请求并校验200与完整标记正文。

| 场景 | CLI清理耗时 | 桌面清理耗时 | 结果 |
|---|---:|---:|---|
| 关闭租约 | 0.311秒 | 0.314秒 | 两者通过 |
| 对持有租约的父进程执行SIGKILL | 0.297秒 | 0.342秒 | 两者通过 |
| SIGHUP/SIGINT保持存活，SIGTERM触发清理 | 0.686秒 | 0.671秒 | 两者通过；耗时含两个信号之间的短观察间隔 |
| core先被SIGKILL | 0.007秒 | 0.004秒 | 两者报告非零退出；core不残留 |

证据：`native-cli/`、`native-desktop/`下的结果JSON、各例core日志、系统代理前后快照，以及对应顶层`.log`。结果JSON记录二进制SHA-256、PID/PPID/PGID及core命令。清理路径结束后核验core/guardian消失、入口关闭。

重要区分：

- 该脚本构造合法macOS journal但`services=[]`，没有应用持有的槽位。它调用真实系统适配器读取当前状态，验证no-op恢复与journal删除，**没有验证真实系统代理写入或还原一个被接管的槽位**。
- core先退出时，单独guardian将状态返回应用，不主动恢复journal。本脚本没有完整应用，故该例journal保留属于预期；不能把它当作“完整应用已恢复代理”证据。
- 脚本为隔离测试进程设置新session；生产启动器独立进程组行为另由`guardian_starts_in_its_own_process_group`单测覆盖。脚本没有关闭实际终端窗口。
- 日常v2rayN正在运行，真实系统代理由它持有（127.0.0.1:1080）。测试前后`scutil --proxy`完全一致；没有终止、重配v2rayN或写入其代理设置。

复现命令（仓库根目录）：

```sh
python3 scripts/verify-local-guardian.py \
  --executable target/debug/v2rayr \
  --core apps/desktop/target/debug/v2rayr-core-sing-box \
  --output /tmp/v2rayr-native-cli-evidence
python3 scripts/verify-local-guardian.py \
  --executable apps/desktop/target/debug/v2rayr-desktop \
  --core apps/desktop/target/debug/v2rayr-core-sing-box \
  --output /tmp/v2rayr-native-desktop-evidence
```

脚本仅支持macOS，不是Windows验证脚本。HTTP客户端在拿到完整预期响应后允许对端RST关闭；未收到200及完整标记正文仍失败。

## 3. 原生桌面UI与实际应用连接

采用本次debug可执行文件，建立临时QA `.app` wrapper，唯一bundle ID `com.v2rayr.lifecycleqa`；使用`V2RAYR_HOME=/tmp/v2rayr-local-lifecycle-ui`。此wrapper只用于本机原生UI测试，不作为正式包或安装证据。UI通过原生辅助功能/截图控制，页面为`tauri://localhost`，显示“本地运行”，不是浏览器mock。

观察记录：

1. 首次启动显示未连接；导入`Lifecycle-QA`测试VLESS节点，结果“新增1、重复0、失败0”，节点表显示1个节点。
2. 搜索`Lifecycle`→关闭窗口→激活同一应用：节点页、筛选文字与结果保留。
3. 设置本地端口草稿32108但不保存→关闭窗口→重开：仍显示32108及“当前分组有未保存修改”。撤销后显示0、“已保存”。没有启动该测试端口。
4. 重新启动QA应用后，选择已有本机SOCKS入口1080作为测试上游；将真实sing-box复制到隔离数据目录。明确在连接页选择“手动端口”，点击连接。应用完成真实连通探测，显示“仅本地可用”、sing-box、SOCKS 127.0.0.1:10808、API10812。
5. 进程链：主应用57539→guardian58488→sing-box58489；guardian/core PGID58488，与主应用PGID57539分离。详见`ui-connected-processes.txt`。
6. 经显式HTTP入口10808请求gstatic探测地址，返回204；关闭WebView后再次请求仍返回204。见`ui-proxy-request.txt`、`ui-parked-request.txt`。
7. 激活应用重建窗口后，仍是手动模式、“仅本地可用”，三个PID完全相同，累计流量从2.9KB↓/427B↑更新到8.8KB↓/1.6KB↑；见`ui-reopened-processes.txt`。这不是大流量精度或多日SQLite统计验收。
8. 保持连接按Cmd+Q正常退出：主应用、guardian、core均结束，10808不再监听，journal不存在，系统代理与基线一致。见`ui-exit-result.json`。

注意：外部访问经本机已有v2rayN上游完成，不等同独立测试了新的公网节点服务；没有改变现有系统代理。原生UI测试并未注入权限失败或挂起，不能证明异常情况下10秒上限已在本机实测。

## 4. 发现的未修复问题

**F01：节点页在未连接时“切换”节点，会覆盖接入方式。**

复现：连接页选择“手动端口”→节点页导入/选择另一个SOCKS节点→点击行内“切换”。结果立即发起系统代理/规则连接，返回连接页显示“系统代理”，没有沿用手动模式。

源码依据：`ui/src/pages/Nodes.tsx`的`onUse`，无`activeId`时调用`connect(node.id, "system-proxy", "rule")`。本次该动作发生在尚未为临时QA包配置core时，出现`core_missing`后提前失败，因此没有接管系统代理。后续补齐core后的真实连接测试，改用连接页明确选择手动模式进行，不把这一绕行算作F01已修复。

这是相对“保持已选择的接入方式”的明确行为偏差，可能导致意外接管；本轮只记录、未修改产品代码。Windows手册列为W27，需单独决策/修复后复验。

## 5. 未验收项目与交接

- 本机真实系统代理/PAC写入、所有权交还、权限拒绝、原生恢复挂起及新旧会话写入竞态；现有适配器注入单测不代替这些验收。
- 关闭实际终端、按名字强杀整个应用族、守护进程自身被强杀、固定端口立即重开冲突、确定性恢复失败用户通知。
- 真实TUN接口/路由/DNS、LAN其他物理设备、长时吞吐与资源稳定性、真实订阅/WebDAV/更新服务、正式安装和签名。
- Windows尚未执行；PowerShell采证工具也未在Windows运行。本机没有PowerShell宿主，不能声明其执行或P/Invoke已通过。

Windows交接入口：[Windows验证手册](../../windows-system-test-manual.zh-CN.md)，含W01–W27、构建、WinINet真实测试、采证脚本、故障注入、安装/TUN及救援步骤。Windows必须取得完整未提交源码快照，不能只检出manifest中的HEAD。

本次没有提交代码；保留了原有并行改动。测试进程已退出，现有系统代理未改变。源文件摘要及环境见`manifest.json`；它只记录验证范围，不证明未覆盖功能已通过。
