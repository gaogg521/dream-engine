# Dream Engine — 迁移设置说明

对应旧仓库 `aionrs-local`：Agent 引擎、CLI/TUI、Provider、工具、MCP 客户端，目标 CLI 命令名 `dream`（取代 `aionrs`）。

这是一份迁移期间的说明文件，完整决策背景见 `D:\aionui-m0\DREAM-PLATFORM-DIRECTION.md`。

## 本次复制说明（2026-08-23）

源码已从 `D:\aionui-m0\aionrs-local` 原样复制过来（不含改名），排除 `.git`、`target/`、`node_modules/`、`out/`、`dist/`、`coverage/`、`.turbo/`。复制干净，0 失败（447/447 文件）。

## Rust crate 映射（P0 初稿，共 16 个 package）

**15 个 `aion-*` → `dream-engine-*`**：`dream-engine-agent`、`dream-engine-cli`（内含二进制 `aionrs` → `dream`）、`dream-engine-compact`、`dream-engine-config`、`dream-engine-mcp`、`dream-engine-memory`、`dream-engine-process`、`dream-engine-protocol`、`dream-engine-providers`、`dream-engine-skills`、`dream-engine-tools`、`dream-engine-tui`、`dream-engine-types`。

`workspace-hack` 不改名（Cargo workspace 内部机制用途）。

⚠️ `dream-engine-process`/`dream-engine-mcp` 与 `dream-core`（原 1oneCore）里的 `aionui-process`/`aionui-mcp` 是真实存在的命名冲突，见 dream-core 的 `DREAM-SETUP-NOTES.md`。

## 引擎专用环境变量（真实运行时变量，可以直接按规则改名）

实测确认（非 NSIS 宏名，是真正的引擎运行时变量）：`AIONRS_MEMORY_DIR`、`AIONRS_SKILL_DIR`、`AIONRS_SESSION_ID` → 分别对应 `DREAM_MEMORY_DIR`、`DREAM_SKILL_DIR`、`DREAM_SESSION_ID`。

项目配置文件 `.aionrs.toml`、`.aionrs/` 目录 → `.dream.toml`、`.dream/`。

## 已完成（2026-08-23）

- [x] `cargo check --workspace` 通过
- [x] 13 个 `aion-*` crate 目录 + Cargo.toml + 所有 `.rs` 内的 `aion_*` 模块路径全部改为 `dream-engine-*`/`dream_engine_*`
- [x] 二进制名 `aionrs` → `dream`；workspace `repository` 字段指向新仓库
- [x] `AIONRS_MEMORY_DIR`/`AIONRS_SKILL_DIR`/`AIONRS_SESSION_ID` → `DREAM_MEMORY_DIR`/`DREAM_SKILL_DIR`/`DREAM_SESSION_ID`
- [x] TUI 欢迎界面里用户可见的 `AionCLI` 文案 → `DreamCLI`（这是真实用户可见文案，不同于 appId 那类不可见标识符）
- [x] insta 快照文件（`crates/dream-engine-providers/src/snapshots/*.snap`）已连文件名带内部 `source:` 路径一起改名
- [x] 全仓（含 docs/、.github/、CI 配置）扫描确认干净，唯一保留的是 `CHANGELOG.md` 里 188 处历史 commit 链接——**这些是故意保留的历史记录**（按决策文档"历史记录可以保留必要的历史名称"原则，且这些链接本身指向真实存在的 `iOfficeAI/aionrs` 历史 commit，改写会得到断链）
- [x] 未发现 `.aionrs.toml`/`.aionrs/` 字面量路径存在于当前代码里，这条待办作废
- [ ] `Cargo.lock` 已删除等待下次 `cargo build` 重新生成（原文件锁定的是旧路径哈希）
- [ ] 引擎自身 OAuth client ID / User-Agent / MCP clientInfo 的 Dream 身份尚未设计（决策文档第 8 节待决事项）
- [ ] 尚未提交、尚未推送到 `https://github.com/gaogg521/dream-engine.git`
