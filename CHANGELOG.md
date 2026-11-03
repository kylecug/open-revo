# 更新日志 (Changelog)

本文档记录 OpenRevo 项目的所有重大更新、功能特性、问题修复与硬件兼容性演进。
版本格式严格遵循 [语义化版本 2.0.0 (Semantic Versioning)](https://semver.org/lang/zh-CN/)。

---

## [Unreleased] - 待发布

### ✨ 新增 (Added)
- 新增插件权限审计与防爆盾审计模式（过渡期只告警不拦截），配置项 `raw_gateway_enforce` 可开启拦截。
- 新增 PR 与 main 分支的 CI 检查。
- 新增更新包 minisign 签名：内置公钥的版本必须验签通过才会安装，发布流水线负责签名。

### ⚡ 优化 (Changed)
- 更新清单地址可在构建时通过 `OPENREVO_UPDATE_GITHUB_URL`、`OPENREVO_UPDATE_GITEE_URL` 配置。
- Release 构建忽略 `OPENREVO_UPDATE_URL`；更新清单缺少 SHA256 时拒绝更新。
- 修正 README 与插件手册中和实际行为不符的体积、安全描述。

### 🐛 修复 (Fixed)
- 修复 `open_external_url` 与 `open_log_file` 经 `cmd.exe` 解析参数导致的命令注入风险。
- 插件宿主只接受来自自身 iframe 的 `postMessage`；微插槽 iframe 增加 `sandbox` 属性。
- 修复发布流水线的 pnpm 安装失败问题。
- 优先通过 Intel DTT 读取 CPU 温度，修复部分机型因 ACPI 热区为固定值而显示恒定温度的问题。
- 修复星耀 14 (P916F) 第二风扇转速读取错误。
- 星耀 14 (P916F) 固件不提供充电上限接口：隐藏相关选项；清单未实现充电上限时不再静默返回成功。

### 🛡️ 硬件与安全 (Security / Hardware)
- 

---

<!-- 历史版本发布记录将归档于下方 -->
