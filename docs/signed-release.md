# 签名候选包与应用更新

[Signed release candidates](../.github/workflows/signed-packages.yml) 是手动触发的工作流，仅上传 CI 构建产物，不发布 Release，也不写入更新站。配置 `release-signing` 环境的审批规则及以下密钥后再触发：

- Tauri：环境变量或仓库变量 `V2RAYR_UPDATER_PUBLIC_KEY`，环境秘密 `TAURI_SIGNING_PRIVATE_KEY` 和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。
- macOS：Developer ID 证书和密码、签名身份，以及 Apple 公证凭据。工作流检查代码签名和已装订的公证票据。
- Windows：可签名的 PFX 与密码。工作流导入当前用户证书存储，并检查 MSI 与 NSIS 的 Authenticode 签名。
- Linux：AppImage 使用 Tauri 更新签名，`.deb` 和 `.rpm` 仍由包管理方式更新。

每个平台还单独输出带 Tauri 签名的管理员 helper 归档；归档签名不等同于 helper 自身的 Authenticode 签名或 Apple 公证。

运行前确认 `tauri.conf.json` 中的版本。输入最终的 HTTPS 清单地址和资产目录；构建结束后审查已签名文件和 `latest.json`，再按既定发布流程上传这些精确文件。清单生成会拒绝缺少产物、缺少签名，或签名注释中版本与构建版本不一致的输入。

启用签名更新的客户端在后台检查并提示，用户选择安装时会重新核对候选版本，下载并验证签名及签名绑定的版本，断开当前会话并确认网络恢复后才安装。没有受信任公钥与发布源的普通调试包不显示更新入口；Linux 包管理器版本不显示 AppImage 更新入口。

本地临时更新密钥只能用作验证，不能替代 Developer ID 公证、Windows Authenticode 或正式更新密钥。正式证书、公网更新地址和实机升级结果应单独记录。
