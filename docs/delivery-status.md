# 开发验收记录 · 2026-11-10

本轮基于已有桌面客户端继续开发。以下按实际验证范围记录，不把浏览器预览、代理适配器测试或应用构建当成三平台发布验收。

## 上轮基线：连接链路与 macOS 应用

| 能力 | 实现与证据 | 当时未验证或未交付 |
| --- | --- | --- |
| 连接与内核 | 真实 sing-box 1.14.0 和 Xray 26.3.27 客户端分别经本地 VLESS 服务请求 HTTP 测试目标；连接先经实际 HTTP 监听验证，再尝试系统接管。Xray HTTP/PAC 和 SOCKS 分别使用正确端口。 | 正式分发中每个平台的固定内核资产、完整协议矩阵、真实第三方订阅和公网目标。 |
| 设置与切换 | 同一实例内串行处理会话操作；固定端口冲突报错、自动端口有界选择；坏节点及设置应用失败后尝试重建旧会话。正式界面保留场景相关偏好，收起自定义 DNS，不暴露内核调优或未支持的 TUN/LAN 开关。 | 切换失败时旧会话可能短暂中断；尚无热切换和性能数据。单条订阅覆盖、DNS 双路编辑及认证 LAN 尚未交付。 |
| 接管与恢复 | macOS 系统代理/PAC 在写入前落恢复记录，读回接管结果；按槽位恢复仍由客户端持有的值，保留外部改动。恢复失败可重试，记录保留到成功；桌面退出先断开。 | 系统代理写入、权限拒绝与恢复用脚本接口模拟，未修改本机真实系统代理做验收。Windows/Linux 原生接管、自启及 TUN 仍需实现和实机验证。 |
| 运行与发布 | Rust 回归与 Clippy、前端类型检查和构建、macOS 本地调试应用包。 | 没有三平台签名、公证和安装升级验收。备份/WebDAV、应用自动更新未完成；M4/M5 不能标记通过。 |

本机代理链路测试不依赖外网，也不写系统代理。用已安装的内核运行：

```sh
V2RAYR_TEST_XRAY=/path/to/xray \
V2RAYR_TEST_SING_BOX=/path/to/sing-box \
cargo test -p v2rayr-app --test real_connection --locked -- --ignored
```

探测验证的是当前分流配置下该请求能否经过本地入口完成，不代表所有网站可访问。手动模式只显示「仅本地可用」。登录启动、硬杀恢复和系统接管仍应在独立测试机器或专用账户按方案 §15 复验。


## 本轮补齐：平台接管与打包

| 平台 | 实现 | 验证范围 |
| --- | --- | --- |
| macOS | 沿用已有网络服务适配器；通用 PAC 不在代理失败时静默直连 | 工作区回归与桌面构建；未写入本机真实系统代理 |
| GNOME Linux | GSettings/dconf：手动 HTTP、HTTPS、SOCKS 与 PAC；按字段组记录并恢复；XDG 登录项 | Ubuntu 容器的真实 dconf 写入、读回和恢复，覆盖已有代理与外部改动；没有 GNOME 桌面和浏览器实测。KDE 和未知桌面仍使用本地端口 |
| Windows | WinINet 默认 LAN 的手动代理与 PAC；当前用户 Run 登录项；OS 文件锁；识别和安装 `.exe` 内核 | 本机验证了地址编码、资产映射和 Rust 类型；Windows 原生构建和 WinINet 写入尚未执行。不覆盖拨号/VPN 的专用代理和 WinHTTP |

默认内核资产按 OS/架构匹配，并使用上游发布的 SHA-256：macOS 和 Linux 的 x64/arm64，以及 Windows x64。自定义下载地址需显式提供摘要。CI 已增加三平台构建和 Linux 隔离 GSettings 测试；手动运行 CI 可生成并保存未签名安装包，不会发布到 Release。该远程工作流本轮未运行。

Linux 设置往返复现（使用**隔离配置目录**）：

```sh
export XDG_CONFIG_HOME="$(mktemp -d)"
export XDG_CURRENT_DESKTOP=GNOME GSETTINGS_BACKEND=dconf V2RAYR_TEST_GSETTINGS=1
dbus-run-session -- cargo test -p v2rayr-platform --locked real_gsettings_round_trip -- --ignored
```

Windows 原生往返用例已提供，需在隔离测试账户设置 `V2RAYR_TEST_WININET=1` 后执行 `cargo test -p v2rayr-platform --locked real_wininet_round_trip -- --ignored`。

TUN 特权助手、认证 LAN、备份/WebDAV、签名公证和应用自动更新仍是未交付项。


## 本轮补齐：TUN、认证 LAN、维护与更新

| 功能 | 已实现 | 已通过的验收与边界 |
| --- | --- | --- |
| Linux TUN | 管理员授权的窄接口、独立固定内核、管道租约、接口和路由清理检查；普通节点内核不提权 | Ubuntu ARM64 容器中真实 TUN 和内核转发；普通用户经 sudo 启动；正常断开、控制端 SIGKILL 均清理接口与路由。应用端实际连通探测、断开也通过。未测物理机、IPv6 数据和公网 DNS；macOS/Windows TUN 尚未实现 |
| 认证 LAN | 仅绑定用户指定的私有 IPv4；单独的 HTTP/SOCKS 端口和每会话凭据；默认关闭 | macOS 上真实 sing-box 1.14.4、Xray 26.3.27：HTTP 未认证和错误认证被拒绝，正确认证转发；SOCKS 拒绝免密并接受正确认证；断开后端口关闭，普通快照无密码。未测其他物理设备和平台 |
| 便携备份 / WebDAV | 桌面与 CLI 支持预览后恢复，合并规则并保留本地已有项；远程凭据只用于当次请求。默认只新建，CLI 可用强 ETag 替换 | 本地恢复、无凭据、去重与拒绝覆盖通过；隔离 Linux HTTPS 服务验证认证、上传、下载、替换和过期 ETag。仅备份白名单偏好与自定义规则，未实现含节点/订阅的完整 ZIP、密钥迁移或商用 WebDAV 服务验收 |
| 应用更新 / 签名打包 | Tauri 后台检查提示，用户确认安装，校验签名和绑定版本；网络恢复后才安装。签名候选包工作流和静态清单脚本 | 用临时密钥构建真实 macOS 更新归档，验证签名与版本并拒绝篡改；未进行正式证书签名、公证、Windows/Linux 包安装和完整在线升级。工作流不自动发布 |

WebDAV CLI 例：

```sh
v2rayr backup export preferences.json
v2rayr backup import preferences.json          # 预览
v2rayr backup import preferences.json --apply # 确认后恢复
v2rayr backup push preferences.json https://dav.example/backup.json --username alice
v2rayr backup pull https://dav.example/backup.json restored.json --username alice
```

CLI 从 `V2RAYR_WEBDAV_PASSWORD` 读取密码；GUI 仅在选择 WebDAV 时显示地址和当次凭据。部署与发行分别见 [Linux TUN](tun-linux.md)、[签名候选包](signed-release.md)。


## macOS / Windows TUN 接入

- **macOS 已实现**：固定管理员安装路径、受限 sudo 授权、自动分配 utun、IPv4/IPv6 路由读回和退出恢复检查。本机接管与流量结果见下方实机复验。
- **Windows 已实现**：固定管理员目录与 ACL 检查、只提升 helper 的 UAC、双向 PID 校验的本地命名管道、Wintun 会话和 Job Object。Windows x64 helper 已交叉编译和链接，应用与测试目标也通过 Windows x64 类型检查。尚未在 Windows 上运行 UAC、Wintun 或恢复用例。
- **Linux 回归通过**：隔离 Ubuntu ARM64 中，新版 helper 实际转发流量，并在正常断开和控制进程被强杀后移除接口及路由。

部署和原生测试入口见 [macOS / Windows TUN](tun-desktop.md)。签名候选工作流已为三个平台分别构建并签署 helper 归档；远程工作流未执行。

## macOS 实机复验（2026-11-10）

本机 ARM64 已安装修复后的 root 所有的 helper 和 sing-box 1.14.4，并验证普通账户只需使用固定的 `sudo -n … tun-stdio` 命令即可建立租约。

| 验收项 | 本机结果 |
| --- | --- |
| IPv6 转发 | 通过。curl 未配置显式代理，真实 utun 把 `2001:db8::20:80` 转给受控 SOCKS 服务，收到 HTTP 204。公网 IPv6 未验证；基线没有 IPv6 默认路由。 |
| IPv4 转发 | 通过。修复后，普通 curl 请求经真实 utun 转发到受控 SOCKS 服务并收到 HTTP 204；应用级 `native_tun` 用例通过。另通过受控 SOCKS 中继和本机现有代理出口完成公网 HTTPS 请求（curl 未使用显式代理）。 |
| DNS | macOS 默认系统解析已通过 DoH 复验；物理网卡对照、故障注入和恢复范围见下文。 |
| 正常退出与控制端强杀 | 通过。应用正常断开成功；对普通租约控制进程发送 SIGKILL 后，helper、专用内核、测试 utun 和路由均退出，默认路由及 DNS 恢复基线。未测试强杀 root helper。 |
| 网络切换 | 通过受控流量验证：保持同一 TUN 租约，将系统默认路由从以太网切到 Wi-Fi 再切回；切换前、切换后、切回后 IPv4 和 IPv6 请求均收到 HTTP 204。以太网和 Wi-Fi 电源已恢复初始状态。未验证公网代理节点的存量连接能否跨链路延续。 |
| Windows | 由用户在 Windows 自行测试；UAC、Wintun 流量接管和退出清理仍待实机结果。 |

macOS 桥接改用 gVisor 后通过 IPv4/IPv6 实测。清理读回有界等待，并识别 Darwin `route` 在无 IPv6 路由时即使退出码为 0 也会在 stderr 报告 `not in table` 的情况；未知错误仍按失败处理。此判定已有回归用例。

### macOS 默认 DNS 实机复验

- 根因：有 TUN 路由和 DNS 劫持规则并不等于 macOS 系统解析器会使用它。修复前，系统仍使用物理接口关联的 DNS；显式 UDP 查询可被劫持，但同一环境中的未缓存系统查询没有触发桥接 DoH。第一版补充解析器的通用排序字段被 configd 丢弃，改用 `SupplementalMatchOrders`。
- 修复：在专用内核就绪并读回 DNS 入口路由后，helper 发布只存于 Dynamic Store 会话中的默认补充解析器。该解析器指向 TUN 捕获的 `198.18.0.53:53`，由专用内核经 SOCKS 发往公网 DoH。关闭租约即撤销配置；不改写用户的持久 DNS，保留更具体的分流域。
- 实测：`dscacheutil` 查询带随机标签的公网域名，观察到 DoH TLS 往返，返回预期 A 记录。以太网 `en0` 的抓包捕获了 TUN 关闭时的明文正对照；正常 DoH、强杀前以及故意阻断 DoH 的测试查询均未出现在物理接口上。DoH 被阻断时，系统查询在 10 秒观察窗口内没有得到应答。正常断开和强杀普通控制进程后，`scutil --dns` 均逐字节恢复到测试前，临时记录和测试接口消失。
- 范围：验证的是系统默认公网查询在当前物理链路上的行为。没有覆盖应用自带解析器、显式绑定物理接口的查询、更具体的 VPN 分流域、其他物理网络或长时间故障。Windows 仍由用户验收。
