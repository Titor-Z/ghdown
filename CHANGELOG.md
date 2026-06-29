# Changelog

## 2026.06.29.0009 — 代理列表更新 + CI 精简 + 版本对齐

- 内置代理从 8 个扩充到 73 个（来自用户提供的公共代理列表）
- 移除 macOS-13（Intel Mac）CI runner，避免排队等待，保留 Apple Silicon
- `build.rs` 改用 `DEV_VERSION` 常量，`ghdown --version` 始终显示 AGENTS.md 版本号
- 新增 `cargo:rerun-if-env-changed`，环境变量 `GHDOWN_VERSION` 变更时自动重编
- 发版流程文档化到 AGENTS.md 认知修正
