# macOS / Windows TUN 助手

两个平台都沿用同一条链路：普通权限的节点内核提供本机 SOCKS，管理员授权的 helper 只启动固定的 sing-box TUN 网关。GUI 不提权，也不读取或转交管理员密码。节点须使用固定 IPv4、sing-box 和全局分流；避免主内核的节点连接被重新接管。helper 首次配置和启动均只使用固定路径，握手只接受本地端口和节点 IPv4。

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

当前实现要求标准 `C:\Program Files` 和 `C:\Windows`，并要求用户可在本账号完成 UAC 提权；使用另一账户输入管理员凭据的路径未支持。管理员验证 helper、sing-box 和对应架构的官方 Wintun DLL 来源，安装到固定目录：

- `C:\Program Files\v2rayR-TUN\v2rayr-helper.exe`
- `C:\Program Files\v2rayR-TUN\sing-box.exe`
- `C:\Program Files\v2rayR-TUN\wintun.dll`
- `C:\Program Files\v2rayR-TUN\run\`（目录）

整个子树由 Administrators 或 SYSTEM 所有，只允许这两者写入；给普通 Users 读和执行，不使用符号链接或目录联接。新部署可在**管理员 PowerShell**中先设置目录 ACL，再从已验证的源复制文件。下面的命令仅适用于新安装；已有目录先由管理员审查。

```powershell
$root = 'C:\Program Files\v2rayR-TUN'
if (Test-Path -LiteralPath $root) { throw '已有部署，请先审查 ACL 和二进制' }
New-Item -ItemType Directory -Path $root | Out-Null
icacls.exe $root /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-32-545:(OI)(CI)RX'
if ($LASTEXITCODE) { throw '设置 TUN 目录 ACL 失败' }
New-Item -ItemType Directory -Path "$root\run" | Out-Null
Copy-Item -LiteralPath .\v2rayr-helper.exe,.\sing-box.exe,.\wintun.dll -Destination $root
icacls.exe $root /setowner '*S-1-5-32-544' /T
if ($LASTEXITCODE) { throw '设置管理员所有者失败' }
```

每次接入时只为固定 helper 请求 UAC。应用和 helper 双向检查本地命名管道的进程身份；管道拒绝远程访问。helper 用 Job Object 管理网关，优先请求正常退出，超时才终止，并验证地址和路由已经撤销。若存在同名适配器的残留路由，拒绝下一次接管，不对其他 Wintun 设备做批量删除。

## 实机验收

在一次性虚拟机或专用测试机上部署后，以普通桌面用户开启全局分流，选固定 IPv4 的 sing-box 节点，再选择 TUN。验证实际转发、DNS、IPv6、正常断开、强杀普通客户端和切换网络。也可以用本地 SOCKS 测试服务跑应用级验收：

```sh
V2RAYR_TUN_ISOLATED_TEST=1 V2RAYR_TEST_TUN_ADDRESS=192.168.77.2 cargo test -p v2rayr-app --test native_tun --locked -- --ignored
```

Windows 在 PowerShell 中先设置对应的 `$env:V2RAYR_TUN_ISOLATED_TEST` 和 `$env:V2RAYR_TEST_TUN_ADDRESS`，再执行同一条 Cargo 命令。将示例 IP 换成隔离环境中本机实际绑定的私有 IPv4。不要在日常工作机用该用例切换系统网络。
