# AGENTS.md

## Changelog

### 2026.06.29.0010 — upgrade 子命令：自动升级

- 新增 `ghdown upgrade` 子命令，自动从 GitHub Release 下载最新版本并替换当前二进制
- 通过 GitHub API (`repos/anomalyco/ghdown/releases/latest`) 获取最新 release 信息
- 自动检测当前平台架构（x64/aarch64/arm × linux/windows/mac），匹配对应 asset
- 支持通过代理访问 API（先遍历代理再直连 fallback）
- 复用现有代理系统下载二进制文件，带进度条
- 下载后设置可执行权限（Unix），自动替换可执行文件
- 版本对比：与当前版本相同时直接提示已是最新

### 2026.06.29.0001 — 简化重构：只接受 URL

- **彻底简化**：删除 release.rs（GitHub API）、列表、模式匹配等复杂功能
- CLI 只接受一个参数：`ghdown <URL>`，直接通过代理下载
- 保留：代理健康管理、自动测速、failover、断点续传
- 去掉子命令系统，保留 `-p`/`--proxy` 指定代理、`-o`/`--output` 指定输出路径、`--no-probe` 跳过测速
- 修复 URL 拼接 bug（缺少分隔符 `/`）
- 实测通过：成功下载 13MB GitHub Release 文件

### 2026.06.29.0002 — 支持 > 重定向 / 管道

- 检测 `stdout.is_terminal()`，当 stdout 被重定向或管道时，自动将二进制内容写入 stdout
- 所有状态/进度信息输出到 stderr，不污染管道数据
- 配合 `>` 重定向、`|` 管道均可正常工作
- 新增 `--file` 参数，强制写入文件（即使 stdout 被重定向）
- 重写了 CLI 帮助文本

### 2026.06.29.0003 — 代理运维管理子命令

- 恢复 `proxy` 子命令：`list` / `test` / `add` / `remove` / `reset` / `health`
- 下载时显示当前使用的代理和备选数量
- 添加 `--no-probe` 参数，跳过自动测速直接按列表顺序尝试
- 代理状态持久化在 `~/.config/ghdown/proxy_state.json`，重启后保留

### 2026.06.29.0004 — 并发分片下载

- 添加 `-j` / `--jobs` 参数（默认 4 线程）
- HEAD 探测文件大小 + Range 支持，不支持 Range 或 <10MB 自动回退单线程
- 每片独立 failover：代理失败自动换下一个，不相互影响
- 每分片最小 5MB，避免分片过小开销大于收益
- 使用 `MultiProgress` 显示每片进度条
- 下载完成后自动合并分片并清理临时文件

### 2026.06.29.0005 — --quiet 静默模式

- 添加 `-q` / `--quiet` 参数，完全屏蔽 stderr 信息（进度条、代理选择、切换提示等）
- 隐藏所有 ProgressBar / MultiProgress，下载静默运行
- 错误仍然打印（exit code ≠ 0 时），静默 ≠ 吞错误
- 适用场景：`ghdown -q URL | bash`、`ghdown -q URL | tar xz` 等一键管道

### 2026.06.29.0006 — 目标 URL 级探针

- 新增 `ProxyManager::probe_for_url()` 方法，对下载 URL 做并行 HEAD 探测
- 下载前对每个代理 HEAD `{proxy}{下载URL}`，按目标延迟重新排序
- 解决「favicon 能访问，但实际下载路径不通」的问题
- `--proxy` 或 `--no-probe` 时跳过此步骤（保持语义）

### 2026.06.29.0008 — CI/CD + 版本号 + 发布规范

- 新增 `build.rs`，从 `GHDOWN_VERSION` 环境变量动态读取版本号，降级到 `CARGO_PKG_VERSION`
- `ghdown --version` 在 CI 中显示 tag 版本（如 `2026.06.29.0008`），本地开发显示 `0.1.0`
- 新增 `.github/workflows/build.yml` — push 到 `main` 时矩阵构建 5 平台并上传产物
- 新增 `.github/workflows/release.yml` — 推送 `v*` 标签时矩阵构建 + 自动创建 GitHub Release
- Release 附件直接挂裸二进制文件，不打压缩包
- Release Notes 从 `CHANGELOG.md` 读取
- 新增 `CHANGELOG.md`，只保留当前版本内容，发版时从 AGENTS.md 同步
- 更新 AGENTS.md 规范，加入版本号、CI/CD、二进制命名、Changelog 管理规则

### 2026.06.29.0009 — 代理列表更新 + CI 精简 + 版本对齐

- 内置代理从 8 个扩充到 73 个（来自用户提供的公共代理列表）
- 移除 macOS-13（Intel Mac）CI runner，避免排队等待，保留 Apple Silicon
- `build.rs` 改用 `DEV_VERSION` 常量，`ghdown --version` 始终显示 AGENTS.md 版本号
- 新增 `cargo:rerun-if-env-changed`，环境变量 `GHDOWN_VERSION` 变更时自动重编
- 发版流程文档化到 AGENTS.md 认知修正

### 2026.06.29.0007 — 多彩 --help 输出

- 使用 `clap::builder::styling::Styles` 自定义 help 样式
- 段头（Usage:/Commands:）→ **黄色加粗**
- 命令/参数名 → **青色加粗**
- 短参数（-o, -p）→ 黄色
- 占位符（&lt;URL&gt;, &lt;OUTPUT&gt;）→ 灰色
- 子命令也继承相同颜色风格

## Taolun

### 2026-06-29 — 项目需求讨论与设计

用户提出做一个 GitHub Release 下的专门下载工具 `ghdown`，利用在线代理加速服务。

**关键需求：**
1. 使用 Rust 语言开发
2. 通过 GitHub 代理加速下载 release assets
3. 代理健康管理：出问题时自动切换
4. 支持断点续传
5. 内置常见代理源 + 支持自定义

**设计要点：**
- 代理状态机：Unknown → Healthy / Dead
- 健康评分体系：基于成功/失败计数加权
- 下载时 failover：当前代理失败自动切换到下一个健康代理
- 断点续传：基于 `.part` 临时文件 + HTTP Range 请求

**代理列表（内置 8 个）：**
- gh-proxy.com、github.akams.cn、mirror.ghproxy.com、ghproxy.net
- gh.zwy.one、gh.llkk.cc、ghproxy.cxkpro.top、gh.h233.eu.org

### 2026-06-29 — AGENTS.md 创建

创建了 AGENTS.md，包含 changelog、taolun、agents 三个主要板块，以及项目进度和开发流程规范。

## Agents

### 规范

1. 一个问题重复 3 次无法解决完成，强制停止，向用户详细汇报遇到的问题，等待用户解答
2. 整个对话流程中，全部强制使用中文，包括 AI 打印在终端中的内容
3. 项目必须有详细的中文注释
4. 测试文件按照功能模块拆分成多个文件，禁止在一个文件里写全部内容
5. 开发采用 OOP 面向对象方式，保持功能模块的单一，做到高内聚低耦合
6. 版本号格式为 `YYYY.MM.DD.xxxx`（如 `2026.06.29.0007`），同时写入 `Cargo.toml` 的 `version` 字段（3 段式，如 `2026.6.29`）
7. Git tag 格式为 `vYYYY.MM.DD.xxxx`，推送 tag 触发 `release.yml` 自动构建+发布
8. 二进制命名规则：`ghdown-{arch}-{platform}`，其中 arch = x64 / aarch64 / arm，platform = linux / windows / mac
9. 每次发版更新 `CHANGELOG.md`，只保留当前版本内容，Release Notes 从该文件读取
10. 所有正式构建必须通过 GitHub Actions 矩阵（5 平台），禁止手动发版

### 项目进度

#### 计划中

- 支持通过 GitHub API 列出 releases/assets
- 支持 aria2 导出
- 多文件并发下载
- 下载完成后自动校验（如果 release 提供了 checksum）

#### 待办

- 下载完成后自动校验 checksum
- aria2 导出模式

#### 已完成

- `2026.06.29.0000` 完整项目实现
  - config.rs — 配置读写、代理状态持久化
  - proxy.rs — 代理列表、状态机、健康探测、测速
  - download.rs — 下载核心、断点续传、failover
  - release.rs — GitHub API 查询 + 模式匹配
  - main.rs — CLI 参数解析、子命令分发
  - 编译通过，测试通过
- `2026.06.29.0010` upgrade 子命令
  - upgrade.rs — 自动检测平台、GitHub API 查询（代理 fallback）、下载并替换二进制

### 开发流程

1. 每次先保存讨论记录，然后再开始改动文件内容
2. 开发时，Windows 系统已内置 coreutils 组件，可使用 bash 命令（grep、ls、sed、find 等），无需使用 PowerShell cmdlet
3. 开发完成后，更新项目进度一栏和 changelog 下的内容。changelog 内容要与 taolun 记录和项目进度形成外链，方便后期溯源

### 认知修正

> 存放 AI 在开发过程中的踩坑记录和用户纠正、用户提醒和给予的明确内容，避免后期 agent 再次陷入死循环。

#### 发版流程（按顺序执行）

1. 在 `AGENTS.md` 的 `## Changelog` 中添加新条目：`### YYYY.MM.DD.xxxx — 标题`
2. 更新 `build.rs` 中的 `DEV_VERSION` 常量（例如 `"2026.06.29.0009"`）
3. 更新 `CHANGELOG.md`：用上一步写入 AGENTS.md 的最新条目内容覆盖全文
4. `git add -A && git commit -m "对应提交信息"`
5. 打标签：`git tag -a vYYYY.MM.DD.xxxx -m "..."`（-m 内容与 CHANGELOG.md 一致）
6. `git push && git push origin vYYYY.MM.DD.xxxx` — push 触发 CI 自动构建+发版
7. 不需要改 `Cargo.toml` 的 version（保持 `0.1.0`，仅作为 cargo 元数据）
