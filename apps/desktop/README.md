# v2rayR 桌面壳

Tauri 2 外壳。这里刻意只做三件事：

1. `tauri.conf.json`：窗口尺寸、CSP、bundle 目标；
2. `src/lib.rs`：把 IPC 命令转发给 `v2rayr-app::App`，把 App 的事件转发给 webview；
3. `capabilities/default.json`：只放行 `core:default` 与应用自己注册的命令。

业务逻辑一行都不在这里——它属于 `crates/app`，因此可以被 CLI 与测试复用。

## 为什么它是独立 workspace

根 `Cargo.toml` 用 `exclude = ["apps/desktop"]` 把它排除在外，它自己有一个空的 `[workspace]` 表。
原因：Tauri 的依赖树很大（几百个 crate），如果混进根 workspace，
`cargo test` / `cargo clippy` 在没有图形会话的机器上会拖慢甚至失败。

## 构建

```bash
cd ../..                      # 回到仓库根
pnpm --dir ui install
pnpm --dir ui build           # 或者用 cargo tauri dev 让它自己起
bash scripts/stage-desktop-cores.sh  # 准备校验过的本机架构内核
cd apps/desktop
cargo tauri dev
cargo tauri build             # .app / .dmg
```

需要 `cargo install tauri-cli --version "^2"`。

发行包内置 sing-box 和 Xray，内核与应用一同升级。准备脚本须在目标平台运行；它使用仓库固定的版本及 SHA-256，缺少内核、许可文件或 Xray 地理数据时会失败。

## 已知缺口

- 托盘菜单只暴露了配置项，图标与菜单项尚未接上。
- 「一键安装特权 helper」的引导流程未实现，TUN 会直接返回 `permission_denied` 并给出替代方案。
- 打包签名/公证需要 Apple 开发者证书，CI 里的 desktop job 只做 `--no-bundle` 的编译验证。
