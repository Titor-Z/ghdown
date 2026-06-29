# Changelog

## 2026.06.29.0007 — 多彩 --help 输出

- 使用 `clap::builder::styling::Styles` 自定义 help 样式
- 段头（Usage:/Commands:）→ **黄色加粗**
- 命令/参数名 → **青色加粗**
- 短参数（-o, -p）→ 黄色
- 占位符（<URL>, <OUTPUT>）→ 灰色
- 子命令也继承相同颜色风格
