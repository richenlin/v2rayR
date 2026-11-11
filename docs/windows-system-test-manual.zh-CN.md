# Windows 全应用功能与系统验证手册

版本：2026-11-11（全功能扩展版）。交给 Windows 工程 LLM 逐项执行。当前文档是执行方案，不是 Windows 验收通过记录。

执行范围：第1–4节准备/原生基线 → 第12节全应用功能 → 第5–9节系统与故障专项 → 第10节汇总；第11节为救援。第12节与W01–W27均为必填结果矩阵，之前本机验证过的项目也必须在Windows重新验证。

全量目录：**95项功能用例（F-*）+27项系统用例（W01–W27）+9组设置检查（S01–S09，组内逐字段）=131个验证编号**。编号数量不等于最终子场景数量；协议/core、模式/策略、设置字段和平台版本还需展开子项。完整验收不能仅引用生命周期基本门槛。

| 功能域 | 执行位置 |
|---|---|
| 环境、构建、安装路径、基线、WinINet原生往返 | 第2–4节；首次安装按W21，暂不卸载 |
| 外壳、节点、导入导出、协议/传输/TLS | 12.2–12.3 |
| 连接、分流、DNS、PAC、LAN认证、Dokodemo | 12.4；OS专项第5–9节 |
| 订阅、测速、流量、存储、长时性能、日志 | 12.5–12.6 |
| 备份、WebDAV、core安装、应用更新、签名 | 12.7；安装/TUN第9节 |
| 全部设置字段、CLI、本机历史重验、接口覆盖 | 12.8–12.11 |
| 故障注入、结果汇总、救援 | 第6–8节、第10–11节 |

## 1. 执行约定与范围

- 使用可恢复快照的 Windows 11 x64 虚拟机或专用测试机、专用用户；普通用户运行桌面应用，安装/TUN 授权才提升权限。测试账户必须与采证账户相同；不要用另一个管理员账户的 HKCU 冒充应用用户配置。
- 覆盖完整应用：原生外壳、节点/协议/传输/TLS、订阅、测速、连接/路由/DNS/PAC、LAN认证、Dokodemo、所有设置字段、流量/SQLite/日志、备份/WebDAV、core维护、应用更新、CLI，以及Windows安装/TUN、独立守护和故障恢复。
- 逐项给出 `PASS / FAIL / BLOCKED / NOT-RUN`。构建成功、模拟测试通过、端口能连上、界面显示“已连接”均不能代替真实转发和系统代理恢复证据。禁止把 Unix 专用测试运行出 `0 tests` 当通过。
- 不调整防病毒、UAC 或系统策略以制造通过结果。节点/订阅凭据仅存于本地测试环境；证据对外移交前脱敏。
- 先执行现有代码并保留失败证据；如修复，单独记录补丁、重建全部相关二进制、重跑失败项和受影响项。不得把前后两个版本的结果合成一次通过。
- 每例结束恢复基线；不能恢复时暂停后续网络用例，用第 11 节救援。不得直接删除唯一 journal。

关键边界：WinINet 默认 LAN 代理是产品当前目标，WinHTTP、浏览器私有代理、拨号/VPN 专用代理不等价。Windows 没有本次 Unix SIGHUP/SIGTERM 清理机制；`taskkill /F` 强杀守护本身无法执行清理。不得用本机 macOS 结果代替 Windows 的进程、管道、WinINet 或安装验收。

## 2. 测试版本与准备

需要：当前源码完整快照、Rust MSVC 工具链、Visual Studio C++ 构建工具及 Windows SDK、Node/pnpm、WebView2 Runtime；打包另需仓库 Tauri 流程的安装器工具。按仓库构建输出补齐依赖，不关闭安全校验。

本次 macOS 基线 HEAD 为 `a741f3ee8ba741ddf360cbd0c942bd97a5a0b412`，但功能代码仍有未提交改动。**仅 checkout 这个提交不含本轮修复。** 需取得完整补丁/工作区快照，比较 `docs/reports/2026-11-11-local-lifecycle/manifest.json` 内相关源文件摘要；若后续有改动，重新记录摘要和差异，不能假定同一版本。

在普通 PowerShell 中，从仓库根目录执行；路径按本机修改。以下变量不得命名为 `$PID`、`$HOME` 等系统变量。

```powershell
$ErrorActionPreference = 'Stop'
$Repo = (Get-Location).Path
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Evidence = Join-Path $Repo "test-results\windows-$Stamp"
$TestRoot = Join-Path $env:LOCALAPPDATA "v2rayR-QA\$Stamp"
New-Item -ItemType Directory -Force $Evidence,$TestRoot | Out-Null
$env:V2RAYR_HOME = $TestRoot
Start-Transcript -Path (Join-Path $Evidence 'transcript.txt')
git rev-parse HEAD
git status --short
rustc -vV
cargo --version
node --version
pnpm --version
Get-ComputerInfo | Select-Object WindowsProductName,WindowsVersion,OsBuildNumber,OsArchitecture
```

记录虚拟机快照名、账户、权限、网络拓扑、已运行的其他代理、WebView2 版本。隔离应用目录不会隔离当前用户的系统代理，也不一定隔离 WebView 的本地存储；首次测试用干净用户配置。

### 构建及基础回归

```powershell
cargo test --workspace --locked 2>&1 | Tee-Object (Join-Path $Evidence 'workspace-tests.log')
if ($LASTEXITCODE -ne 0) { throw 'Workspace tests failed' }
pnpm --dir ui install --frozen-lockfile
if ($LASTEXITCODE -ne 0) { throw 'UI install failed' }
pnpm --dir ui check
if ($LASTEXITCODE -ne 0) { throw 'UI type check failed' }
node ui/scripts/verify-lifecycle.mjs
if ($LASTEXITCODE -ne 0) { throw 'UI lifecycle regression failed' }
pnpm --dir ui build
if ($LASTEXITCODE -ne 0) { throw 'UI build failed' }
cargo build --locked -p v2rayr-cli
if ($LASTEXITCODE -ne 0) { throw 'CLI build failed' }
```

桌面按 `scripts/package-desktop.sh` 和 `scripts/stage-desktop-cores.sh` 构建，使用 Git Bash 从仓库根执行：

```bash
bash scripts/package-desktop.sh --config tauri.windows.conf.json
```

失败时保存完整输出，不使用 macOS/Linux 的 embedded-cores。准备脚本会下载并校验 Windows 原生 sidecar、Xray 地理数据和 Windows TUN 组件。源码 workspace 测试与桌面 workspace 独立，必须分别构建。

将真实路径填入下列变量；不要默认安装器文件名就是运行程序名：

```powershell
$CliExe = Join-Path $Repo 'target\debug\v2rayr.exe'
$DesktopExe = 'C:\Program Files\v2rayR\v2rayr-desktop.exe' # 安装后查找并核实
Test-Path $CliExe
Test-Path $DesktopExe
Get-FileHash -Algorithm SHA256 $CliExe,$DesktopExe
Get-ChildItem dist | Select-Object Name,Length
```

如果实际运行文件名不同，修改 `$DesktopExe`。记录安装包、应用、helper、sing-box、Xray 的 SHA-256 和版本。UI 显示必须为真实本地运行，不是浏览器 mock。

## 3. 基线与证据工具

仓库提供只读脚本 `scripts/capture-windows-lifecycle.ps1`，查询真实 WinINet 默认连接的 `flags/server/bypass/pac`，同时采集进程、监听端口、注册表辅助信息及 journal。该脚本在 macOS 无法执行，Windows 工程 LLM 必须先核对/运行它；若 P/Invoke 或查询报错，保留错误并修复采证工具，不能跳过有效值验证。

```powershell
& .\scripts\capture-windows-lifecycle.ps1 `
    -EvidenceDir (Join-Path $Evidence 'baseline') -TestRoot $TestRoot
reg.exe export 'HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings' `
    (Join-Path $Evidence 'internet-settings-before.reg') /y
if ($LASTEXITCODE -ne 0) { throw 'Proxy backup failed' }
```

此备份包含 Connections 等子项，但不是跨用户通用恢复方案。保存 Windows 设置→网络和 Internet→代理的截图。TUN 另保存：

```powershell
Get-NetIPInterface | Export-Clixml (Join-Path $Evidence 'ip-interface-before.xml')
Get-NetRoute | Export-Clixml (Join-Path $Evidence 'routes-before.xml')
Get-DnsClientServerAddress | Export-Clixml (Join-Path $Evidence 'dns-before.xml')
```

每个用例至少采集 `before / connected / after` 三次，使用不同目录。比较 WinINet 的四个字段，不比较 JSON 时间戳。`flags` 位 1=直连、2=手动代理、4=PAC、8=自动检测；必须保留原先的组合状态。注册表与 WinHTTP 查询只能辅助判断，不能替代有效 WinINet 值。

准备一个已知可用、受控的真实节点和测试 URL（最好有服务端访问日志）。先记录直连基线，再用显式代理证明经本机入口访问成功；如要证明流量确实经过远端，检查出口地址或服务端日志。不能只访问一个本来就能直连的网站就宣布全局代理成功。

## 4. Windows 原生适配器自动测试

只在已备份的专用账户执行，期间会短暂把代理改为测试地址，不能同时运行其他接管工具：

```powershell
$env:V2RAYR_TEST_WININET = '1'
cargo test -p v2rayr-platform --locked real_wininet_round_trip -- --ignored --nocapture
if ($LASTEXITCODE -ne 0) { throw 'Native WinINet round trip failed' }
Remove-Item Env:V2RAYR_TEST_WININET
```

预期：真正运行 1 项，包含手动代理与 PAC 的写入/读回/恢复；四字段与基线一致。此测试未验证真实流量或守护交接。

可另运行 `unsafe_bypass_round_trip_keeps_the_other_fields`（同样设置环境变量）。如果基线 bypass 没有 CIDR/IPv6 等特殊内容，这个用例会提前返回，必须记录“特殊分支未覆盖”，不能按用例名字推断覆盖。测试后重新采证。

## 5. 桌面正常路径

从同一个已设置 `$env:V2RAYR_HOME` 的 PowerShell 启动：

```powershell
$DesktopProcess = Start-Process -FilePath $DesktopExe -PassThru
```

| ID | 操作 | 通过标准与证据 |
|---|---|---|
| W01 | 首次启动，导航全部页面；再启动同一 exe、同一数据目录 | 原生 WebView 正常渲染；单实例第二次激活已有窗口；无重复主应用。记录启动耗时与进程。 |
| W02 | 导入真实分享链接、重复链接、无效链接；收藏、筛选；添加/更新/禁用测试订阅 | 数量和错误逐项准确；重复不增殖；失败不破坏原节点；重启后持久化。记录脱敏结果。 |
| W03 | 手动接入，用 sing-box 与 Xray 分别连接并访问测试 URL | 每个 core 实际转发成功，系统代理保持基线。Xray HTTP 和 SOCKS 端口可能不同，必须从界面/配置核实。 |
| W04 | 系统代理接入，全局策略，分别连接两个 core | WinINet 指向实际端口；真实系统代理客户端与显式代理请求都成功；journal 在 `run\takeover.json`；断开后四字段恢复、journal 删除、端口释放。 |
| W05 | PAC 接入，测试匹配规则和直连规则 | PAC URL 可获取且脚本可解析；通过使用系统 PAC 的浏览器验证两类流量；不能用 curl 的显式代理测试代替 PAC 选择。断开恢复原 PAC/手动状态。CLI 当前没有 PAC mode，必须用桌面。 |
| W06 | 保持连接，关闭窗口→托盘重开，重复 20 次 | 主应用/guardian/core 的 PID 与创建时间不变；网络不断；没有新的重复核心；节点页及搜索/筛选恢复；WebView2 可重建但不持续累积。关闭窗口不等于退出。 |
| W07 | 设置本地端口草稿但不保存，关闭窗口重开；然后撤销；再保存非网络偏好并重启 | 草稿恢复且未偷偷应用；撤销回到已保存值；已保存偏好重启保留。测试账户/原生 WebView，不能以 mock 代替。 |
| W08 | 持续下载≥60秒，产生上传；停止请求，切换节点、断开、重启 | 速率、上下行累计合理，已结束请求计入累计；页面刷新不倒退；重开没有重复采样；数据库持久化按当前产品统计口径核对。固定目标文件大小作为对照，不要求协议开销后逐字节相等。 |
| W09 | 接管中从托盘“退出”，而非关闭窗口 | 主应用应在10秒清理等待上限后退出，另留调度容差（实测>12秒判失败）；恢复后的四字段正确；所有本会话子进程结束，监听端口释放。无永久挂起。 |

HTTP 显式代理请求示例（把入口换成实际 HTTP 端口，目标换成受控 URL）：

```powershell
curl.exe --noproxy 'never-bypass.invalid' --proxy 'http://127.0.0.1:10808' --max-time 15 `
    --output NUL --write-out '%{http_code} %{size_download}\n' 'https://你的测试服务/health'
```

curl 的结果只证明显式代理入口，不证明它使用了 WinINet。浏览器验证时关闭浏览器私有代理、记录系统代理路径和测试站出口证据。

### W27：断开状态下的节点切换与接入方式

macOS本轮复现：连接页选择“手动端口”→节点页点击另一节点“切换”，会调用硬编码的`connect(node.id, "system-proxy", "rule")`，改为系统代理/规则模式，而非仅选择节点或沿用接入方式。源码在`ui/src/pages/Nodes.tsx`的`onUse`。此项尚未修复。

Windows必须先在无可用core的隔离配置中验证模式变化，保存前后截图；避免误接管测试账户之外的真实网络。若期望保持模式，则记FAIL。给出产品决策或修复后，再在有core场景验证。本次测试中缺少core使请求提前失败，不能据此宣称不会改系统代理。

## 6. 进程识别与故障矩阵（重点）

先在连接期间采证，确认三层关系：

1. 主应用：路径为 `$DesktopExe`，命令行不含 `--internal-core-guardian`。
2. guardian：同一应用 exe，命令行含 `--internal-core-guardian` 与本次 `$TestRoot\run\takeover.json`，父进程是主应用。
3. core：sing-box 或 Xray，父进程是 guardian。PID 不能只凭名称认定；记录 PID、创建时间、路径、命令行和监听端口。`KernelProcess.pid()` 当前返回 guardian PID。

```powershell
Get-CimInstance Win32_Process |
    Where-Object { $_.Name -match 'v2ray|xray|sing-box' } |
    Format-List ProcessId,ParentProcessId,CreationDate,ExecutablePath,CommandLine
```

手工核实本例 PID，赋给 `$MainProcessId / $GuardianProcessId / $CoreProcessId`。**不要使用 `$PID`，不要让脚本自动挑第一个同名进程。** 每例重新取得 PID，禁止复用前一例 PID。

| ID | 操作 | 必须观察的结果 |
|---|---|---|
| W10 | 分别在系统代理/PAC 模式连接，执行 `taskkill /PID $MainProcessId /F`，**不加 `/T`** | 只杀主应用；guardian 因租约 EOF 接手。20秒内 core/guardian 消失、端口释放、代理恢复、journal 删除；记录0/1/3/10/20秒证据及耗时。不能重开应用帮助恢复后再把结果算成守护成功。 |
| W11 | 只杀 core：`taskkill /PID $CoreProcessId /F` | guardian 返回退出状态；存活主应用报告错误并恢复代理，不停留假“已连接”；允许重试后重新连接。恢复与恢复失败两种状态应有明确区分。 |
| W12 | 保持主应用和租约存活，只杀 guardian | 这是守护自身失效，不是父进程死亡路径；记录 core 是否遗留、主应用是否发现并恢复。Windows 无 Unix 信号清理钩子，不得预填“应自动恢复”。如留孤儿/失效代理，记 FAIL/已知缺口，按第11节恢复。 |
| W13 | 测试副本中先保留损坏 journal，再通过 CLI `--home $TestRoot status` 启动恢复 | 仅用于恢复数据容错，不先接管真实代理。守护遇到不可解析记录应保留；应用启动对损坏 journal 的处理可能不同，分别记录，不把两者混同。禁止破坏本例唯一真实代理备份。 |
| W14 | 连续20次连接→断开，以及10次主进程强杀→等待清理→重开 | 无残留本会话进程/监听、无句柄数单调攀升；采集 HandleCount/进程创建时间。Windows 不以Unix僵尸术语或`kill -0`判定；必要时 Process Explorer 查句柄。 |
| W15 | 强杀后立即重开并自动连接；分别固定10808和自动端口，重复10次 | 测恢复与重连竞态、新连接的代理是否被旧 guardian 关掉、端口占用持续多久。当前没有专门的固定端口短重试，可能出现 `port_in_use`；发生则如实 FAIL/已知缺口，不能延迟重开掩盖。等待旧 core 结束后重试应能恢复。 |
| W16 | 从控制台运行 CLI `run ... --mode manual`，再关闭该控制台；另测Ctrl+C | Windows控制台结束语义单独验收；core/guardian无残留，手动模式不改代理。源码单元测试不代替控制台/ConsoleHost/Windows Terminal行为。 |

进程“消失”应比对 PID+创建时间，避免把复用 PID 当残留；测试URL和入口端口还需再次验证。对于 kill-by-name、任务管理器“结束进程树”、`taskkill /T /F`：仅在可还原快照中做边界测试。它们可能把 guardian 一起杀掉，**不属于已有实现可保证自动恢复的路径**；报告影响并由负责人决定是否阻止发布，不得修改测试命令逃避这一限制。

## 7. 所有权与恢复竞态

### W17：用户接管优先

分别测试手动代理与 PAC：连接后采证，在 Windows 代理设置中把相应地址改成另一受控代理/PAC地址，保存并重新采证；再正常断开或仅强杀主应用。

预期：外部新地址仍保留，应用不能强制改回基线或关闭外部正在使用的模式；未被用户修改且仍属应用的字段可恢复。Windows `server` 是合成字段，不能假定与macOS逐HTTP/HTTPS槽位完全等价。比对 `native.before/written` 与有效 WinINet 值，按字段组验证。结束后人工恢复基线。

### W18：journal 被新会话替换

先跑 `cargo test -p v2rayr-engine --locked retry_does_not_restore_replaced_journal -- --nocapture`。预期确实执行测试；这是适配器注入测试，不是真实注册表竞态证明。

系统级重现需在测试分支为代理恢复适配器加可控制的“一次失败+屏障”，记录补丁与二进制摘要。步骤：旧会话EOF→首次恢复失败并暂停→新会话实际接管并替换journal→放行旧guardian重试。预期旧guardian不再调用恢复，新代理保持，新journal未删除；旧core仍须回收。另测core在等待恢复时自行退出。禁止仅写两个JSON文件就宣称覆盖了原生代理恢复；无法可靠注入则标BLOCKED，不用概率性快速点击代替。

字节相等检查不能提供跨进程事务锁；应另观察“检查之后、原生写入之前”以及“读到journal之前已被替换”的窗口。如果能复现仍误恢复，作为新缺陷记录，不能因已有单测通过而关闭。

## 8. 有界退出与恢复失败

### W19：确定性恢复失败

在可恢复的测试分支注入代理恢复返回错误，不要修改HKCU ACL或禁用安全策略。连接真实代理后触发退出，记录：主应用是否有界退出、guardian重试次数、journal是否保留、core是否结束、网络是否受影响。当前实现退出时没有专门的确定性失败确认/系统通知；因此“用户无提示”应作为已知产品缺口记录，不把日志等同用户通知。

### W20：恢复挂起

注入一次可控阻塞的恢复调用，记录开始时间、主应用退出时间、guardian退出时间。当前代码主应用最多等待10秒，guardian给恢复任务8秒预算，再清理core（Unix另有2秒TERM宽限；Windows走Child.kill）。Windows实测验收须确认阻塞线程、系统调用及管道都不阻止进程结束；不可把代码里的timeout当实测。测试后恢复基线，再验证journal仍可用于启动恢复。

失败注入只说明错误路径，不代替W04/W05/W10真实成功恢复。清楚区分普通断开失败时保留服务的策略和最终退出时停止服务的策略。

## 9. 安装、TUN与维护扩展

这些项必须单独汇报，guardian通过不代表它们通过。

| ID | 执行 | 验收与边界 |
|---|---|---|
| W21 | MSI、NSIS分别在干净快照安装、启动、卸载；普通用户运行 | 文件路径/权限正确，无意外UAC；WebView2与sidecar就绪；卸载无活动会话残留；用户数据保留/删除符合安装器实际选项。 |
| W22 | 管理员PowerShell运行 `scripts/test-windows-tun-package.ps1` | 脚本会安装/卸载两类安装器并故意破坏Wintun DLL再修复，必须在一次性环境。要求dist中恰有一个MSI、一个NSIS安装包。返回码、受信路径/ACL/摘要校验及修复全部留证。 |
| W23 | 默认Program Files安装后，固定IPv4 sing-box节点开启全局TUN，先拒绝UAC再重试 | 普通GUI不提权；仅固定helper授权；拒绝不留下路由/DNS；成功后TCP/UDP、IPv4/IPv6、DNS真实转发。Windows当前不以macOS应用规则分流能力作为预期。 |
| W24 | TUN中分别正常断开、只杀GUI、只杀helper、只杀节点core；另测网卡切换 | 对比接口GUID、地址族、路由、DNS和WeakHostSend；允许保护性恢复记录留存但未恢复项不能算PASS。helper强杀后可能需后续恢复及UAC，不能冒充即时自动恢复。见`docs/tun-desktop.md`。 |
| W25 | 开启/关闭登录启动，在专用账户真实注销登录；测试connect-on-launch | Run项指向已安装路径，隔离测试环境变量未丢失或使用专用账户默认数据目录；不启动重复主实例；恢复旧journal先于新接管。 |
| W26 | 备份导出/预览/应用；升级和日志维护 | 仅测试产品实际支持字段，备份不能假定含所有节点/密钥；在线升级需受信测试更新源及签名密钥。没有源/签名则BLOCKED。日志轮转用测试文件，不覆盖真实故障日志。 |

`native_tun` 现有自动测试需按 `docs/tun-desktop.md` 在专用机设置隔离标记与实际绑定IP后运行；它不能代替W23/W24全矩阵。不得在Windows执行 `real_connection.rs` 然后把`0 tests`当本地节点链路通过，该文件当前为Unix专用。

## 10. 结果模板与交付要求

每例一行结果索引，并附详细记录：

```text
用例ID/标题：
状态：PASS | FAIL | BLOCKED | NOT-RUN
OS/账户权限/网络拓扑/VM快照：
源码HEAD + dirty补丁摘要 / 应用及core SHA256：
模式/core/节点标识（脱敏）/端口：
步骤与实际命令：
开始/主应用退出/代理恢复/core退出/guardian退出时间：
预期结果：
实际结果：
证据：before/connected/after目录、截图、服务端访问日志、错误日志
清理：代理四字段与原基线对比；残留进程/端口/路由/DNS检查
失败复现率与问题编号：
```

交付：`windows-test-report.md`、矩阵CSV、原始命令输出、WinINet前后JSON、进程与监听快照、截图、安装与UAC日志、构建与二进制摘要、失败最小复现。屏障/失败注入补丁单独保存。汇总时逐项说明覆盖层次：单元模拟、真实core、真实WinINet写入、已安装桌面、真实外部网络。

W01–W11、W14、W17只是生命周期基本门槛，不能代表应用完整验收；全量门槛见12.11。所有故障/竞态和完整功能项必须有明确结果或阻塞原因。已知缺口（按名称强杀、固定端口立即重开、退出失败提示）必须显式列出；是否接受需负责人决定，工程LLM不能自行删掉失败项宣布全量验收。

## 11. 失败后的本机救援

1. 停止自动重连，保留journal、日志、WinINet有效值和进程证据。不要继续批量测试。
2. 核对本次PID+创建时间+路径，停止测试主应用；先给guardian最多20秒恢复，勿立即用进程树强杀。
3. 若仍有本例core残留，只结束已经核实的本例PID；保留其他程序的进程。
4. 通过Windows代理设置恢复**测试前**手动代理、PAC、自动检测和绕过列表；无原代理的基线才可全部关闭。测试前本来有企业代理时，禁止一键“全部关闭”。
5. 用只读脚本重新采证，确认WinINet四字段。无法可靠人工恢复时，从同一账户导入已保存的reg备份、注销再登录并复查，或还原VM快照。不要把`netsh winhttp reset proxy`当WinINet恢复，它不是本应用的代理层。
6. TUN异常按`docs/tun-desktop.md`及helper受保护恢复流程处理，保存路由/DNS/WeakHostSend证据，不随意删除系统路由。无法恢复则回滚VM快照。
7. 确认基线网络恢复、无本例残留进程/监听，最后再清理测试环境变量并结束记录：

```powershell
Remove-Item Env:V2RAYR_HOME -ErrorAction SilentlyContinue
Remove-Item Env:V2RAYR_TEST_WININET -ErrorAction SilentlyContinue
Stop-Transcript
```

## 12. 全应用功能检测（Windows必须重新执行）

本节与W01–W27共同构成完整验证范围，不能只执行退出/守护矩阵。**macOS已通过的功能也必须在Windows重新执行；macOS报告只提供测试设计与历史对照，不给Windows用例预填PASS。** 每个ID有独立结果；同一行列出的子场景也要逐条记录，不能用其中一个成功代表整行成功。

### 12.1 测试资源、覆盖层级与执行次序

先准备以下测试资源并登记`fixtures-manifest.json`：资源ID、用途、服务地址、服务/内核版本、配置摘要、有效期、预期返回/出口、是否涉及真实系统写入。凭据另存，不进入公共报告。

| 资源 | 必须准备的内容 |
|---|---|
| R-HTTP | 受控HTTP/HTTPS服务，支持固定正文下载、已知大小上传、延时、超时、重定向、4xx/5xx；保留请求与出口日志。 |
| R-PROXY | 按12.3准备的真实协议服务；至少sing-box和Xray各一条可工作的链路，UDP专用回显服务、IPv6及域名测试资源。 |
| R-SUB | 可切换内容的订阅服务：原始链接、base64、Clash YAML、sing-box JSON；具备版本A/B、空列表、损坏内容、超时、401、配额/到期响应头。 |
| R-ROUTE | 三组能区分direct/proxy/block的受控域名/IP；直连与代理出口可区分；可观测的DNS服务；PAC浏览器测试配置。 |
| R-LAN | 第二台实际设备，和测试机处于专用可信网段；不能用同机回环请求替代跨设备验收。 |
| R-DAV | 可信HTTPS WebDAV文件端点，支持认证、GET/PUT、If-None-Match/If-Match及强ETag；可制造409/412、重定向及证书错误。 |
| R-UPDATE | 受信测试更新源、签名正确的新版本包、错误签名/篡改包、同版本与旧版本响应；专用快照允许安装回滚。 |
| R-STATE | 可还原的数据目录/VM快照；旧schema、损坏settings/数据库、端口占用器、大节点集及已知流量基线。 |

顺序：准备和基线→原生适配器测试→本节正常功能→W01–W27故障/系统专项→长时稳定性→基线恢复→完整报告。每一网络写入例结束检查系统代理；TUN例另检查路由/DNS/WeakHostSend。

覆盖标记必须分开：`PARSE`导入解析、`CONFIG`真实core接受配置、`FLOW`真实数据转发、`OS`系统接管/恢复、`UI`原生桌面、`STORE`持久化。例如“VLESS CONFIG通过、FLOW缺服务”应记录FLOW=BLOCKED，不能写协议全通过。没有可用服务必须明确资源阻塞，工程LLM不得创建一个永远返回成功的mock来替代验收。

本机已验证过的LAN认证、备份合并/WebDAV、临时签名更新包等历史记录（见`docs/delivery-status.md`与`docs/signed-release.md`）同样不能免除本节的Windows重验。历史文档中若存在不同阶段的状态，以当前源码与本次实测逐项判断，不能直接复制旧结论。

实际能力以当前源码`ui/src/lib/api.ts`、`crates/ipc/src/settings.rs`、`crates/config/src/capability.rs`及对应业务实现为准。`docs/config.md`部分内容属于规划，不得把规划中的全局热键、任意脚本、完整密钥备份等当已实现功能，也不得因界面缺少规划控件误报回归。

### 12.2 原生外壳、节点与导入导出

| ID | 操作/数据 | 预期与必须留存的证据 |
|---|---|---|
| F-UI01 | 干净账户首次启动、无节点/无订阅启动；各页面导航、刷新、重开 | 无白屏、崩溃或mock数据；空态和按钮状态准确；真实版本/schema信息可查询；与W01结合留截图。 |
| F-UI02 | 同一数据目录第二次启动；另用独立目录运行测试实例 | 同目录激活已有实例，不重复后台服务；独立目录行为记录；目录隔离不等于系统代理隔离，两实例不得同时接管真实系统代理。 |
| F-UI03 | 100%/150%/200%缩放、最小窗口、最大化、窗口调整与多显示器切换 | 主要按钮/弹窗/表格可操作，无遮盖截断；键盘Tab、Enter、Esc、焦点可见；高DPI跨屏无无法点击区域。 |
| F-UI04 | 系统浅/深色与应用浅/深/跟随系统，中英文/跟随系统 | 所有五页、弹窗、空态、错误信息切换正确；重启保留；紧凑布局/减少动效行为可观察，无仅保存但不生效的假开关。 |
| F-UI05 | 关闭到托盘开/关两种设置；最小化、托盘显示/退出、多次重建WebView | 窗口关闭与退出语义一致；关闭到托盘时后台连接持续；页面、筛选、草稿和流量恢复；关联W06/W07/W09，20次循环留PID/句柄证据。 |
| F-N01 | 单链接、多行、空行、BOM、中文/空格/百分号编码名称；空输入 | 有效项正确展示协议/地址/名称；无效项定位明确，空输入无幽灵节点；原始秘密不进入错误提示。 |
| F-N02 | 重复导入同链接、同连接参数不同名称、参数顺序变化；再次重启 | 按连接指纹去重，数量符合真实导入报告；收藏/标签/用户选择不因去重被悄悄覆盖；记录前后ID和数量。 |
| F-N03 | 原始链接列表、标准/URL-safe base64、Clash YAML、sing-box JSON分别导入 | 支持格式成功解析；字段、TLS和传输参数不丢失；格式不支持项明确报错，不能静默制造可连接假节点。 |
| F-N04 | 混合有效/无效/未知scheme/缺UUID或密码/非法端口或TLS参数 | 合法项与失败数准确；不得用默默丢弃替代失败报告；错误文本/日志脱敏；失败后已有数据保持。 |
| F-N05 | 导入普通HTTP(S)URL，分别不开启/开启“作为代理节点” | 默认作为订阅入口且有明确行为；代理节点选项生成HTTP/HTTPS代理语义；带不支持路径的代理链接拒绝；不把订阅token当代理密码。 |
| F-N06 | 搜索名称/地址/协议/标签；协议筛选、全部/收藏组合；零匹配与清空 | 结果集合正确、计数准确；搜索大小写/中文行为一致；关闭重开恢复筛选；不影响未显示节点。 |
| F-N07 | 收藏/取消、编辑多标签/空标签、复制分享链接并重新导入 | 标记和标签持久化；导出再解析连接参数一致且去重；复制行为用户明确触发，秘密只按分享用途出现。 |
| F-N08 | 删除普通、收藏、订阅来源及当前选用节点；取消删除 | 取消无变化；确认后数据/计数一致；当前会话处置明确且无孤儿/失效系统代理；订阅下次更新重现行为按来源规则记录。 |
| F-N09 | 断开状态与连接中分别切换节点；同节点重复点击；快速连点 | 断开/连接行为符合明示动作；模式/策略不意外覆盖；连接中验证新节点失败时恢复原可用会话；W27问题必须复验。 |
| F-N10 | 1,000与10,000节点集，滚动至首/中/尾、过滤后操作指定项 | 虚拟列表无错行操作，选中ID准确；无冻结/长时内存累积；记录导入/过滤/滚动耗时与资源峰值，不仅截图首屏。 |
| F-N11 | 启动参数传入分享链接及`v2rayr://import/<base64>`；应用未运行/已运行两种 | 已实现的参数入口能够导入/提示；未注册的OS协议不得伪称可从浏览器唤醒。单独记录安装器是否注册URL scheme、已有实例是否转交参数。 |

### 12.3 协议、传输、TLS及core兼容性

每条正向协议例都需：导入→检查保密存储/显示→生成配置由真实core校验→连接→通过已知正文/出口验证→断开→导出再导入。除明确负向例外，缺少真实服务就标FLOW=BLOCKED。可用本地Windows原生core搭建隔离服务；**不能将Unix专用`real_connection.rs`的零测试输出当Windows链路证据**。

| ID | 节点组合 | 必测结果 |
|---|---|---|
| F-P01 | VLESS普通TCP，sing-box自动选择及显式Xray | 两core分别真实转发；验证不同HTTP/SOCKS入口与回滚；覆盖本机已测VLESS链路在Windows的重验。 |
| F-P02 | VLESS+TLS/Reality，含SNI、公钥、shortId、flow；各一个错误参数 | 支持组合正常；错误证书/参数失败且不接管系统代理；不依赖关闭证书校验通过。 |
| F-P03 | VLESS xhttp、mKCP、支持的VLESS encryption参数 | 自动选择Xray；真实core与服务兼容；强制sing-box明确拒绝而非忽略参数；分别登记服务版本。 |
| F-P04 | VMess TCP及至少一种非TCP传输 | UUID/加密参数保留；sing-box/Xray按能力成功或明确拒绝；错误身份验证不能显示成功接管。 |
| F-P05 | Trojan/Trojan-go链接，TLS、WS等声明支持组合 | 密码/SNI/传输字段一致；正确凭据成功、错误凭据失败；别名解析不等于实现了所有trojan-go专有特性。 |
| F-P06 | Shadowsocks传统AEAD与2022、标准与base64链接编码 | 正确方法/密钥转发；2022自动选sing-box；不兼容core与未知method被拒绝；包含密钥长度负向例。 |
| F-P07 | SOCKS5无认证/有认证，SOCKS4链接 | 分别验证实际支持能力；错误认证拒绝；SOCKS4仅解析成功不代表core兼容，按capability记录明确结果。 |
| F-P08 | HTTP与HTTPS代理，认证/无认证，代理证书错误 | `http-proxy://`、`https-proxy://`及UI转换语义正确；证书/认证失败不假成功；记录真实HTTP CONNECT链路。 |
| F-P09 | Hysteria2/`hy2`，含obfs/带宽等节点参数 | sing-box真实QUIC链路与UDP验证；错误密码/obfs失败；强制Xray拒绝而不吞参数。 |
| F-P10 | TUIC，UUID/password及UDP relay参数 | sing-box链路正确，TCP/UDP分开留证；不兼容core拒绝。 |
| F-P11 | AnyTLS节点，正确与错误身份 | 自动core及TLS参数正确；真实服务转发与失败路径都有证据。 |
| F-P12 | WireGuard/`wg`，密钥、peer、allowed IP及端点参数 | 真实支持core按能力转发；接口/路由副作用可恢复；缺资源只记阻塞，不以成功导入代替。 |
| F-P13 | Naive/naive+https/naive+quic、chain、自定义payload | 解析、导出、执行能力分开判断；当前无受支持执行路线的组合明确拒绝，不出现“已连接”；不要求尚未实现的链式代理成功。 |
| F-P14 | Raw、WS path/Host/early-data、gRPC serviceName、HTTPUpgrade、HTTP/H2；xhttp/mKCP/QUIC补充组合 | 按`capability.rs`各支持组合执行CONFIG与FLOW；每种传输至少一条可用及一条非法配置；不能用一种TCP通过代表所有传输。 |
| F-P15 | IPv4、域名、IPv6服务端，端口缺省/边界，中文备注 | 支持地址正确解析连接，IPv6格式/导出不损坏；失败诊断可理解，不产生错误监听或路由回环。 |
| F-P16 | 自动/强制sing-box/Xray/mihomo；缺失core、无执行权限/被隔离、版本不兼容 | 选择理由与实际进程一致；不能满足协议组合时拒绝；mihomo未内置应明确缺失，可在取得可信Windows内核后验证，不能用自动替换冒充强制成功。 |

### 12.4 连接、分流、DNS、PAC、LAN及本地转发

四种接入方式`manual/system-proxy/pac/tun`与三种策略`rule/global/direct`建立12格矩阵：每格记录core、配置、真实出口、系统接管状态。支持格需真实验证；不支持格必须明确拒绝且无副作用。不要把TUN规则分流在macOS的支持照搬到Windows。

| ID | 操作/数据 | 预期与证据 |
|---|---|---|
| F-C01 | 干净默认配置导入→连接→真实访问→断开，sing-box/Xray分别执行 | 新用户路径完整；探测不成功不能提前接管；journal、实际端口、代理和UI一致；W03–W05提供OS部分。 |
| F-C02 | 每种接入方式下切换rule/global/direct，连接中切换接入方式 | 正确重连/恢复旧设置；direct/proxy出口分辨明确，block目标确实失败；旧core/监听不残留。 |
| F-C03 | 不可达节点、错误密码、探测URL失败、core启动即退出、API未就绪 | 正确错误码、可重试状态和诊断；不显示假成功；保留旧可用连接或清理新资源；重试后能恢复。 |
| F-C04 | 连接与断开期间重复点击、切换节点、保存设置、关闭窗口 | 串行化操作不死锁、不双core；最终UI/OS/进程一致；失败和取消不能遗留端口/PAC。 |
| F-C05 | 自动端口已占10808；固定端口占用；HTTP/API辅助端口冲突 | 自动模式选择可用端口并展示；固定模式报告`port_in_use`不静默改端口；TCP/UDP占用分别验证；释放后重试成功。 |
| F-C06 | 应用连接期间休眠/唤醒、网络断开/恢复、Wi-Fi/有线切换 | 无假连接或永久阻塞，错误及恢复行为明确；恢复后真实请求成功；代理和路由归属不丢失。 |
| F-R01 | 开/关内置规则集，对大陆/非大陆与自定义例外目标请求 | 观察真实出口与生成规则，开关真实生效；资源缺失/损坏给出错误或明示回退，不伪称规则已加载。 |
| F-R02 | 广告阻断开/关，访问受控匹配目标和普通目标 | 匹配项按规则阻断，普通请求不误伤；没有可用真实规则资源时标BLOCKED。 |
| F-R03 | 新增、编辑、启用/禁用、删除domain/domain-suffix/ip-cidr/process/geo-ip/geo-site规则 | 六种kind分别测；proxy/direct/block三种action分别测；重启持久化；连接中编辑与下次连接生效边界符合提示。 |
| F-R04 | 精确域名与后缀、IPv4/IPv6 CIDR、冲突规则、空值/非法CIDR/动作/重复项 | 精确匹配不误命中子域，后缀不误命中相似域；实际优先级按生成顺序核对；拒绝非法值而不破坏已有规则。若无排序UI，不要求不存在的拖拽排序。 |
| F-R05 | Windows选择应用路径，路径带空格/中文；文件更新/移动；process匹配 | 选择结果身份正确，失效身份报错；当前平台未部署的应用身份接管不能静默退化成全机规则，须拒绝或明示限制。 |
| F-R06 | 默认DNS与自定义DNS；受控A/AAAA、超时及不可达解析器 | 核对生成配置和真实解析路径/出口，无意外明文泄漏或循环；错误DNS能回滚/恢复，不能只看输入框保存。 |
| F-R07 | prefer_ipv4/prefer_ipv6/ipv4_only/ipv6_only，空/非法/过多DNS服务器 | 双栈结果符合所选策略，单栈不误报另一栈可用；校验错误不修改活动会话。仅后端暴露字段按12.8执行，不编造UI控件。 |
| F-R08 | PAC资源更新成功/网络失败/损坏内容；PAC默认/自定义规则/直连与阻断 | 更新数量/错误准确、失败保留旧可用资源；使用系统PAC的浏览器验证，PAC请求失败的行为不可悄悄变成全局直连。 |
| F-L01 | LAN默认关闭，外部设备探测监听；开启绑定指定私有IPv4 | 默认仅loopback；开启后只绑定所选可信地址，不变成0.0.0.0/所有接口；显示实际独立HTTP/SOCKS端口。 |
| F-L02 | 第二台设备经LAN HTTP入口请求，无认证/错误认证/正确认证 | 前两者拒绝（HTTP典型407），正确凭据实际转发；sing-box与Xray分别验证；不能只测登录握手。 |
| F-L03 | 第二台设备经SOCKS入口，无认证/错误认证/正确认证并完成请求 | 禁止免密，正确凭据TCP转发；UDP是否支持单独实测；密码只在明确请求时显示，普通快照/日志不泄漏。 |
| F-L04 | 断开/重连/切换节点、绑定网卡消失；旧凭据重用 | 监听及会话凭据回收；新会话凭据变化，旧凭据失效；断开后LAN不可用；错误绑定不扩大监听范围。 |
| F-D01 | Xray Dokodemo开启，指定本地端口和受控目标端口 | TCP/UDP按实际支持分别验证转发；普通代理入口仍可用；生成Xray配置与目标一致；关闭后监听释放。 |
| F-D02 | Dokodemo非法域名/端口、目标端口0、与混合/API冲突、强制sing-box/TUN组合 | 校验明确拒绝或报告占用；不产生半连接/孤儿；不自动把不兼容组合改成另一模式。 |

TUN的TCP/UDP、双栈、DNS、UAC、helper身份、恢复记录、WeakHostSend及网络切换继续执行W21–W24；本节未重复它们不代表可跳过。

### 12.5 订阅、自动更新与延迟测试

订阅服务至少提供版本A三条节点，版本B保留一条、修改一条、删除一条、新增一条；其中一条提前收藏并加标签。记录ID/指纹/来源，区分“服务端改名”和“连接参数改变”。

| ID | 操作/数据 | 预期与证据 |
|---|---|---|
| F-S01 | 添加名称/URL、首次拉取、同URL重复添加；取消添加；非法URL | 列表与节点数正确；重复与错误处理明确；取消无写入；地址/token不进入不必要日志。 |
| F-S02 | R-SUB四种格式分别拉取，再从A切换到B更新 | 支持内容完整合并、去重；保留用户收藏/标签语义；来源关系正确；不把一个订阅的删除传播到其他来源。 |
| F-S03 | 上游删除节点，测试普通/收藏及keepRemovedNodes两种设置 | 默认删除策略与收藏保护符合实现；开启保留策略后丢失项保留；逐项记录而非只对比总数。 |
| F-S04 | 200空列表、损坏内容、401/403、404、500、超时、断网及恢复 | 错误状态可见且脱敏；失败不清空原有可用节点；空订阅和解析失败不能混淆；恢复重试成功。 |
| F-S05 | 单条手动更新、启用/禁用、删除确认/取消 | 禁用后不自动更新但数据保留；删除行为与提示一致，来源/收藏保留策略有证据；取消不删；无重复请求风暴。 |
| F-S06 | 全局间隔1/12/168小时、启动更新开/关、到期调度；重启与睡眠唤醒 | next/last时间和实际请求一致；禁用项不调度；自动更新不能凭单次手动更新代替。长周期可受控时钟/测试注入并标注，至少真实等待一次可行短周期。 |
| F-S07 | 服务返回配额、上传下载、到期字段；缺字段、过期与非法值 | 流量/额度/日期显示正确，无负数溢出/NaN，缺失显示未知；不能从本地流量统计冒充服务商额度。 |
| F-S08 | concurrency/timeout/userAgent不同值，多条慢订阅，并测试代理回退 | 测服务端请求日志及并发峰值、UA、超时；直连失败后的既有代理路径按实现验证；不要求未实现的任意自定义请求头/转换服务。 |
| F-T01 | 测速页空数据、全量、指定节点CLI测试，成功/拒绝/超时混合 | 展示TCP握手方法和时间，失败原因可读；不能把TCP延迟当HTTP健康/带宽/真实代理可用性。 |
| F-T02 | 多节点并发测试，测试中重按按钮、离开页面/关窗重开 | 无重复任务风暴；结果更新到正确节点，时间/方法持久化符合实现；不要要求未实现的测速停止或下载测速按钮。 |
| F-T03 | 连接期间测速、更新订阅并产生真实流量 | 测速不切换当前节点/系统代理；节点合并与活动会话一致；过期响应不能覆盖较新的节点/流量状态。 |

### 12.6 流量、存储、性能及数据恢复

| ID | 操作/数据 | 预期与证据 |
|---|---|---|
| F-V01 | 两core各下载已知大小文件及上传；包含持续连接与已结束短连接 | 上下行速率/累计与实际传输同方向且合理；短连接结束仍计入累计；测量协议开销和采样窗口，不按载荷逐字节苛求。 |
| F-V02 | 连接空闲10分钟；后台/前台/重建WebView，重复20次 | 无网络流量时不持续制造无意义采样写入；WAL/DB大小、CPU和core控制API请求频率可观察；不以截图0B/s代替。 |
| F-V03 | 切换节点/core、断开重连、正常退出重启及强杀后重启 | 统计不负数、不回卷、不重复累加；历史与累计跨重启按产品口径保存；崩溃时未提交采样的损失量明确记录。 |
| F-V04 | 制造旧样本执行保留清理，切换本地日期/跨午夜 | recent顺序、时间窗和累计口径正确；修剪不删除总量或近期数据；仅在测试副本进行时间/样本注入，记录补丁。 |
| F-V05 | 慢/挂起控制API，大量节点快照，正在断开或切换节点 | 统计探针不阻塞断开/切换；晚到快照不覆盖新流量；超时/重试有界，无持续占锁。 |
| F-V06 | 从旧schema数据副本升级；损坏settings、未来schema、只读目录/磁盘空间不足 | 迁移/保留/拒绝策略符合实际实现；不静默覆盖未来版本配置；错误可读且不丢原始文件。损坏DB不是自动修复承诺，必须安全失败并留证。 |
| F-V07 | 两个进程争用同目录、强杀后再次打开、路径带中文/空格 | 锁有效、无SQLite损坏；锁释放与journal恢复有证据；当前真实数据目录、DB与settings路径一致。 |
| F-V08 | 两core分别持续运行≥2小时，转发/空闲交替、20次切换及20次托盘重开 | CPU、内存、句柄、线程、文件增长有时间序列；无稳定增长泄漏/网络失效/重复进程。只执行几次短测不能写长稳PASS。 |
| F-V09 | 日志默认级别、临时调试级别、大日志再次启动core、日志尾部读取 | 应用/core日志可定位错误；轮转到`.1`等实际策略生效，非每次连接清空；秘密不泄漏；CLI logs仅当前进程日志语义要与磁盘文件区别。 |
| F-V10 | 检查DB payload实际scheme、运行配置/备份/日志中的测试密码，重启后凭据可用 | 当前默认可能为`v2rayr:plain:`（base64封装，不是加密）；启用`sealed-payloads`才有AES-GCM路线。记录构建feature和实际scheme，禁止把“文本搜索不到密码”当作已加密；运行配置不可作为公开证据。 |
| F-V11 | 仅在副本中测试缺失/损坏secrets、权限异常、跨账户复制数据 | 不无提示丢失原数据或假恢复；正确凭据重启可用；密钥重新生成但旧payload不可读需记录失败，不假设已实现DPAPI/跨用户密钥迁移；默认明文封装与加密构建分别报告。 |

### 12.7 本地备份、WebDAV、内核维护及应用更新

| ID | 操作/数据 | 预期与证据 |
|---|---|---|
| F-B01 | 设置非默认外观/规则/周期→导出→改动→预览→取消；再预览确认应用 | 取消无变化、预览不写入；确认仅恢复白名单偏好及自定义规则；保存后重启核实。备份不承诺节点/订阅凭据、数据库、下载core、运行journal或密钥迁移。 |
| F-B02 | 重复导入同备份、已有规则冲突、禁用规则/应用身份规则、连接中应用 | 合并去重/保留本地策略有证据；连接中受保护，不能边恢复偏好边破坏代理；规则身份跨平台不适用应明确提示而非假可执行。 |
| F-B03 | 损坏JSON、未知字段/版本、>4MiB、>10,000规则、非法周期；目标文件已存在/文件选择取消 | 按当前验证边界拒绝，已有配置不变；本地导出不静默覆盖既有文件；取消无多余文件。 |
| F-B04 | HTTPS WebDAV创建新文件、读取预览、确认应用；正确/错误/缺失凭据 | 数据语义与本地备份相同，认证/错误码正确；读取不是自动应用；请求秘密不落settings、日志或备份。 |
| F-B05 | 已有远程文件创建、CLI强ETag覆盖、过期/弱ETag、服务端并发修改 | 默认If-None-Match防覆盖；强ETag匹配才覆盖，过期返回冲突且不覆盖；弱/格式错误拒绝；保留服务端请求头与版本证据。 |
| F-B06 | HTTP而非HTTPS、URL含user-info/query/fragment、重定向、证书错误、超大/中途断流响应 | 当前限制明确拒绝，不绕过证书验证、不跟随重定向泄漏认证；失败不覆盖本地/远程已有备份。 |
| F-K01 | 查询核心状态/版本/路径，自动发现内置和用户目录core | Windows `.exe`及实际架构识别正确，路径/版本准确；未知/坏二进制不会被报成可用；UI状态与真实进程一致。 |
| F-K02 | 缺core连接→错误处安装→重试；设置页安装/重新探测 | 成功下载经摘要校验后可连接；取消/网络失败保留已有可用版本；不因成功下载但未执行而报告可用。 |
| F-K03 | 默认受信下载、自定义URL无摘要/错误摘要/正确摘要，损坏包、磁盘不可写 | 校验失败拒绝执行/覆盖旧core；自定义来源必须满足实现要求；Windows地理数据/许可文件齐全。仅在专用快照运行受信测试包。 |
| F-K04 | 内置与自定义core优先级、升级后路径、Windows Defender隔离文件 | 显示实际选中路径；不可执行提供诊断；不关闭Defender绕过；mihomo按真实供应渠道/能力验证。 |
| F-U01 | 无更新配置、已最新、有效新版本、检查超时/服务错误 | 无配置时按UI隐藏/明确不可用；不误报最新版；检查不自动安装；失败不破坏连接。 |
| F-U02 | 用户取消/确认安装，分别连接中与断开时 | 取消不安装；确认后先断开恢复网络再运行安装器；重启后版本/数据/设置正确；安装需要的UAC由用户授权。 |
| F-U03 | 错误签名/篡改包、清单与目标版本不一致、旧版本/同版本、下载中断 | 受信链和版本绑定生效，拒绝不可验证包；失败保留旧可启动版本和数据；缺R-UPDATE则BLOCKED。 |

### 安装包身份与更新签名的区别

| ID | 操作/数据 | 预期与证据 |
|---|---|---|
| F-I01 | MSI/NSIS、应用exe、helper分别查询Authenticode，比较发布摘要；普通本地构建与正式候选包分开记录 | `Get-AuthenticodeSignature`记录状态/签名者/时间戳；正式候选包应符合`docs/signed-release.md`的签名要求，缺正式证书不能冒称签名通过。Tauri更新签名与Authenticode两条链分别验证，helper归档签名不等同helper可执行文件签名。 |

WebDAV CLI模板（在专用CLI数据目录执行，密码由环境注入，完成后清除）：

```powershell
$CliRoot = Join-Path $env:LOCALAPPDATA "v2rayR-QA\cli-$Stamp"
New-Item -ItemType Directory -Force $CliRoot | Out-Null
& $CliExe --home $CliRoot backup export (Join-Path $Evidence 'preferences.json')
& $CliExe --home $CliRoot backup import (Join-Path $Evidence 'preferences.json')
# 确认预览后才应用
& $CliExe --home $CliRoot backup import (Join-Path $Evidence 'preferences.json') --apply
# 用受控HTTPS端点替换示例；真实密码不要写进命令行或报告
$env:V2RAYR_WEBDAV_PASSWORD = '<测试凭据，通过本机安全方式注入>'
& $CliExe --home $CliRoot backup push (Join-Path $Evidence 'preferences.json') 'https://受控服务/qa.json' --username qa
& $CliExe --home $CliRoot backup pull 'https://受控服务/qa.json' (Join-Path $Evidence 'downloaded-new.json') --username qa
Remove-Item Env:V2RAYR_WEBDAV_PASSWORD
```

`$CliRoot`是独立CLI测试目录，12.9继续使用；示例域名不是可用测试服务。`backup pull`目标必须不存在；不要用同一文件覆盖源证据。ETag覆盖使用CLI的`--if-match`，从R-DAV实际响应获取强ETag，不能手写一个猜测值。

### 12.8 设置逐字段检查（包括无常驻UI的后端字段）

所有字段均执行：默认值→合法修改→保存读回→相关配置/行为生效→重启保留→恢复默认。数字/枚举执行边界和非法值；连接中可变更项还要验证候选失败时恢复旧设置/会话，不能只检查JSON。

仅后端/配置提供而当前UI未显示的字段，不要求凭空增加控件：在停止应用、备份测试配置后修改隔离目录的`config/settings.json`并重新启动，验证加载/生效；**动态保存和校验**用同版本Windows应用层集成测试调用`save_settings_and_apply`等正式API，或既有真实IPC测试工具，记录调用和返回。不得临时给发行应用暴露未认证远程调试端口。若字段当前没有消费路线，记录“字段可保存但未生效/不支持”，不能据此写PASS。

每个S-ID下面每个字段单独出结果（建议`S01.launchAtLogin`格式），避免一行一项通过覆盖整组。

| ID | 组/字段（JSON camelCase） | 必测行为/边界 |
|---|---|---|
| S01 | startup：launchAtLogin、startMinimized、closeToTray、showTrayIcon、singleInstance、connectOnLaunch | 注册/移除当前用户Run项；真实登录/启动最小化；有无托盘时关闭行为；单实例；自动连接及失败提示。UI只提供其中部分字段，未消费字段需如实记录。 |
| S02 | inbound：mixedPort | 0自动、1024与65535合法、1/1023及越界拒绝；固定与自动冲突策略；显示实际端口与HTTP/SOCKS差异。 |
| S03 | inbound：dokodemoListenPort、dokodemoAddress、dokodemoTargetPort | 0关闭；本地1024–65535、有效目标域名/IP与非零目标端口；不合法组合拒绝，关联F-D01/F-D02。 |
| S04 | inbound：allowLan、lanBind、bypassLocal、bypassExtra、tunEnabled | 私网绑定和LAN认证；绕过开关/额外域名IP真实生效；TUN开关与实际接入语义核对，不只检查布尔字段；失效网卡无监听扩张。 |
| S05 | routing：policy、ruleSetEnabled、blockAds、dnsStrategy、dnsServers、fakeIp | 三策略、规则/广告开关、四DNS策略；DNS最多8行；FakeIP仅支持路线才能生效，不支持须拒绝/明示，不能假装Windows所有模式都支持。 |
| S06 | subscription：intervalHours、updateOnStart、concurrency、timeoutSecs、userAgent、keepRemovedNodes | 周期1–168小时，并发1–16，超时1–120秒；0与上限+1拒绝；启动拉取、UA、删除保留策略真实可观测。 |
| S07 | appearance：theme、language、compact、reduceMotion | 枚举合法/非法、跟随系统、即时表现/保存语义、重启与WebView重建后恢复，关联F-UI04。 |
| S08 | advanced：coreOverride、logLevel、tunMtu、sniffing、tcpFastOpen、muxEnabled、muxConcurrency | core能力拒绝/选择；日志级别真实输出；MTU1280–9000、mux并发1–256；嗅探/TFO/Mux按core和协议的配置与实际行为验证，不承诺各组合都支持。 |
| S09 | 设置通用：revision、schema、草稿/保存/撤销/分组隔离/搜索/重置 | 晚到响应不能覆盖新revision；搜索只筛选不改值；取消/撤销不写磁盘；分组保存不带入别组草稿；reset恢复默认且保留范围明确；旧/损坏/未来schema见F-V06。 |

本节字段来自`crates/ipc/src/settings.rs`。执行时对该结构再次枚举；如分支新增字段，追加结果行，不把当前表当永远不变的白名单。设置重置必须在可还原副本操作，核对节点、订阅、规则和凭据是否按实际语义保留，不能默认“重置=清空所有数据”。

### 12.9 CLI全命令检查

CLI与桌面分别验收。CLI`connect`是连通自检后自动断开，持久连接用`run`；`disconnect`不是跨进程控制桌面会话的远程API。不得同时对同一目录运行CLI和桌面，否则单实例锁拒绝属于正确行为。

```powershell
$CliRoot = Join-Path $env:LOCALAPPDATA "v2rayR-QA\cli-$Stamp"
New-Item -ItemType Directory -Force $CliRoot | Out-Null
& $CliExe --help
& $CliExe --home $CliRoot paths
& $CliExe --home $CliRoot status
& $CliExe --home $CliRoot --json status
```

| ID | 命令族与步骤 | 通过标准 |
|---|---|---|
| F-Q01 | help/version、status、paths、cores、logs；默认目录/env/--home分别执行 | 参数/退出码/路径明确；结构化输出仅在实现支持命令上验证JSON，不假设全局--json使所有命令输出自动JSON化；版本与真实文件一致。 |
| F-Q02 | import：内联链接、UTF-8文件、stdin `-`；nodes；export→再import | UTF-8/Windows换行/路径含空格正常；导入数量/去重/错误与桌面同源；无秘密误泄漏。 |
| F-Q03 | sub list/add/update指定ID/update全部/remove；非法ID/坏URL | 完整管理链路、退出码与数据正确；更新失败保留旧节点；CLI支持参数以--help为准，不编造edit/enable命令。 |
| F-Q04 | install sing-box/xray/mihomo，默认与--url/--sha256；cores重新探测 | 真实Windows二进制/摘要/版本可用；下载错误不执行坏文件；与F-K02/F-K03分别留CLI证据。 |
| F-Q05 | connect节点名/UUID；名字无匹配/多匹配；--mode manual/system-proxy/tun及策略 | connect成功后真实停止core、恢复接管；错误非零且诊断明确；多匹配不随便挑一个；PAC不是当前CLI mode。 |
| F-Q06 | run manual及system-proxy，另测TUN；Ctrl+C、关闭控制台、主PID强杀；单独disconnect | 持久会话真实转发，退出恢复；W10/W16并入此项；单独disconnect只处理本进程/恢复记录语义，不假称控制另一个正在运行会话。 |
| F-Q07 | test全量/单节点；pac-update；backup export/import/pull/push | 真实行为及失败码分别核对；TCP延迟非吞吐；PAC更新资源不等于开启PAC接管；备份/WebDAV复用F-B01–F-B06但CLI另出结果。 |

### 12.10 既有本机验证到Windows的强制重验对应

| 本机历史证据 | Windows必须重新执行 | 不能替代的证明 |
|---|---|---|
| Rust workspace 309通过 | 本次Windows workspace真实构建与执行，并列出平台差异/忽略项 | 不能沿用309这个计数；Windows新增/缺少用例可能不同。 |
| 真实VLESS/两core转发与失败回滚 | F-P01、F-C01/F-C03、F-V01；Windows原生服务或真实远端 | Unix专用测试在Windows为0项，不能称通过。 |
| 协议配置被release core接受 | F-P01–F-P16的Windows CONFIG层，正负向能力组合 | 不能代替FLOW、UDP、TLS身份或系统接管。 |
| TUN桥配置接受 | W23/W24及Windows helper/安装验证 | 不能代替接口、路由、DNS和WeakHostSend恢复。 |
| CLI/桌面guardian 8个macOS场景 | W09–W20对应Windows进程/控制台/强杀路径 | Windows没有Unix信号语义，不能把SIGHUP命令机械转换并填通过。 |
| 原生导入、筛选、草稿、托盘重开 | F-N01/F-N06/F-UI05、S09，Windows WebView2真实执行 | UI mock脚本通过不代表Windows本地存储与重建通过。 |
| 原生手动连接、真实请求、退出清理 | F-C01/F-V01/W03/W06/W09，两core分别验证 | macOS使用本机已有代理作上游，不代表Windows有同等可用环境。 |
| 代理前后未变化、no-op native恢复 | W04/W05/W10/W17、原生WinINet写入测试 | 必须真正写入与还原所拥有的字段，不只做no-op。 |
| F01节点页模式覆盖发现 | W27/F-N09 | 未有修复前不能忽略，也不能用缺core提前失败证明无风险。 |

### 12.11 功能/API覆盖复核与完整验收门槛

本节按实际功能入口复核，防止某页“打开过”就漏掉动作。执行者应对当前`Backend`接口全部方法逐一映射结果：

| 方法/能力族 | 对应功能用例 |
|---|---|
| snapshot、version、paths、logs | F-UI01、F-Q01、F-V09、S09 |
| connect、disconnect、switchNode、setPolicy、setMode、retry | F-C01–F-C06、F-N09、W03–W20、W27 |
| lanCredentials | F-L01–F-L04，包含获取前提及秘密不进入普通snapshot |
| removeNode、setFavorite、setTags、shareLink、importText | F-N01–F-N11、F-P01–F-P16 |
| addSubscription、refreshSubscription、removeSubscription、setSubscriptionEnabled | F-S01–F-S08 |
| testLatency | F-T01–F-T03、F-Q07 |
| saveSettings、resetSettings | S01–S09，以及对应真实行为用例 |
| rules、pickRuleApplications、addRule、updateRule、setRuleEnabled、removeRule | F-R03–F-R05 |
| updatePac | F-R08、F-Q07 |
| exportBackup、previewBackup、applyBackup、readWebDav、createWebDav | F-B01–F-B06、F-Q07 |
| updatesConfigured、checkUpdate、installUpdate | F-U01–F-U03、W26 |
| cores、installCore | F-K01–F-K04、F-P16、F-Q04 |

完整验收要求：

1. W01–W27、本节全部F-ID与S-ID均有结果，数字设置/协议/模式组合有子项。不得只交通过项，也不得用“本机测过”写NOT-RUN后宣布Windows全通过。
2. 核心正常功能、声明支持的协议/模式、真实OS恢复、数据完整性、安全边界有FAIL即不满足全量通过。BLOCKED和NOT-RUN列为未验收，必须由负责人明确接受范围收缩；工程LLM不能自行按“非关键”删除。
3. 尚不支持的组合，只有明确拒绝、无副作用且限制对用户可见，才可标“负向行为PASS”；不得把缺失正向功能改写为负向通过。
4. 安装MSI/NSIS、普通用户/管理员边界、WebView2、真实流量和WinINet独立留证。Windows11 x64通过不自动覆盖Windows10、ARM64或其他版本；目标发布平台若包含它们，逐平台执行同一矩阵。
5. 最终报告除逐例明细，还提供“正常功能/故障恢复/平台安装/长稳性能/未支持与阻塞”五组结论，并链接构建版本、资源清单、证据和所有已知缺口。
