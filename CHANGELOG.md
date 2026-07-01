# Changelog

## 2026.07.02.0012 — Windows self-upgrade 修复：rename+copy 方案

- Windows 下升级不再报"权限不足"：用 `rename(exe→.old)` 释放路径，再 `copy(tmp→exe)` 写入新文件
- 失败时自动恢复备份，不会丢旧文件
- Unix 保持原有 `rename` 逻辑不变
