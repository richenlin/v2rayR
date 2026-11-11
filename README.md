# v2rayR

![CI](https://github.com/richenlin/v2rayR/actions/workflows/ci.yml/badge.svg)
![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)
![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)

v2rayR 是适用于 macOS、Windows 和 Linux 的代理客户端。它导入节点与订阅，选择并校验代理内核，再按系统代理、PAC、手动端口或 TUN 接管本机网络。

v2rayR is a proxy client for macOS, Windows, and Linux. The desktop app and the `v2rayr` command share one Rust core for profiles, subscriptions, proxy kernels, and system takeover. Signed installers are not published yet.

## 界面

| 连接（浅色） | 连接（深色） |
| --- | --- |
| ![连接，浅色主题](docs/images/app-connection-light.png) | ![连接，深色主题](docs/images/app-connection-dark.png) |

## 功能

**节点。** 支持导入 VLESS、VMess、Trojan、Shadowsocks、SOCKS、HTTP、Hysteria2、TUIC、WireGuard、AnyTLS 分享链接，以及整段 Base64 和多行文本。相同连接按指纹合并，收藏、标签和延迟予以保留。节点列表使用虚拟滚动。

**订阅。** 识别链接列表、Base64 列表、SIP008 JSON、Clash YAML 和 sing-box JSON。更新时按指纹新增、原地更新，并删除未收藏的失效节点。更新失败只记录错误，不替换已有节点。原生配置中被跳过的条目会逐条说明原因；若一条都无法导入，则报错并保留现有节点。桌面端按设定间隔自动检查。

**内核。** 默认使用 sing-box。VLESS 的 xhttp、mKCP 以及 VLESS Encryption 使用 Xray。mihomo 可选。内核版本固定，下载时校验 SHA-256，解压后原子替换。连接前先做内核自检，启动后用本地 HTTP 代理确认链路，然后再接管系统。

**接入。** 提供系统代理、PAC、手动端口，以及由管理员预先安装的 TUN 助手。系统代理和 PAC 覆盖 macOS 的活动网络服务、GNOME，以及 Windows 的默认 LAN 连接。写入前保存恢复记录；恢复失败时保留记录，以便再次尝试。局域网共享使用单独端口和当次凭据，默认关闭。

**分流。** 内置私有网段，并提供图形化规则：域名、后缀、网段、进程、GeoIP、GeoSite，动作分为直连、代理和拦截。规则立即保存，于下次连接生效。

**桌面。** 支持登录时启动。关闭窗口后可保持连接。同一数据目录只运行一个实例。托盘菜单提供显示主窗口、重新连接、断开和退出。可通过 `v2rayr://import/<base64>` 或命令行导入分享链接。

**维护。** 可导出和恢复白名单内的偏好与自定义规则，并经 HTTPS WebDAV 上传或下载。配置了受信任公钥和发布源之后，才会提供签名更新。

## 平台

|            | macOS                  | Linux                            | Windows                                |
| ---------- | ---------------------- | -------------------------------- | -------------------------------------- |
| 系统代理 / PAC | 活动网络服务                 | GNOME（GSettings）。KDE 和其他桌面使用本地端口 | 默认 LAN（WinINet）。不含拨号、VPN 专用连接和 WinHTTP |
| 登录时启动      | LaunchAgent            | XDG 桌面入口                         | 当前用户 Run 键                             |
| TUN        | 本机 ARM64 已复验转发、DNS 和恢复 | 隔离环境已复验转发和清理                     | UAC 与 Wintun 未做实机验收                    |
| 安装包        | `.app`、`.dmg`          | `.deb`、`.rpm`、AppImage           | MSI、NSIS                               |

## 从源码构建

需要 Rust 1.85 或更高版本、Node.js 22 和 pnpm 10。编译桌面应用另需 Tauri CLI 2。macOS 还需要 Xcode Command Line Tools。

```bash
git clone https://github.com/richenlin/v2rayR.git
cd v2rayR

# 命令行与后端
cargo build --workspace
./target/debug/v2rayr --help

# 浏览器预览：模拟数据，不写本机配置
pnpm --dir ui install
pnpm --dir ui dev

# 当前平台的桌面安装包，产物在仓库根目录 dist/
bash scripts/package-desktop.sh
```

浏览器预览地址为 `http://localhost:1420`。

桌面包内置当前平台的 sing-box 和 Xray，优先使用包内版本。mihomo 仍按需下载。TUN 助手使用单独部署的内核，不从用户数据目录加载可执行文件。

签名、公证和更新源的配置见 [签名候选包](docs/signed-release.md)。

## 命令行

`v2rayr` 面向无界面环境，以及需要脚本化导入、连接、测速和诊断的场景。`connect` 在确认内核就绪后立即断开，用于自检；需要保持连接时使用 `run`。命令行默认 `manual`，不修改系统代理，以免进程退出后系统仍指向已经消失的端口。`test` 只测量到节点的 TCP 延迟。

所有命令都可加 `--json`。`--home` 或环境变量 `V2RAYR_HOME` 指定数据目录。WebDAV 密码从 `V2RAYR_WEBDAV_PASSWORD` 读取，不作为命令行参数。

```bash
export V2RAYR_HOME=/tmp/v2rayr

v2rayr import 'vless://...'          # 也可传入文件路径，或用 - 读取标准输入
v2rayr sub add 'https://example/sub' --name Example
v2rayr sub update
v2rayr install sing-box              # 下载固定版本并校验 SHA-256
v2rayr run 'Tokyo' --mode manual     # Ctrl-C 断开
```

| 命令 | 作用 |
| --- | --- |
| `status` | 查看连接与内核状态 |
| `nodes` | 列出已保存的节点 |
| `import` | 导入分享链接、Base64 或文件 |
| `export` | 输出指定节点的分享链接 |
| `sub list` / `add` / `update` / `remove` | 管理订阅 |
| `connect` | 连接并在内核就绪后断开 |
| `run` | 保持连接，直到中断 |
| `disconnect` | 断开当前会话 |
| `test` | 测量一个或全部节点的 TCP 延迟 |
| `cores` | 查看已发现的内核 |
| `install` | 下载 sing-box、xray 或 mihomo |
| `logs` | 查看最近的日志 |
| `pac-update` | 刷新 PAC 使用的域名规则 |
| `backup export` / `import` / `pull` / `push` | 导出、预览恢复，或经 WebDAV 传输 |
| `paths` | 打印解析后的数据目录 |

`backup import` 默认只预览将要改动的设置组，确认后加 `--apply`。

## 数据目录

默认路径如下。`V2RAYR_HOME` 会覆盖该位置；本机实际路径以 `v2rayr paths` 为准。

| 系统 | 默认路径 |
| --- | --- |
| macOS | `~/Library/Application Support/v2rayR` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/v2rayR` |
| Windows | `%APPDATA%\v2rayR` |

```
config/settings.json      设置，带 schema 版本
config/secrets/           本地密钥材料
data/v2rayr.db            节点、订阅、规则、流量样本
data/cores/               自行安装的 sing-box、xray、mihomo
data/rules/               可选规则集（*.srs），放入后才会被引用
run/                      运行中的内核配置、接管记录、单实例锁
logs/                     应用日志与内核日志
```

## 协议与内核

显式指定内核时，仍会检查该内核能不能表达所选节点。没有可用内核时直接拒绝连接。

| 节点 | 内核 |
| --- | --- |
| VLESS + xhttp / mKCP | Xray。Xray 没有混合入站，SOCKS 与 HTTP 分端口 |
| VLESS Encryption（`mlkem768x25519plus.*`） | Xray 25.x |
| 普通 VLESS、Reality、XTLS Vision | 默认 sing-box，可指定 Xray |
| VMess、Trojan、Shadowsocks、SOCKS、HTTP | 默认 sing-box |
| Hysteria2、TUIC、AnyTLS、WireGuard、Shadowsocks 2022 | sing-box |
| 其余 sing-box 能表达的出站 | sing-box |

Naive 分享链接可以导入，当前没有内核能运行它，连接会被拒绝。代理链尚未实现。Clash 与 sing-box 原生配置中无法识别的协议、插件和传输会被跳过，并给出原因。

## 当前限制

- 规则集（`*.srs`）不随仓库分发。未放入 `data/rules/` 时，规则模式只使用内置私有网段和自定义规则。
- TUN 目前要求全局分流，以及地址为固定 IPv4 的 sing-box 节点。未部署助手时，连接页不提供可用的 TUN 开关。
- 便携备份和 WebDAV 只保存白名单中的偏好与自定义规则，不包含节点、订阅和密钥。
- Windows 的系统代理写入，以及 Windows、Linux 安装包的安装与升级，尚未完成原生验收。
- 正式代码签名、公证和公网更新源尚未配置。没有受信任公钥的调试包不显示更新入口。

## 许可

本程序以 [GPL-3.0-or-later](LICENSE) 发布。版权声明见 [NOTICE](NOTICE)。

仓库不包含内核二进制。自行安装或打进安装包的内核保留各自的许可证：sing-box 与 mihomo 为 GPL-3.0，Xray-core 为 MPL-2.0。分发这些二进制时须遵守对应许可证。清单见 [resources/licenses/README.md](resources/licenses/README.md)。

## 相关项目

自建机场时，服务端配合 [x-ui Control Plane](https://github.com/richenlin/x-ui-control-plane) 使用。负责入站、密钥和客户端链接；v2rayR 导入这些链接，在本机完成连接。

v2rayR 使用 [sing-box](https://github.com/SagerNet/sing-box)、[Xray-core](https://github.com/XTLS/Xray-core) 和 [mihomo](https://github.com/MetaCubeX/mihomo) 作为代理内核，桌面壳使用 [Tauri](https://tauri.app/)。
