# 开发指南

## 工具链

| 工具 | 版本 | 说明 |
| --- | --- | --- |
| Rust | 1.85+（edition 2024） | `rust-toolchain.toml` 未固定，CI 用 stable |
| Node | 22 LTS | 只在 `ui/` 需要 |
| pnpm | 10+ | `ui/pnpm-lock.yaml` 已提交 |
| tauri-cli | 2.x | 只有编译 `apps/desktop` 时需要 |

macOS 上还需要 Xcode Command Line Tools（`networksetup`、`codesign`）。

## 常用命令

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                       # 不包含 apps/desktop
cargo build --workspace

cd ui
pnpm install
pnpm dev            # http://localhost:1420，离线预览（mock 后端）
pnpm check          # tsc --noEmit
pnpm build          # 类型检查 + 产物到 ui/dist
```

如果 `pnpm exec` 因依赖状态检查而反复尝试安装（有些沙箱会拦 esbuild 的 postinstall），
直接调用二进制即可：`node node_modules/typescript/bin/tsc --noEmit`。

## 桌面壳

`apps/desktop` 不在根 workspace 里，单独构建：

```bash
cargo install tauri-cli --version "^2"
cd apps/desktop
cargo tauri dev      # 需要先 `pnpm --dir ../../ui install`
cargo tauri build    # 产出 .app / .dmg
```

`tauri.conf.json` 的 `beforeDevCommand` / `beforeBuildCommand` 已经指向前端目录，不用手动起 dev server。

## 离线预览（ui/ 单独跑）

`ui/src/lib/api.ts` 里有两个后端实现：

- `tauriBackend`：真正 `invoke` 到桌面壳，只有在 `window.__TAURI_INTERNALS__` 存在时启用。
- `mockBackend`：浏览器里模拟连接、订阅、测速、设置保存，数据存在 `localStorage` 的 `v2rayr.preview.v1`。

预览模式会在顶栏显示「浏览器预览 · 模拟数据」，设置页底部也有提示，避免把演示当成真实状态。
想恢复初始数据：设置 → 内核状态 → 「重置预览数据」，或在 devtools 里删掉那个 localStorage 键。

## 加一个协议

1. `crates/domain/src/model.rs`：`Protocol` 增加变体（`#[serde(tag = "kind")]`），实现 `kind()`、
   `requires_singbox()`、`Profile::validate` 里需要的约束。
2. `crates/domain/src/sharelink/<proto>.rs`：解析；`crates/domain/src/sharelink/export.rs`：导出（能导出才加）。
3. `crates/config/src/capability.rs`：哪些内核支持；`singbox.rs` / `xray.rs` / `mihomo.rs`：outbound 形状。
4. 测试：解析/导出往返、能力选择、发射结果里的关键字段。

## 加一个设置项

1. 在 `crates/ipc/src/settings.rs` 找到所属分组，加字段 + `Default`（`deny_unknown_fields` 会让旧配置报错，
   所以新字段必须有默认值）。
2. `crates/app/src/settings.rs`：若需要范围校验，加进 `validate`；若语义特殊，加进 `Settings::apply`。
3. 前端：`ui/src/lib/types.ts` 同步类型，`ui/src/lib/i18n.ts` 加中英文案，`pages/Settings.tsx` 加字段行；
   mock 的 `validateSettingsPatch` 也要跟着改，否则预览模式会接受非法值。
4. 若影响内核配置：`crates/app/src/session.rs` 组装 `SessionPlan` 时传下去，并在发射器里使用。

改 `settings.json` 的结构（重命名/移动字段）必须同时把 `CURRENT_SCHEMA` 加一，并在 `Settings::migrate`
里写幂等迁移；旧版本读到新文件会备份后回退默认值，不会猜着读。

## 测试约定

- 库层测试就近放在模块底部（`#[cfg(test)] mod tests`），集成契约放 `crates/*/tests/`。
- 时间相关的逻辑用显式 `now` 参数，不用 `Utc::now()` 藏在深处。
- 涉及网络的测试只连本机 `TcpListener`，不依赖外网。
- 只改备注的节点会因指纹相同被合并——需要区分节点时改端口或地址。

## 提交约定

Conventional Commits：`feat(engine): ...`、`fix(app): ...`、`docs: ...`、`test: ...`。
一个 PR 只做一件事；动到跨层契约（`crates/ipc`）时同时改前后端与契约测试。
