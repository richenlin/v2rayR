# v2rayR

[![CI](https://github.com/richenlin/v2rayR/actions/workflows/ci.yml/badge.svg)](https://github.com/richenlin/v2rayR/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

面向 macOS、Windows 和 Linux 的代理客户端。Rust 负责节点、订阅、内核和系统接管，React 负责工作台。界面默认中文，也提供英文；浅色与深色主题跟随系统，可以手动切换。

A desktop and command-line proxy client. The workspace version is **0.1.0**. Signed installers are not published yet; build from source below.

连接状态、失败恢复和设置保存遵循三条约束：

- **状态不说谎。** 内核端口就绪且系统接管成功，才显示已连接。接管失败就显示失败。
- **失败不破坏已有配置。** 订阅更新失败保留旧节点；接管写入失败按快照回滚。
- **设置可以撤销。** 按组保存，保存前校验；影响正在运行的会话时，失败后尝试恢复旧会话。

## 截图

| 连接（浅色） | 连接（深色） |
| --- | --- |
| ![连接 · 浅色](docs/images/app-connection-light.png) | ![连接 · 深色](docs/images/app-connection-dark.png) |

| 节点 | 订阅 | 设置 |
| --- | --- | --- |
| ![节点](docs/images/app-nodes.png) | ![订阅](docs/images/app-subscriptions.png) | ![设置](docs/images/app-settings.png) |

这些图来自浏览器里的离线预览（`ui/` 使用模拟后端）。预览不读写本机配置，也不启动内核。数据目录、系统代理和内核进程只在桌面应用里生效。

## 功能

- **节点。** 导入 VLESS、VMess、Trojan、Shadowsocks、SOCKS、HTTP、Hysteria2、TUIC、WireGuard、AnyTLS 分享链接，支持整段 Base64 和多行文本。按连接指纹去重，保留收藏、标签和延迟。节点列表可虚拟滚动。
- **订阅。** 识别链接列表、Base64 列表、SIP008 JSON、Clash YAML 和 sing-box JSON。按指纹合并：新增、原地更新、删除未收藏节点。更新失败只记录错误。桌面运行时按间隔自动检查。原生配置里被跳过的条目会逐条说明原因；一条都无法导入时直接报错，不清空现有节点。
- **内核。** 默认 sing-box。VLESS 的 xhttp、mKCP 以及 VLESS Encryption 使用 Xray。mihomo 可选。版本固定，下载时校验 SHA-256，解压后原子替换。配置先经内核自检，再启动，并用本地 HTTP 代理确认链路，然后才接管系统。
- **接入。** 系统代理、PAC、手动端口，以及经管理员部署的 TUN 助手。系统代理和 PAC 覆盖 macOS、GNOME，以及 Windows 的默认 LAN。写入前保存恢复记录，恢复失败时保留记录以便重试。局域网共享使用单独端口和当次凭据，默认关闭。
- **分流。** 内置私有网段，加上图形化规则：域名、后缀、网段、进程、GeoIP、GeoSite，动作是直连、代理或拦截。规则立即保存，下次连接生效。
- **桌面。** 登录时启动，关闭窗口后可保持连接，单实例，托盘菜单提供显示、重连、断开和退出。支持 `v2rayr://import/<base64>` 和命令行导入分享链接。
- **维护。** 导出和恢复白名单里的偏好与自定义规则，可经 HTTPS WebDAV 上传下载。签名更新在配置了受信任公钥和发布源之后才会出现。
- **命令行。** 同一套后端提供 `v2rayr`，可在没有界面的机器上导入、连接、测速和诊断。

## 平台支持

| | macOS | Linux | Windows |
| --- | --- | --- | --- |
| 系统代理 / PAC | 活动网络服务 | GNOME（GSettings）。KDE 和其他桌面使用本地端口 | 默认 LAN（WinINet），不含拨号、VPN 专用连接和 WinHTTP |
| 登录时启动 | LaunchAgent | XDG 桌面入口 | 当前用户 Run 键 |
| TUN | 本机 ARM64 已复验转发、DNS 和恢复 | 隔离环境已复验转发和清理 | 助手已实现，尚未在 Windows 上做 UAC 和 Wintun 实机验收 |
| 安装包 | `.app`、`.dmg` | `.deb`、`.rpm`、AppImage | MSI、NSIS |

TUN 需要管理员事先安装固定路径的助手和专用 sing-box，应用本身不提权。部署步骤见 [Linux](docs/tun-linux.md) 和 [macOS / Windows](docs/tun-desktop.md)。各平台已经验证和尚未验收的范围见 [开发验收记录](docs/delivery-status.md)。

## 从源码构建

需要 Rust 1.85+、Node.js 22 和 pnpm 10。编译桌面应用另需 Tauri CLI 2。macOS 还需要 Xcode Command Line Tools。

```bash
git clone https://github.com/richenlin/v2rayR.git
cd v2rayR

# 命令行与后端
cargo build --workspace
./target/debug/v2rayr --help

# 浏览器预览：模拟数据，不写本机配置
pnpm --dir ui install
pnpm --dir ui dev          # http://localhost:1420

# 桌面安装包（当前平台；产物在仓库根目录 dist/）
bash scripts/package-desktop.sh
```

桌面包内置当前平台的 sing-box 和 Xray，优先使用包内版本。mihomo 仍按需下载。TUN 助手使用单独部署的内核，不从用户数据目录取可执行文件。

仓库目前没有已签名的正式安装包。签名、公证和更新源的配置见 [签名候选包](docs/signed-release.md)。

## 命令行

`v2rayr connect` 连上并确认内核就绪后立即断开，用来自检。需要保持连接时用 `v2rayr run`。命令行默认 `manual`，不改系统代理，避免进程退出后系统仍指向一个已经消失的端口。`test` 只测量到节点的 TCP 延迟。所有命令都可以加 `--json`。`--home` 或环境变量 `V2RAYR_HOME` 指定数据目录。

```bash
export V2RAYR_HOME=/tmp/v2rayr

v2rayr status
v2rayr import 'vless://...'          # 也可以是文件路径，或用 - 读取标准输入
v2rayr sub add 'https://example/sub' --name Example
v2rayr sub update
v2rayr nodes
v2rayr install sing-box              # 下载固定版本并校验 SHA-256
v2rayr connect 'Tokyo' --mode manual
v2rayr run 'Tokyo' --mode manual     # Ctrl-C 断开
v2rayr test
v2rayr export 'Tokyo'
v2rayr backup export preferences.json
v2rayr paths
```

常用子命令还有 `disconnect`、`cores`、`logs`、`pac-update`，以及 `sub list|remove`、`backup import|pull|push`。WebDAV 密码从 `V2RAYR_WEBDAV_PASSWORD` 读取，不写进命令行参数。

## 数据目录

可用 `v2rayr paths` 查看本机实际路径。`V2RAYR_HOME` 会覆盖默认位置。

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
data/rules/               可选规则集（*.srs），放进来才会被引用
run/                      运行中的内核配置、接管记录、单实例锁
logs/                     应用日志与内核日志
```

## 协议与内核

显式指定内核时仍会检查该内核能不能表达这个节点。没有可用内核时直接拒绝，而不是先启动再失败。

| 节点 | 内核 |
| --- | --- |
| VLESS + xhttp / mKCP | Xray。Xray 没有混合入站，SOCKS 与 HTTP 分端口 |
| VLESS Encryption（`mlkem768x25519plus.*`） | Xray 25.x |
| 普通 VLESS、Reality、XTLS Vision | 默认 sing-box，可指定 Xray |
| VMess、Trojan、Shadowsocks、SOCKS、HTTP | 默认 sing-box |
| Hysteria2、TUIC、AnyTLS、WireGuard、Shadowsocks 2022 | sing-box |
| 其余 sing-box 能表达的出站 | sing-box |

Naive 分享链接可以导入，但当前没有内核能运行它，连接会被拒绝。代理链同样未实现。Clash 与 sing-box 原生配置里不认识的协议、插件和传输会被跳过，并给出原因。

## 文档

| 文档 | 内容 |
| --- | --- |
| [架构说明](docs/architecture.md) | 分层、会话状态机、端口、错误模型和安全边界 |
| [开发指南](docs/development.md) | 工具链、桌面壳、如何加协议或设置项 |
| [设置取舍](docs/config.md) | 哪些设置交给用户，哪些留在程序里 |
| [验收记录](docs/delivery-status.md) | 已验证的范围和尚未通过的发行门槛 |
| [Linux TUN](docs/tun-linux.md)、[macOS / Windows TUN](docs/tun-desktop.md) | 助手的安装和验收 |
| [签名发布](docs/signed-release.md) | 签名候选包和更新源 |

## 开发

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm --dir ui check
pnpm --dir ui build
```

`apps/desktop` 不在根 workspace 里，所以没有桌面工具链时也能跑上面的后端测试。持续集成在 macOS、Windows 和 Linux 上执行同样的检查，并单独构建桌面壳，见 [.github/workflows/ci.yml](.github/workflows/ci.yml)。

提交使用 [Conventional Commits](https://www.conventionalcommits.org/)，例如 `feat(engine): ...`、`fix(app): ...`。一个拉取请求只做一件事；改到 `crates/ipc` 的契约时，同时改前端、后端和契约测试。更具体的约定在 [开发指南](docs/development.md)。

问题请开 [Issue](https://github.com/richenlin/v2rayR/issues)。改代码前先看 [验收记录](docs/delivery-status.md)，避免把尚未在目标平台跑通的能力写成已经发布。

## 当前限制

- 规则集（`*.srs`）不随仓库分发。未放入 `data/rules/` 时，规则模式只使用内置私有网段和自定义规则。
- TUN 目前要求全局分流，以及地址为固定 IPv4 的 sing-box 节点。未部署助手时，连接页不提供可用的 TUN 开关。
- 便携备份和 WebDAV 只保存白名单中的偏好与自定义规则，不包含节点、订阅和密钥。
- Windows 的系统代理写入，以及 Windows、Linux 安装包的安装与升级，还没有完成原生验收。
- 正式代码签名、公证和公网更新源尚未配置。没有受信任公钥的调试包不显示更新入口。

## 许可

本程序以 [GPL-3.0-or-later](LICENSE) 发布，版权声明见 [NOTICE](NOTICE)。

内核二进制不在仓库里。自行安装或打进安装包的内核保留各自的许可证：sing-box 与 mihomo 为 GPL-3.0，Xray-core 为 MPL-2.0。分发这些二进制时需要遵守对应许可证。清单见 [resources/licenses/README.md](resources/licenses/README.md)。

## 相关项目

v2rayR 使用 [sing-box](https://github.com/SagerNet/sing-box)、[Xray-core](https://github.com/XTLS/Xray-core) 和 [mihomo](https://github.com/MetaCubeX/mihomo) 作为可选的代理内核，桌面壳使用 [Tauri](https://tauri.app/)。
