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
bash scripts/package-desktop.sh      # 安装包与 SHA256SUMS 写到仓库根目录 dist/
cd apps/desktop && ../../ui/node_modules/.bin/tauri dev
```

打包脚本会安装 UI 依赖、构建并内联前端、校验并准备本机架构的内核，然后调用仓库锁定的 Tauri CLI。只编译、不打安装包时用 `bash scripts/package-desktop.sh --no-bundle`。

发行包内置 sing-box 和 Xray，内核与应用一同升级。准备脚本须在目标平台运行；它使用仓库固定的版本及 SHA-256，缺少内核、许可文件或 Xray 地理数据时会失败。

## 已知缺口

- 托盘菜单只暴露了配置项，图标与菜单项尚未接上。
- Windows 的 MSI 和 NSIS 在默认 Program Files 目录部署 TUN 组件。其他平台的部署流程见 [TUN 助手](../../docs/tun-desktop.md)。
- 合并到 `main` 后，CI 在 macOS、Windows、Linux 上分别打包，产物在该任务的 `dist/`，并作为构建产物上传。签名和公证仍由手动的 Signed release candidates 工作流完成。
- 拉取请求只编译桌面壳，不产出安装包。
