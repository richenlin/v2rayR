# macOS / Windows TUN 助手

两个平台都沿用同一条链路：普通权限的节点内核提供本机 SOCKS，管理员授权的 helper 只启动固定的 sing-box TUN 网关。GUI 不提权，也不读取或转交管理员密码。节点须使用固定 IPv4 和 sing-box，避免主内核的节点连接被重新接管。Windows 目前使用全局代理。macOS 可用新版 helper 与 sing-box 1.14 或更新版本开启规则分流，未命中默认直连。助手只使用固定路径，握手接受本地端口、节点 IPv4 和有界结构化规则。详见 [应用分流方案](application-routing.md#当前交付路线tun-规则分流)。

## macOS

从可信构建取得 `v2rayr-helper`，校验管理员专用 sing-box 的发布摘要。管理员将二进制安装在 root 控制的固定目录；不要用 Homebrew 或用户数据目录的可写路径：

```sh
sudo install -d -o root -g wheel -m 0755 /Library/PrivilegedHelperTools/v2rayr
sudo install -o root -g wheel -m 0755 ./v2rayr-helper /Library/PrivilegedHelperTools/v2rayr/v2rayr-helper
sudo install -o root -g wheel -m 0755 ./sing-box /Library/PrivilegedHelperTools/v2rayr/sing-box
sudo visudo -f /etc/sudoers.d/v2rayr-tun
```

sudoers 仅授权实际测试账户（把 `DESKTOP_USER` 替换成用户名）：

```sudoers
Cmnd_Alias V2RAYR_TUN = /Library/PrivilegedHelperTools/v2rayr/v2rayr-helper tun-stdio
Defaults!V2RAYR_TUN !use_pty
DESKTOP_USER ALL=(root) NOPASSWD: V2RAYR_TUN
```

应用用 `sudo -n` 运行该固定命令。sing-box 自动选择 `utun`，helper 按会话地址识别接口，并读回 IPv4/IPv6 的路由。普通应用退出或被强杀会关闭租约管道；helper 正常关闭网关后检查接口和路由。现版本没有安装 LaunchDaemon，也不承诺 helper 本身被强杀时能自动恢复。

TUN 就绪后，helper 使用 macOS Dynamic Store 的临时解析器接管系统默认 DNS，查询经 TUN 转成 DoH；无需修改系统设置中的 DNS。更具体的 VPN 分流域保留原有规则。验证时可比较接管前后 `scutil --dns`，并用未缓存的系统查询及物理接口抓包区分 DoH、明文回退和缓存命中。具体实测范围见 [验收记录](delivery-status.md)。

## Windows x64

用可信的 MSI 或 NSIS 安装包按机器安装到默认的 `C:\Program Files\v2rayR`。安装包在 `windows-tun` 子目录内放置独立 helper、专用 sing-box、官方 Wintun 0.14.1 DLL、许可与版本及 SHA-256 清单。启动前校验路径、所有者、ACL、版本和文件摘要。旧版 `C:\Program Files\v2rayR-TUN` 目录可由管理员在确认旧会话已停止后清理。

连接仍需要当前账号完成 UAC。只有安装目录里的 helper 会被提升。应用和 helper 检查对方的命名管道 PID，并使用异步管道同时处理回复与租约断开。Windows CLI 也复用安装目录中的组件。

异常退出后，helper 使用受保护的恢复记录核对 TUN 路由与接管前已启用的 `WeakHostSend`。如果 helper 被强杀，客户端在断开、重试或再次连接时可请求管理员恢复。恢复记录按接口 GUID 和地址族存储，尚未通过读回的记录会保留。会话期间才出现且被上游 core 修改的接口可能缺少启动前记录，热插拔恢复须单独验收。

升级前先断开 TUN。桌面内置更新会先等待网络恢复，再启动安装器。修复时可重新运行可信 NSIS 安装包，或以管理员身份运行 `msiexec /fa "路径\v2rayR.msi"`。也可运行以下命令确认安装目录的内容、版本和权限一致。

```powershell
& 'C:\Program Files\v2rayR\windows-tun\v2rayr-helper.exe' tun-verify-install-v2
```

## 实机验收

在一次性虚拟机或专用测试机上部署后，以普通桌面用户开启全局分流，选固定 IPv4 的 sing-box 节点，再选择 TUN。验证实际转发、DNS、IPv6、正常断开、强杀普通客户端和切换网络。也可以用本地 SOCKS 测试服务跑应用级验收：

```sh
V2RAYR_TUN_ISOLATED_TEST=1 V2RAYR_TEST_TUN_ADDRESS=192.168.77.2 cargo test -p v2rayr-app --test native_tun --locked -- --ignored
```

macOS 安装新版 helper 后，可在同一条命令前另加 `V2RAYR_TEST_TUN_RULES=1`，验证测试进程的精确应用规则能通过 TUN 到达该 SOCKS 服务。该用例仅覆盖匹配后的 TCP 代理及正常清理。

Windows 实机应分别从 MSI 与 NSIS 安装，使用同账号的非提升桌面进程验证首次 UAC、拒绝后重试、TCP/UDP、IPv4/IPv6、DNS，以及正常断开和分别强杀 GUI、helper、core 后的恢复。用 `Get-NetIPInterface`、`Get-NetRoute` 和 `Get-DnsClientServerAddress` 比对接管前后的状态，包含 `WeakHostSend`、切换网卡、升级及损坏文件修复。

Windows 在 PowerShell 中先设置对应的 `$env:V2RAYR_TUN_ISOLATED_TEST` 和 `$env:V2RAYR_TEST_TUN_ADDRESS`，再执行同一条 Cargo 命令。将示例 IP 换成隔离环境中本机实际绑定的私有 IPv4。不要在日常工作机用该用例切换系统网络。
