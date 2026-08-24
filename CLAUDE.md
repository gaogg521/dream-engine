@AGENTS.md

## 项目定位

**dream-engine** 是 **One Work** 平台的 Agent 引擎（Rust CLI/TUI），一个连接 LLM API、自主调用本地工具（文件读写/Shell/搜索等）完成任务的命令行 Agent。本项目最初基于开源项目 aionrs 二次开发，**现已完全独立成自有平台，不再跟随或合并上游**，技术前缀统一为小写 `dream`，二进制名 `dream`。

> **代码溯源**：本仓库 2026-08-23 从旧仓库 `D:\aionui-m0\aionrs-local`（原始最上游是开源项目
> aionrs）**原样复制的一次性快照**，不含 `.git` 历史。如果在本仓库里发现某个功能/文件
> "应该存在但找不到"，先去 `D:\aionui-m0\aionrs-local` 翻一下。`D:\aionui-m0` 三仓
> （`1oneUI`/`1oneCore`/`aionrs-local`）定位是只读归档，不再往里提交新代码。

> 本仓库这一轮品牌独立化没有代码改动（纯 CLAUDE.md 重写）。持久化数据迁移的完整过程
> （dream-ui/dream-core 两仓的详细记录）见
> [dream-ui/docs/guides/session-2026-08-23-dream-rebrand-data-migration.zh-CN.md](https://github.com/gaogg521/dream-ui/blob/main/docs/guides/session-2026-08-23-dream-rebrand-data-migration.zh-CN.md)。

## 三仓架构

| 仓库 | 角色 | 关键产物 |
| --- | --- | --- |
| **[dream-ui](https://github.com/gaogg521/dream-ui)** | Electron 桌面、React UI、WebUI 静态资源 | 安装包 |
| **[dream-core](https://github.com/gaogg521/dream-core)** | Rust 本地服务 | `dreamcore` 二进制，通过 `dream-engine-* = { git = "...", branch = "main" }` 直接依赖本仓库 `main` 分支 |
| **dream-engine**（本仓库） | Agent 引擎、CLI/TUI、Provider、工具、MCP 客户端 | `dream` 二进制 |

改完本仓库 `main` 分支并推送后，dream-core 那边 `cargo build`（或 `cargo update -p dream-engine-*`）即可对齐到最新版本。详见 [README.md](./README.md)（英文技术文档，Quick Start / Architecture / Providers 等）。

## 品牌与技术身份

用户可见产品名是 **One Work**（首字母大写、中间有空格，容易被误传成 "OneWork"/"ONE WORK"，改动前以 dream-ui 的 `BRAND_DISPLAY_NAME` 常量为准），但本仓库是纯技术引擎，几乎不直接面向最终用户展示品牌名。技术前缀统一小写 `dream`：CLI 命令 `dream`、crate 名 `dream-engine-*`。

## 持久化/跨进程取值改名的铁律

本仓库的 `LlmEvent`、`AgentSessionKind`、工具契约等如果被 dream-core 反序列化消费，改动前先确认协议双方是否同步：

- 纯内部实现细节（crate 内部函数名、私有类型）可以直接改。
- 跨进程/跨 crate 边界的枚举、事件类型改名，若字段值会被持久化（如落盘的 session 状态文件）或被 dream-core 一侧按字符串匹配读取，需要保持向后兼容或与 dream-core 同步更新，否则升级瞬间旧数据/旧调用方会解析失败。
- 改完只看 `cargo build` 通过不够——协议层的字符串不匹配大多是运行时错误，编译期看不出来。

## 测试

```powershell
cargo nextest run --workspace   # 推荐，比 cargo test 快很多
cargo test --workspace
```

## 关键设计决策（供快速定位历史脉络，非详尽变更记录）

- **多级 thinking 重试阶梯**（原样/content-block/省略/文本化）：应对不同网关对 `thinking` 参数支持程度不一的问题，是本仓库独立于任何上游的基础设施，不依赖模型名硬判断。
- **有界续写处理输出截断**：Provider 响应因输出上限被截断时，最多 12 轮有界续写逐段拼接，并通过 `LlmEvent::ToolCallTruncated` 让截断的工具调用可被感知和恢复，而不是静默丢弃或误判为正常完成。OpenAI 与 Anthropic 两种协议路径都需要独立处理 EOF 断连（未见终止帧）与半截 JSON 两类截断场景。
- **视觉委托的用量上报**：`ReadImage` 委托视觉模型读图时的 token 用量通过 `DelegateUsageSink` 上报给宿主（dream-core），费率匹配用委托模型名而非会话模型名，避免宿主端的成本上限/账本出现幽灵调用或错误归因。
- 改动这些机制级修复时优先看是否已有 `ProviderCompat` 配置项可用，避免退化为 `if model == "..."` 这类按模型名硬编码的特判。
