# Changelog

## 2026.06.29.0010 — upgrade 子命令：自动升级

- 新增 `ghdown upgrade` 子命令，自动从 GitHub Release 下载最新版本并替换当前二进制
- 通过 GitHub API (`repos/anomalyco/ghdown/releases/latest`) 获取最新 release 信息
- 自动检测当前平台架构（x64/aarch64/arm × linux/windows/mac），匹配对应 asset
- 支持通过代理访问 API（先遍历代理再直连 fallback）
- 复用现有代理系统下载二进制文件，带进度条
- 下载后设置可执行权限（Unix），自动替换可执行文件
- 版本对比：与当前版本相同时直接提示已是最新
