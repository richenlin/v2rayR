# Linux TUN 部署与验收

当前接入范围是 Linux、全局分流和固定 IPv4 的 sing-box 节点。macOS 与 Windows 的部署见 [平台 TUN 助手](tun-desktop.md)。未部署可信 helper 时，连接页不会显示可用的 TUN 选项。

特权进程使用专用 sing-box，只读取它自己生成的固定结构配置。它把流量送到普通用户进程的本地 SOCKS 入口；订阅、认证 LAN、节点秘密和下载的普通内核留在非特权侧。接口为 `v2rayr-tun0`，独占路由表和规则基址为 `18622`；发现残留或冲突会拒绝接管。

管理员需先验证待部署 helper 的来源与签名，并校验官方 Linux sing-box 资产的 SHA-256。不要将用户数据目录的内核、符号链接或任意路径放进 sudo 策略。下列命令是管理员手动部署步骤，不在应用启动时运行：

```sh
sudo install -d -o root -g root -m 0755 /usr/local/libexec/v2rayr
sudo install -o root -g root -m 0755 ./v2rayr-helper /usr/local/libexec/v2rayr/v2rayr-helper
sudo install -o root -g root -m 0755 ./sing-box /usr/local/libexec/v2rayr/sing-box
sudo visudo -f /etc/sudoers.d/v2rayr-tun
```

将 `DESKTOP_USER` 替换为明确授权的本机账户。只授权这个完整命令：

```sudoers
Cmnd_Alias V2RAYR_TUN = /usr/local/libexec/v2rayr/v2rayr-helper tun-stdio
Defaults!V2RAYR_TUN !use_pty
DESKTOP_USER ALL=(root) NOPASSWD: V2RAYR_TUN
```

应用用 `sudo -n` 启动；无需读取或传递管理员密码。管道就是会话租约，关闭或控制进程退出会触发清理。普通退出检查接口及 IPv4/IPv6 路由表；恢复未通过时提示重试，不把它当成断开成功。若内核与 helper 均被强制终止，需按宿主机策略处理残留；应用不盲删系统路由。

在**隔离 Linux 网络命名空间**内，配置好设备、权限和已安装 helper 后，以被授权的普通用户运行：

```sh
V2RAYR_TUN_ISOLATED_TEST=1 python3 crates/helper/tests/linux_tun_smoke.py
V2RAYR_TUN_ISOLATED_TEST=1 V2RAYR_TEST_TUN_ADDRESS=192.168.77.2 cargo test -p v2rayr-app --test native_tun --locked -- --ignored
```
