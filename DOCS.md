# do_it — Documentation

An autonomous coding agent powered by local LLMs via [Ollama](https://ollama.com). Reads, writes, and fixes code in your repositories. Runs on Windows and Linux with no shell dependency, no Python, no cloud APIs.

---

## Table of Contents

1. [Quick Start](#quick-start)
2. [Installation](#installation)
3. [Configuration](#configuration)
4. [CLI Reference](#cli-reference)
5. [Agent Roles](#agent-roles)
6. [Tools](#tools)
7. [Telegram — ask_human](#telegram--ask_human)
8. [How the Agent Works](#how-the-agent-works)
9. [Model Selection and Routing](#model-selection-and-routing)
10. [Sub-agent Architecture (Roadmap)](#sub-agent-architecture-roadmap)
11. [Limitations](#limitations)
12. [Tips and Recommendations](#tips-and-recommendations)
13. [Troubleshooting](#troubleshooting)
14. [Project Structure](#project-structure)

---

## Quick Start

```bash
# 1. Pull a model
ollama pull qwen3.5:9b

# 2. Build
cargo build --release

# 3. Run
./target/release/do_it run \
  --task "Find and fix the bug in src/parser.rs" \
  --repo /path/to/project

# With a role (recommended for smaller models)
./target/release/do_it run \
  --task "Add input validation to handlers.rs" \
  --role developer
```

Windows:
```powershell
.\target\release\do_it.exe run `
  --task "Find and fix the bug in src/parser.rs" `
  --repo C:\Projects\my-project `
  --role developer
```

---

## Installation

### Requirements

- [Rust](https://rustup.rs/) 1.85+ (edition 2024)
- [Ollama](https://ollama.com) running locally

### Build from source

```bash
git clone https://github.com/oleksandrpublic/doit
cd doit
cargo build --release
```

Binary: `target/release/do_it` (Linux) / `target\release\do_it.exe` (Windows).

### Install from crates.io

```bash
cargo install do_it
```

### Ollama setup

```bash
ollama pull qwen3.5:9b   # recommended default
ollama list              # verify installed models
curl http://localhost:11434/api/tags  # verify Ollama is running
```

---

## Configuration

All configuration lives in `config.toml`. If the file is not found, the agent runs with built-in defaults.

```toml
# Ollama endpoint
ollama_base_url  = "http://localhost:11434"

# Default model — used when no role-specific override is set
model            = "qwen3.5:9b"

# Sampling temperature: 0.0 = deterministic
temperature      = 0.0

# Max tokens per LLM response
max_tokens       = 4096

# Number of recent steps to include in full in context; older steps are collapsed to one line
history_window   = 8

# Max characters in tool output before truncation
max_output_chars = 6000

# System prompt (overridden by --role and --system-prompt)
system_prompt = """..."""

# Optional: Telegram for ask_human / notifications
# telegram_token   = "1234567890:ABCdef..."
# telegram_chat_id = "123456789"

# Optional: per-role model overrides
[models]
# thinking  = "qwen3.5:9b"
# coding    = "qwen3-coder-next"
# search    = "qwen3.5:4b"
# execution = "qwen3.5:4b"
# vision    = "qwen3.5:9b"
```

### Defaults

| Field | Default |
|---|---|
| `ollama_base_url` | `http://localhost:11434` |
| `model` | `qwen3.5:9b` |
| `temperature` | `0.0` |
| `max_tokens` | `4096` |
| `history_window` | `8` |
| `max_output_chars` | `6000` |

```bash
do_it config              # print resolved config
do_it config --config custom.toml
```

---

## CLI Reference

```
do_it run     Run the agent on a task
do_it config  Print resolved config and exit
do_it roles   List all roles with their tool allowlists
```

### `run` arguments

```
--task, -t        Task text, path to a .md file, or path to an image
--repo, -r        Repository / working directory (default: .)
--config, -c      Path to config.toml (default: ./config.toml)
--role            Agent role: boss | research | developer | navigator | qa | memory
--system-prompt   Override system prompt: inline text or path to a file
--max-steps       Maximum agent steps (default: 30)
```

### Examples

```bash
# Plain task
do_it run --task "Refactor the database module"

# With role (recommended)
do_it run --task "Add rate limiting" --role developer
do_it run --task "What does this project do?" --role navigator
do_it run --task "Find docs for tower-http middleware" --role research

# Task from file
do_it run --task tasks/issue-42.md --role developer --repo ~/projects/my-app

# Screenshot with an error (vision mode)
do_it run --task error-screenshot.png --role developer

# Custom prompt for a specific task
do_it run --task "Review only, do not edit" --system-prompt prompts/reviewer.md

# More steps for complex tasks
do_it run --task "Refactor auth module" --role developer --max-steps 50
```

`--task` and `--system-prompt`: if the value is a path to an existing file, its contents are read; if the extension is an image (`.png`, `.jpg`, `.webp`, etc.), vision mode is activated.

### Logging

```bash
RUST_LOG=info  do_it run --task "..."   # default
RUST_LOG=debug do_it run --task "..."   # includes raw LLM responses
RUST_LOG=error do_it run --task "..."   # errors only
```

---

## Agent Roles

Roles restrict the agent to a focused tool set and apply a role-specific system prompt. This is critical for smaller models — 6–8 tools instead of 20+ significantly improves output quality and reduces hallucinations.

```bash
do_it roles   # print all roles and their allowlists
```

### Role table

| Role | Purpose | Key tools |
|---|---|---|
| `default` | No restrictions | all tools |
| `boss` | Orchestration and planning | `memory_read/write`, `tree`, `web_search`, `ask_human` |
| `research` | Information gathering | `web_search`, `fetch_url`, `memory_read/write`, `ask_human` |
| `developer` | Reading and writing code | `read/write_file`, `str_replace`, `run_command`, `diff_repo`, `git_*`, AST tools |
| `navigator` | Exploring codebase structure | `tree`, `list_dir`, `find_files`, `search_in_files`, `find_references`, AST tools |
| `qa` | Testing and verification | `run_command`, `read_file`, `search_in_files`, `diff_repo`, `git_status`, `git_log` |
| `memory` | Managing `.ai/` state | `memory_read`, `memory_write` |

### System prompt priority

```
--system-prompt  (highest)
  ↓
--role           → looks for .ai/prompts/<role>.md, falls back to built-in
  ↓
system_prompt in config.toml
  ↓
built-in DEFAULT_SYSTEM_PROMPT  (lowest)
```

### Custom role prompts

Create `.ai/prompts/<role>.md` in the repository root to override the built-in prompt for that role:

```bash
mkdir -p .ai/prompts
cat > .ai/prompts/developer.md << 'EOF'
You are a Rust expert working on this codebase.
Always run `cargo clippy -- -D warnings` after edits.
Use thiserror for error types, never anyhow in library code.
All public functions require doc comments.
EOF
```

The file is picked up automatically — no restart needed.

---

## Tools

All tools are implemented in native Rust (`src/tools.rs`) with no shell dependency.

### Filesystem

| Tool | Arguments | Description |
|---|---|---|
| `read_file` | `path`, `start_line?`, `end_line?` | Read file with line numbers (default: first 100 lines) |
| `write_file` | `path`, `content` | Overwrite file, create directories if needed |
| `str_replace` | `path`, `old_str`, `new_str` | Replace a unique string in a file |
| `list_dir` | `path?` | List directory contents (one level) |
| `find_files` | `pattern`, `dir?` | Find files by name: `*.rs`, `test*`, substring |
| `search_in_files` | `pattern`, `dir?`, `ext?` | Regex search across file contents |
| `tree` | `dir?`, `depth?`, `ignore?` | Recursive directory tree (ignores `target`, `.git`, etc. by default) |

### Execution

| Tool | Arguments | Description |
|---|---|---|
| `run_command` | `program`, `args[]`, `cwd?` | Run a program with explicit args array (no shell) |
| `diff_repo` | `base?`, `staged?`, `stat?` | Git diff vs HEAD or any ref |

### Git

| Tool | Arguments | Description |
|---|---|---|
| `git_status` | `short?` | Working tree status and branch info |
| `git_commit` | `message`, `files?`, `allow_empty?` | Stage files and commit |
| `git_log` | `n?`, `path?`, `oneline?` | Commit history, optionally filtered by path |
| `git_stash` | `action`, `message?`, `index?` | Stash management: `push`, `pop`, `list`, `drop`, `show` |

### Internet

| Tool | Arguments | Description |
|---|---|---|
| `web_search` | `query`, `max_results?` | Search via DuckDuckGo (no API key required) |
| `fetch_url` | `url`, `selector?` | Fetch a web page and return readable text |

### Code Intelligence

Regex-based, supports Rust, TypeScript/JavaScript, Python, C++, Kotlin. Detected by file extension.

| Tool | Arguments | Description |
|---|---|---|
| `get_symbols` | `path`, `kinds?` | List all symbols: fn, struct, class, impl, enum, trait, type, const |
| `outline` | `path` | Structural overview with signatures and line numbers |
| `get_signature` | `path`, `name`, `lines?` | Full signature + doc comment for a named symbol |
| `find_references` | `name`, `dir?`, `ext?` | All usages of a symbol across the codebase |

`kinds` filter examples: `"fn,struct"`, `"class,interface"`, `"fn,method"`.

### Memory (`.ai/` hierarchy)

| Tool | Arguments | Description |
|---|---|---|
| `memory_read` | `key` | Read a memory entry |
| `memory_write` | `key`, `content`, `append?` | Write or append to a memory entry |

**Logical key mapping:**

| Key | File |
|---|---|
| `plan` | `.ai/state/current_plan.md` |
| `last_session` | `.ai/state/last_session.md` |
| `session_counter` | `.ai/state/session_counter.txt` |
| `external` | `.ai/state/external_messages.md` |
| `history` | `.ai/logs/history.md` |
| `knowledge/<n>` | `.ai/knowledge/<n>.md` |
| `prompts/<n>` | `.ai/prompts/<n>.md` |
| any other key | `.ai/knowledge/<key>.md` |

### Communication

| Tool | Arguments | Description |
|---|---|---|
| `ask_human` | `question` | Send a question via Telegram (if configured) or console |
| `finish` | `summary`, `success` | Signal task completion |

---

## Telegram — ask_human

### Setup

1. Create a bot via [@BotFather](https://t.me/BotFather) → copy the token
2. Send `/start` to your bot
3. Find your `chat_id` via [@userinfobot](https://t.me/userinfobot)
4. Add to `config.toml`:

```toml
telegram_token   = "1234567890:ABCdef-ghijklmnop"
telegram_chat_id = "123456789"
```

### How it works

When the agent calls `ask_human`:
1. Sends the question to Telegram
2. Polls every 5 seconds, waits up to 5 minutes for a reply
3. Your reply is returned to the agent as the tool result
4. If Telegram is not configured or unreachable — falls back to console stdin

---

## How the Agent Works

### Main loop (`src/agent.rs`)

```
session_init():
  → increment .ai/state/session_counter.txt
  → read last_session.md → inject as step 0 in history

if --task is an image:
  → vision model describes it → description becomes effective_task

for each step 1..max_steps:
  1. thinking model → JSON { thought, tool, args }
  2. check role tool allowlist (if role != Default)
  3. if specialist model differs → re-call with specialist
  4. execute tool
  5. record in history
  6. if tool == "finish" → done
```

### Context window (`src/history.rs`)

- Last `history_window` steps in full
- Older steps collapsed to one line: `step N ✓ [tool] → first line of output`

### LLM response parsing

Finds the first `{` and last `}` — handles models that add prose before or after JSON. Strips ` ```json ` fences automatically.

### Tool error handling

A tool error does not stop the agent — it is recorded in history as a failed step. The agent can recover and try a different approach on the next step.

---

## Model Selection and Routing

### Recommended models (qwen3.5 family)

| Model | Size | Use case |
|---|---|---|
| `qwen3.5:4b` | 3.4 GB | Roles with ≤8 tools (navigator, research) |
| `qwen3.5:9b` | 6.6 GB | Default — good balance |
| `qwen3.5:27b` | 17 GB | Complex multi-step tasks |
| `qwen3-coder-next` | ~52 GB | Developer role, best code quality |

### Per-role model routing

```toml
model = "qwen3.5:9b"   # fallback for all roles

[models]
coding    = "qwen3-coder-next"   # used for write_file, str_replace
search    = "qwen3.5:4b"        # used for read_file, find_files, etc.
execution = "qwen3.5:4b"        # used for run_command
```

### Remote Ollama

```toml
ollama_base_url = "http://192.168.1.100:11434"
```

---

## Sub-agent Architecture (Roadmap)

The current role system is the foundation for full sub-agent orchestration.

### Planned: `spawn_agent(role, task)`

A new tool that lets the `boss` agent delegate subtasks to specialised sub-agents. Each sub-agent runs with its own history, role prompt, and tool allowlist. The result is returned as a string to the boss's history.

### Execution flow (planned)

```
do_it run --task "Add OAuth" --role boss
  │
  ├─ boss: reads plan, decomposes task
  │
  ├─ spawn_agent(role="research", task="find best OAuth crates for Axum")
  │    └─ research agent → memory_write("knowledge/oauth_crates", ...)
  │
  ├─ spawn_agent(role="navigator", task="find current auth middleware location")
  │    └─ navigator agent → returns structure summary
  │
  ├─ spawn_agent(role="developer", task="implement OAuth per the plan")
  │    └─ developer agent → writes code, runs tests
  │
  └─ spawn_agent(role="qa", task="verify all tests pass")
       └─ qa agent → runs tests → writes report
```

### `.ai/` memory hierarchy (already implemented)

```
.ai/
├── prompts/               ← custom role prompts (override built-ins per project)
│   ├── boss.md
│   ├── developer.md
│   └── qa.md
├── state/
│   ├── current_plan.md    ← boss writes the task plan here
│   ├── last_session.md    ← read on startup, written at end of session
│   ├── session_counter.txt
│   └── external_messages.md
├── logs/
│   └── history.md
└── knowledge/             ← agent-written notes about the project
```

---

## Limitations

**Context window** — for long sessions reduce `history_window` to 4–5 and `max_output_chars` to 3000.

**`find_files`** — simple patterns only: `*.rs`, `test*`, substring. No `**`, `{a,b}`, or `?`.

**`run_command`** — no `|`, `&&`, `>`. Each command is a separate call with an explicit args array.

**`fetch_url`** — public URLs only, no authentication, no JavaScript rendering. For GitHub use `raw.githubusercontent.com`.

**`web_search`** — DuckDuckGo HTML endpoint, no API key required. Rate limiting possible under heavy use.

**Code intelligence** — regex-based, covers ~95% of real-world cases. Does not handle macros, conditional compilation, or dynamically generated code.

**Vision** — qwen3.5 supports images, but current GGUF files in Ollama may not include the mmproj component. Use llama.cpp directly for guaranteed vision support.

---

## Tips and Recommendations

### Choose the right role

```bash
do_it run --task "What does this project do?"            --role navigator
do_it run --task "Find examples of using axum extractors" --role research
do_it run --task "Add validation to src/handlers.rs"     --role developer
do_it run --task "Do all tests pass?"                    --role qa
do_it run --task "Plan the auth module refactor"         --role boss
```

### Before running

```bash
git status           # start from a clean working tree
ollama list          # verify required models are available
```

### Write a good task description

```bash
# Vague — bad
do_it run --task "Improve the code"

# Specific — good
do_it run --task "In src/lexer.rs the tokenize function returns an empty Vec \
  for input containing only spaces. Fix it and add a regression test." --role developer

# From a file with full context
do_it run --task tasks/issue-42.md --role developer
```

A task file can include reproduction steps, logs, expected vs actual behaviour, and any relevant constraints.

### Project-specific role prompts

```bash
mkdir -p .ai/prompts
cat > .ai/prompts/developer.md << 'EOF'
You are a Rust expert on this codebase.
Always run `cargo clippy -- -D warnings` after edits.
Always run `cargo test` after edits.
Use thiserror for error types in library code.
All public items require doc comments.
EOF
```

---

## Troubleshooting

**`Tool 'X' is not allowed for role 'Y'`** — the model tried to use a tool outside the role's allowlist. Either switch to `--role default` or add the tool to `.ai/prompts/<role>.md` with an explicit mention.

**`Cannot reach Ollama at http://localhost:11434`**
```bash
ollama serve
curl http://localhost:11434/api/tags
```

**`Model 'X' not found`**
```bash
ollama pull qwen3.5:9b
ollama list
```

**`LLM response has no JSON`** — the model responded outside of JSON format. Try a larger model, set `temperature = 0.0`, or use a role to reduce the tool count. Add `IMPORTANT: respond ONLY with a JSON object.` to the system prompt.

**`str_replace: old_str found N times`** — provide more context to make `old_str` unique:
```json
{ "old_str": "fn process(x: i32) -> i32 {\n    x + 1", "new_str": "..." }
```

**`ask_human via Telegram: no reply received within 5 minutes`** — verify the bot token and chat_id, and that you have sent `/start` to the bot. The agent continues with an error and falls back to console.

**`fetch_url: HTTP 403` or empty result** — the site blocks bots or requires JavaScript. Use direct API endpoints instead: `raw.githubusercontent.com` instead of `github.com`.

**Agent is looping** — reduce `history_window` to 4, restart with a more specific task, or use a larger model.

---

## Project Structure

```
do_it/
├── Cargo.toml           name="do_it", edition="2024", version="0.2.0"
├── config.toml          runtime configuration (models, Telegram, prompt)
├── README.md            project overview
├── DOCS.md              this file
├── LICENSE              MIT
├── .gitignore
├── .ai/                 agent memory (created automatically, gitignored)
│   ├── prompts/         custom role prompts
│   ├── state/           plan, last_session, session_counter
│   ├── logs/            history.md
│   └── knowledge/       agent-written project notes
└── src/
    ├── main.rs          CLI: run | config | roles; --role flag; image detection
    ├── agent.rs         main loop, role enforcement, tool allowlist, session_init
    ├── config.rs        AgentConfig, Role enum + allowlists + built-in prompts, ModelRouter
    ├── history.rs       sliding window context manager
    ├── shell.rs         Ollama HTTP client: chat, chat_with_image, check_models
    └── tools.rs         all 19 ACI tools; 6-language AST parsers
```

### Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | async runtime |
| `reqwest` | HTTP — Ollama, fetch_url, web_search, Telegram API |
| `serde` / `serde_json` | JSON serialization |
| `toml` | config.toml parsing |
| `clap` | CLI argument parsing |
| `walkdir` | recursive filesystem traversal |
| `regex` | search_in_files, find_references, AST parsers |
| `base64` | image encoding for vision |
| `anyhow` | error handling |
| `tracing` / `tracing-subscriber` | structured logging |
