# 第三方组件

本仓库不包含任何内核二进制。用户自己安装的内核遵循各自的许可证：

| 组件 | 许可证 | 备注 |
| --- | --- | --- |
| [sing-box](https://github.com/SagerNet/sing-box) | GPL-3.0-or-later | 默认内核；按 GPL 要求，分发二进制时须同时提供其源码 |
| [Xray-core](https://github.com/XTLS/Xray-core) | MPL-2.0 | VLESS + xhttp / mKCP 场景 |
| [mihomo](https://github.com/MetaCubeX/mihomo) | GPL-3.0 | 可选的第三内核 |

关于「谁在什么情况下必须开源」：v2rayR 自身以 GPL-3.0-or-later 发布（见根目录 `LICENSE`），
如果打包分发 sing-box/mihomo 二进制，需要附带对应版本的源码或书面要约；只提供下载链接不算。

Rust / npm 依赖的许可证可以用下面两条命令导出，CI 不做许可证白名单拦截，
但引入新依赖时请在 PR 描述里说明它的许可证：

```bash
cargo tree --workspace --format "{p} {l}"
cd ui && pnpm licenses list
```
