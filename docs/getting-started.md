# Getting Started

## Installation

```bash
# Build from source
cargo build --release

# Binary location
./target/release/dream
```

## Command Format

```
dream [OPTIONS] [PROMPT]...
```

- With `PROMPT`: single-shot mode — completes the task and exits
- Without `PROMPT`: enters interactive REPL mode

> For the full list of CLI parameters, run `dream --help`.

### Subcommands

Management operations live under noun subcommands instead of flat flags. A
subcommand runs its action and exits — it does not start the agent main flow.

| Subcommand | Description |
|------------|--------------|
| `dream config init` | Generate a default global config file |
| `dream config path` | Print the global config file path |
| `dream auth login` | Login with Anthropic account (OAuth device flow) |
| `dream auth logout` | Logout (remove saved OAuth credentials) |
| `dream session list` | List saved sessions |
| `dream skills path` | Print skill directory paths |

### Key Parameters

| Parameter | Description |
|-----------|-------------|
| `--provider <name>` | Provider: `anthropic`, `openai`, `bedrock`, `vertex`, or a custom alias |
| `--model <id>` | Model name |
| `--profile <name>` | Named profile from config file |
| `--compaction <level>` | Output compaction: `off`, `safe` (default), `full` |
| `--toon` | Enable TOON tabular encoding (with `full` compaction) |
| `--max-turns <n>` | Broad model-turn limit per run; unset by default, `0` disables |
| `--max-tool-call-malformed-turns <n>` | Stop after repeated same tool-call-malformed rounds; `0` disables |
| `--max-tool-call-failure-turns <n>` | Stop after repeated tool-call-failure rounds; `0` disables |
| `--auto-approve` | Skip all tool confirmations |
| `--json-stream` | JSON Lines mode for host integration |
| `--resume <id>` | Resume a previous session |
| `--fork-session` | With `--resume`: fork into a new session id, leaving the original untouched |
| `--log-dir <path>` | Enable file logging to the given directory |
| `--log-level <filter>` | Log level filter (e.g. `debug`, `info`, `dream_engine_providers=debug`) |

---

## Configuration

### Three-Level Cascading

```
<global config>                   (global, user-level; run `dream config path` to find)
    ↓ overridden by
./.dream.toml                  (project-level, working directory)
    ↓ overridden by
CLI parameters / env vars        (highest priority)
```

### Generate Default Config

```bash
dream config init
# Creates the global config file (run `dream config path` to see the location)
```

### Config File Format

```toml
# Global config file (path varies by OS, use `dream config path` to find)

[default]
provider = "anthropic"
# model = "claude-sonnet-4-20250514"
# max_tokens = 8192  # optional per-response output cap; omit to use provider/model defaults
# max_turns = 20  # optional max model turns per run; omit or set 0 to disable
max_tool_call_malformed_turns = 3  # default; set 0 to disable this breaker
max_tool_call_failure_turns = 3  # default; set 0 to disable this breaker

[providers.anthropic]
# api_key = "sk-ant-xxx"       # or env var ANTHROPIC_API_KEY
# base_url = "https://api.anthropic.com"

[providers.openai]
# api_key = "sk-xxx"           # or env var OPENAI_API_KEY
# base_url = "https://api.openai.com/v1"

# Custom provider alias
[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"

# Named profiles, switch with --profile <name>
[profiles.deepseek]
provider = "openai"
model = "deepseek-chat"
api_key = "sk-xxx"
base_url = "https://api.deepseek.com/v1"

[profiles.deepseek-v4-pro]
provider = "openai"
model = "deepseek-v4-pro"
api_key = "sk-xxx"
base_url = "https://api.deepseek.com/v1"
max_tokens = 16384

[profiles.deepseek-v4-pro.compat]
supports_thinking = true

[profiles.ollama]
provider = "openai"
model = "qwen2.5:32b"
api_key = "ollama"
base_url = "http://localhost:11434"

[profiles.my-service]
provider = "my-service"

# Profile names are user-defined; this is not a built-in profile.
[profiles.my-weak-provider]
provider = "openai"
max_tool_call_malformed_turns = 2
max_tool_call_failure_turns = 2

[tools]
auto_approve = false
allow_list = ["Read", "Grep", "Glob"]

[session]
enabled = true
directory = ".dream/sessions"
max_sessions = 20

[compact]
compaction = "safe"   # off | safe | full
toon = false          # Enable TOON encoding for JSON arrays
# autocompact_threshold_pct = 50  # trigger autocompact at N% of context window

[file_cache]
enabled = true
max_entries = 100

[plan]
enabled = true
plan_directory = ".dream/plans"

# [logging]
# enabled = true              # enable file logging (default: false)
# level = "info"              # log level filter (default: "info")
# dir = "/path/to/logs"       # log directory (default: platform-specific)
```

### Runtime Limits

`max_turns` is the broad model-turn limit per run. It is unset by default, so
runs have no broad model-turn limit unless you configure one. Set it to `0` to
explicitly disable the broad limit. See [Core Concepts](core-concepts.md) for
the distinction between runs, turns, tool rounds, and tool calls.

`max_tool_call_malformed_turns` limits consecutive same tool-call-malformed
rounds from a provider. The default is `3`; `0` disables this breaker
and leaves stopping to `max_turns` if a broad turn limit is configured.

`max_tool_call_failure_turns` limits consecutive repeats of the same failed
tool name and input pattern. Assistant explanation text does not reset the
count, and successful sibling calls in a mixed round do not erase failures
that are still repeating. A failure-free tool round resets the exact-call and
cycle history. The default is `3`.

The same guard also warns after 3 consecutive all-error rounds and finalizes
after 8, and detects repeating 2-4 round call cycles after 2 repetitions before
finalizing after 3. Setting `max_tool_call_failure_turns` to `0` disables all
of these tool-failure guards and leaves stopping to `max_turns` if a broad turn
limit is configured.

Precedence is `CLI > profile > project config > global config > built-in default 3`. Use `--max-tool-call-malformed-turns <n>` or `--max-tool-call-failure-turns <n>` for a one-off CLI override.

### API Key Resolution Order

1. `--api-key` CLI parameter
2. Config file `providers.<name>.api_key`
3. Env var `API_KEY`
4. Env var `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` (depends on provider)
5. OAuth credentials (via `dream auth login`)

> **Note**: `bedrock` and `vertex` providers use their own cloud credentials and do not require a traditional API key. See [Providers & Auth](providers.md).

### Custom Provider Alias

如果某个后端兼容内置 provider 的协议，可以在 `providers.<alias>` 下声明一个 alias：

```toml
[default]
provider = "my-service"

[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"
```

- `default.provider` 和 `profile.provider` 都可以写 alias 名称
- `providers.<alias>.provider` 必须声明底层类型，目前只能是 `anthropic`、`openai`、`bedrock`、`vertex`
- alias 条目会覆盖对应底层 provider 的默认配置

---

## Quick Start

### 1. Initialize and Configure

```bash
dream config init
# Edit the config file (run `dream config path` to find it), add your API key
```

### 2. Single-Shot Mode

```bash
dream "Read and explain crates/dream-engine-agent/src/engine.rs"
```

### 3. Interactive terminal UI

```bash
dream
```

When stdin and stdout are attached to a terminal, `dream` keeps finalized
conversation in native terminal scrollback and renders an inline composer at
the bottom, with streaming responses, tool activity, and in-place approval
prompts. Type `/` at the beginning of the composer to
open the command popup, continue typing to filter it, use Up/Down to move, and
press Tab to complete or Enter to run the selected command.

Each thinking segment starts with `•`. Consecutive tool calls are collected
under a single `• Tools` step and appended as nested rows, so a batch remains
easy to scan without losing its individual calls. Tool rows update in place as
`queued`, `approval`, `running`, `done`, `failed`, or `cancelled`; the status
label and semantic color change together. Tool input and output are displayed
as a responsive one-line preview, while the full value remains in the saved
conversation.

| Key | Action |
|-----|--------|
| Enter | Send the current message |
| Shift+Enter | Insert a newline (Ctrl+J is available as a fallback) |
| Up/Down | Select a slash command |
| Mouse wheel | Scroll finalized conversation in the terminal's native scrollback |
| Ctrl+C | Stop the active turn; clear a draft or quit while idle |
| Ctrl+D | Quit while the composer is empty |

Mouse capture is deliberately disabled. Drag across any visible conversation
text and use the terminal's normal copy shortcut (`Cmd+C` on macOS or usually
`Ctrl+Shift+C` on Linux/Windows terminals).

Agent commands are `/compact`, `/context`, `/clear`, `/help`, and `/quit`.
The interactive UI also provides:

| Command | Description |
|---------|-------------|
| `/status` | Show provider, model, session, permission mode, and context usage |
| `/model [name]` | Show the current model or switch to a model ID |
| `/permissions [default\|auto_edit\|yolo]` | Show or change the tool approval mode |
| `/new` | Start a new session without exiting the interactive UI |
| `/resume [id\|latest]` | Open the session picker or resume an ID in place |
| `/mcp` | Show connected MCP servers and tool counts |
| `/skills` | Show model-visible skills loaded for the current runtime |

The session picker replaces the conversation with a focused full-screen list.
Use the mouse wheel or Up/Down to move, Enter to resume, and Esc to close it.
All saved sessions are shown in last-update order; the number retained on disk
is controlled by `session.max_sessions` (20 by default).
Resumed conversations are restored into the terminal's native scrollback. Use
the mouse wheel to move between the earliest and latest turns.

`/help` combines both command groups. `/exit` is an alias for `/quit`. Empty
messages no longer exit interactive mode. Model switching accepts an explicit
model ID; an interactive model candidate picker is not available yet.

When input or output is redirected, `dream` keeps the plain line-oriented REPL
for compatibility with scripts and terminals that do not support the interactive UI.

### 4. Switching Profiles

```bash
dream --profile deepseek "Fix the bug in main.rs"
dream --profile ollama "Analyze code quality"
```

### 5. Environment Variables

```bash
export ANTHROPIC_API_KEY=sk-ant-xxx
dream "List all Rust files in this project"
```

---

## Tool Confirmation

Tools that require approval are displayed in an in-place dialog in the terminal
UI. Press `y` or Enter to allow once, `a` to allow the category for the rest of
the session, or `n`/Esc to deny. The plain REPL uses the equivalent text prompt:

```
[tool] Write({"file_path": "/tmp/test.rs", "content": "..."})
Allow? [y]es / [n]o / [a]lways / [q]uit > y
```

| Option | Description |
|--------|-------------|
| `y` / `yes` / Enter | Allow this execution |
| `n` / `no` | Deny — LLM receives a "denied" error |
| `a` / `always` | Auto-approve this tool for the rest of the session |
| `q` / `quit` | Abort the entire agent run |

- Read-only tools (Read, Grep, Glob) are auto-approved by default
- `--auto-approve` skips all confirmations
- `tools.allow_list` in config customizes the whitelist

---

## Session Management

Sessions auto-save to `.dream/sessions/<session-id>/state.json`. Sessions
created by versions that used the duplicated
`.dream/sessions/sessions/<session-id>/state.json` layout remain readable and
are copied to the normalized location when resumed.

```bash
# List saved sessions
dream session list

# Resume the latest session
dream --resume latest

# Resume a specific session
dream --resume a1b2c3

# Fork a session: copy its history into a new session id and continue
# there, leaving the original session untouched
dream --resume a1b2c3 --fork-session

# Create a session with a custom ID
dream --session-id my-conv-123
```

- `--session-id` and `--resume` are mutually exclusive
- `--fork-session` requires `--resume`; the forked session records its
  parent in `forked_from` and the fork-tree root in `root_id`
- `--session-id` errors if the ID already exists
- Both flags work in interactive and `--json-stream` mode
- Auto-saves after each tool round
- Auto-cleans oldest sessions when exceeding `max_sessions`
