# 规则集

这些文件**不随仓库分发**：体积大、更新频繁，而且需要和内核版本对齐。
`crates/app/src/session.rs` 只在该目录里存在对应文件时才把规则集写进内核配置：

| 文件 | tag | 作用 |
| --- | --- | --- |
| `geosite-cn.srs` | `geosite-cn` | 常用国内域名 → 直连 |
| `geoip-cn.srs` | `geoip-cn` | 国内 IP 段 → 直连 |
| `geosite-ads.srs` | `geosite-ads` | 广告域名 → 拒绝（需在设置里打开「拦截广告」） |

放置位置是数据目录下的 `data/rules/`（默认 `~/Library/Application Support/v2rayR/data/rules/`）。
格式为 sing-box 的 binary rule-set（`.srs`），可自行用 `sing-box rule-set compile` 从 JSON 源文件生成。

缺文件不会让连接失败：配置里只出现内置私有网段与自定义规则，日志会写一条
「未找到规则集文件，规则模式只使用内置私有网段与自定义规则」。
