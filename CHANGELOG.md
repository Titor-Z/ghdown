# Changelog

## 2026.07.02.0011 — Checksum 校验：发布 .sha256 + 下载自动验证

- CI 构建后自动生成 `.sha256` 校验文件，随 release 一起上传
- 下载时尝试拉取同名 `{url}.sha256` 文件（先走代理再直连 fallback），解析验证 checksum
- 去掉 `try_resolve_digest_from_url` 对 GitHub API `assets[].digest` 的依赖（该字段不存在）
- 改用 `sha256sum` 标准格式解析，统一 `sha256:hex` digest 格式
- 校验不匹配时打印警告（不中断下载），旧 release 无 `.sha256` 文件时静默跳过
