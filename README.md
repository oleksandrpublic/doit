# do_it

[![Crates.io](https://img.shields.io/crates/v/do_it.svg)](https://crates.io/crates/do_it)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

An autonomous coding agent that runs local LLMs via [Ollama](https://ollama.com) to read, write, and fix code in your repositories. Works on Windows and Linux with no shell dependency, no Python, no cloud APIs.

---

## Features

- **Local-first** — runs entirely on your machine via Ollama
- **Cross-platform** — Windows (MSVC) and Linux, no shell operators
- **Agent roles** — restrict tools and prompts per task type (`developer`, `navigator`, `qa`, `boss`, `research`, `memory`)
- **Persistent memory** — `.ai/` hierarchy: plan, last session notes, knowledge base
- **Rich tool set** — filesystem, git, web search, code intelligence (AST), Telegram notifications
- **Model routing** — use different models per role (e.g. a large coder model for `developer`, a small fast model for `navigator`)

## Quick Start

```bash
# 1. Pull a model
ollama pull qwen3.5:9b

# 2. Install
cargo install do_it

# 3. Run
do_it run --task "Find and fix the bug in src/parser.rs" --repo /path/to/project

# With a role (recommended for smaller models)
do_it run --task "Add input validation to handlers.rs" --role developer
```

## Roles

Each role restricts the agent to a focused set of tools and a role-specific system prompt. This is critical for smaller models — 6–8 tools instead of 20+ significantly improves output quality.

| Role | Purpose | Key tools |
|---|---|---|
| `developer` | Write and edit code | read/write file, str_replace, run_command, git, AST |
| `navigator` | Explore codebase structure | tree, find_files, search, outline, find_references |
| `research` | Find information | web_search, fetch_url, memory |
| `qa` | Run tests, verify changes | run_command, diff_repo, git_log, search |
| `boss` | Plan and orchestrate | memory, tree, web_search, ask_human |
| `memory` | Manage `.ai/` state | memory_read, memory_write |

```bash
do_it roles   # list all roles and their tool allowlists
```

## Tools

**Filesystem:** `read_file`, `write_file`, `str_replace`, `list_dir`, `find_files`, `search_in_files`, `tree`

**Execution:** `run_command`, `diff_repo`

**Git:** `git_status`, `git_commit`, `git_log`, `git_stash`

**Internet:** `web_search` (DuckDuckGo, no API key), `fetch_url`

**Code intelligence** (Rust, TypeScript, JavaScript, Python, C++, Kotlin):
`get_symbols`, `outline`, `get_signature`, `find_references`

**Memory** (`.ai/` hierarchy): `memory_read`, `memory_write`

**Communication:** `ask_human` (Telegram or console), `finish`

## Configuration

```toml
# config.toml
ollama_base_url  = "http://localhost:11434"
model            = "qwen3.5:9b"
temperature      = 0.0
max_tokens       = 4096
history_window   = 8
max_output_chars = 6000

# Optional: different models per role
[models]
coding    = "qwen3-coder-next"
search    = "qwen3.5:4b"
execution = "qwen3.5:4b"

# Optional: Telegram notifications for ask_human
# telegram_token   = "..."
# telegram_chat_id = "..."
```

```bash
do_it config   # show resolved config
```

## Memory hierarchy

The agent maintains persistent state in `.ai/` at the repository root:

```
.ai/
├── prompts/          ← custom role prompts (override built-ins)
├── state/
│   ├── current_plan.md
│   ├── last_session.md    ← agent reads this on startup
│   └── session_counter.txt
├── logs/history.md
└── knowledge/             ← agent-written notes about the project
```

At session start, `last_session.md` is automatically injected into context so the agent remembers what it did before.

Custom role prompts: create `.ai/prompts/developer.md` to override the built-in developer prompt for a specific project.

## CLI

```
do_it run    --task <text|file|image>
             --repo <path>          (default: .)
             --role <role>          (default: unrestricted)
             --config <path>        (default: config.toml)
             --system-prompt <text|file>
             --max-steps <n>        (default: 30)

do_it config [--config <path>]
do_it roles
```

## Roadmap

- [ ] `spawn_agent` — boss delegates subtasks to role-specific sub-agents
- [ ] `git_push` / `git_pull` structured tools
- [ ] Web search providers beyond DuckDuckGo
- [ ] Tree-sitter backend for more accurate AST analysis

## Authors

Built by [Claude Sonnet 4.6](https://www.anthropic.com/claude) with Oleksandr.

## License

MIT
