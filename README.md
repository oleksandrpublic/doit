# do_it

[![Crates.io](https://img.shields.io/crates/v/do_it.svg)](https://crates.io/crates/do_it)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

An autonomous coding agent that runs local LLMs via [Ollama](https://ollama.com) to read, write, and fix code in your repositories. Works on Windows and Linux with no shell dependency, no Python, no cloud APIs.

Inspired by [mini-swe-agent](https://mini-swe-agent.com/latest/) — a minimal, transparent approach to software engineering agents. 

**do_it** extends that foundation with persistent memory, multi-role orchestration, sub-agents, GitHub integration, and a significantly expanded tool set. 

Most of the new features were designed and implemented by [Claude Sonnet 4.6](https://www.anthropic.com/claude).

---

## Features

- **Local-first** — runs entirely on your machine via Ollama, no cloud APIs required
- **Cross-platform** — Windows (MSVC) and Linux, no shell operators, no Python
- **Agent roles** — focused tool sets and prompts per task type: `boss`, `research`, `developer`, `navigator`, `qa`, `reviewer`, `memory`
- **Sub-agent orchestration** — `boss` role delegates to specialised sub-agents via `spawn_agent` or `spawn_agents` (parallel); results flow through shared memory
- **Persistent memory** — `.ai/` hierarchy: session notes, task plan, knowledge base, architectural decisions, lessons learned. Global `~/.do_it/` memory for user preferences and cross-project boss insights
- **Browser integration** — headless browser tools (`screenshot`, `browser_get_text`, `browser_action`, `browser_navigate`) via CDP; connect Chrome or Lightpanda by setting `cdp_url` in config
- **Background processes** — run long-running processes in the background with `run_background`, check status with `process_status`, kill with `process_kill`
- **Agent self-improvement** — `tool_request` and `capability_gap` tools let the Boss record missing capabilities to `~/.do_it/tool_wishlist.md`; review to prioritise new tool development
- **Project auto-detection** — `.ai/project.toml` scaffolded on first run with commands, GitHub repo, and agent conventions
- **GitHub integration** — `github_api` tool for issues, PRs, branches, commits, file contents (token from env)
- **Git tools** — full Git support including `git_pull`, `git_push`, `git_stash`, etc.
- **Test coverage** — `test_coverage` auto-detects Rust/Node/Python and runs the right tool
- **Telegram notifications** — `ask_human` for blocking questions, `notify` for non-blocking progress updates
- **Loop detection** — automatically detects stuck patterns and sends a Telegram alert
- **Model routing** — use different models per role (e.g. a large coder model for `developer`, a small fast one for `navigator`)
- **Vision support** — pass an image as `--task` for visual debugging (requires vision-capable model)

---

## Quick Start

```bash
# 1. Pull a model
ollama pull qwen3.5:9b

# 2. Install
cargo install do_it

# 3. Run
do_it run --task "Find and fix the bug in src/parser.rs" --repo /path/to/project

# With a role (recommended)
do_it run --task "Add input validation to handlers.rs" --role developer

# Orchestrate a complex task with sub-agents
do_it run --task "Add OAuth2 login to the API" --role boss --max-steps 80
```

---

## Roles

Each role restricts the agent to a focused set of tools and a role-specific system prompt. This is critical for smaller models — 6–8 tools instead of 20+ significantly improves output quality.

| Role | Purpose | Key tools |
|---|---|---|
| `default` | No restrictions | all tools |
| `boss` | Orchestration — plans tasks, delegates to sub-agents | `memory_read/write`, `tree`, `web_search`, `ask_human`, `spawn_agent`, `notify` |
| `research` | Information gathering | `web_search`, `fetch_url`, `memory_read/write`, `ask_human` |
| `developer` | Reading and writing code | `read/write_file`, `str_replace`, `run_command`, `diff_repo`, `git_*`, AST tools, `github_api`, `test_coverage`, `notify` |
| `navigator` | Exploring codebase structure | `tree`, `list_dir`, `find_files`, `search_in_files`, `find_references`, AST tools |
| `qa` | Testing and verification | `run_command`, `read_file`, `search_in_files`, `diff_repo`, `git_status`, `git_log`, `github_api`, `test_coverage`, `notify` |
| `reviewer` | Static code review — no execution | `read_file`, `search_in_files`, `find_references`, AST tools, `diff_repo`, `git_log`, `memory_read/write`, `ask_human` |
| `memory` | Managing `.ai/` state | `memory_read`, `memory_write` |

```bash
do_it roles   # list all roles and their tool allowlists
```

---

## Tools

All tools are implemented in native Rust with no shell dependency.

### Filesystem
`read_file`, `write_file`, `str_replace`, `list_dir`, `find_files`, `search_in_files`, `tree`

### Execution
`run_command`, `diff_repo`, `run_background`, `process_status`, `process_kill`, `process_list`

### Git
`git_status`, `git_commit`, `git_log`, `git_stash`, `git_pull`, `git_push`

### Internet
`web_search` (DuckDuckGo, no API key), `fetch_url`, `github_api`

### Code Intelligence (Rust, TypeScript, JavaScript, Python, C++, Kotlin)
`get_symbols`, `outline`, `get_signature`, `find_references`

### Testing
`test_coverage` (auto-detects Rust/Node/Python)

### Memory (.ai/ hierarchy)
`memory_read`, `memory_write`

### Communication
`ask_human` (Telegram or console), `notify` (one-way Telegram), `finish`

### Multi-agent
`spawn_agent`, `spawn_agents`

### Browser (requires [browser] in config.toml)
`screenshot`, `browser_get_text`, `browser_action`, `browser_navigate`

### Self-improvement
`tool_request`, `capability_gap`

---

## Sub-agent Orchestration

The `boss` role can spawn specialised sub-agents. Sub-agents run in-process with isolated history and communicate through shared `.ai/knowledge/` memory.

```bash
do_it run --task "Add OAuth2 login" --role boss --max-steps 80
```

```
boss: reads last_session, plan, decisions, user_profile
  │
  ├─ spawn_agent("research",  "find best OAuth crates for Axum",    memory_key="knowledge/oauth")
  ├─ spawn_agent("navigator", "locate existing auth middleware",     memory_key="knowledge/structure")
  ├─ spawn_agent("developer", "implement OAuth per the plan")
  ├─ screenshot("http://localhost:3080/login")   ← boss sees the result directly
  ├─ spawn_agent("reviewer",  "review the OAuth implementation",    memory_key="knowledge/review_report")
  ├─ spawn_agent("qa",        "verify all tests pass",              memory_key="knowledge/qa_report")
  └─ notify("OAuth complete, all tests pass") → finish
```

---

## Persistent Memory

```
.ai/
├── project.toml           ← auto-scaffolded on first run, edit freely
├── prompts/               ← custom role prompt overrides
├── state/
│   ├── current_plan.md        ← boss writes task breakdown here
│   ├── last_session.md        ← agent reads this on startup
│   ├── session_counter.txt
│   └── external_messages.md  ← external inbox, read and cleared on startup
├── logs/history.md
└── knowledge/
    ├── lessons_learned.md     ← QA appends project-specific patterns
    ├── decisions.md           ← architectural decisions and rationale
    └── qa_report.md           ← latest test results
```

Global memory in `~/.do_it/` persists across all projects and is read by the `boss` role at startup:

| File | Purpose |
|---|---|
| `user_profile.md` | Your preferences: language, stack, workflow style. Boss reads this every session. |
| `boss_notes.md` | Cross-project insights accumulated by Boss — patterns that apply beyond the current repo. |
| `tool_wishlist.md` | Missing capabilities recorded by the agent via `tool_request` and `capability_gap`. Review to prioritise new tool development. |

Edit `~/.do_it/user_profile.md` once and the boss will always know your stack and conventions.

---

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

# Optional: Telegram for ask_human and notify
# telegram_token   = "..."
# telegram_chat_id = "..."
```

Config priority: `--config` flag → `./config.toml` → `~/.do_it/config.toml` → built-in defaults.
On first run, `~/.do_it/` is created with a full template including `user_profile.md`, `boss_notes.md`, and `tool_wishlist.md`.

```bash
do_it config   # show resolved config
```

### Browser backend (optional)

```toml
[browser]
# Connect to a running CDP server — Chrome, Lightpanda, or any CDP-compatible browser
cdp_url = "ws://127.0.0.1:9222"

# Or launch Chrome locally (coming soon — requires chromiumoxide feature)
# chrome_path = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"

# Screenshot output directory (default: .ai/screenshots)
# screenshot_dir = ".ai/screenshots"
```

Start a CDP server:
```bash
# Chrome
google-chrome --headless --remote-debugging-port=9222

# Lightpanda (lightweight, AI-optimised, 9x less RAM than Chrome)
lightpanda serve --host 127.0.0.1 --port 9222
# or via Docker:
docker run -d -p 9222:9222 lightpanda/browser:nightly
```

---

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
do_it status
do_it init
```

---

## Roadmap

- [ ] Session reports and metrics tracking
- [ ] `do_it init` command for project setup
- [ ] `do_it status` command for project overview
- [ ] Ollama streaming support for real-time output
- [ ] GitHub Actions CI/CD
- [ ] Improved HTML extraction for `fetch_url` (readability algorithm)
- [ ] Tree-sitter backend for more accurate AST analysis
- [ ] Structured tool schemas (JSON Schema for function calling)
- [ ] Web search providers beyond DuckDuckGo (SearXNG, Brave Search API)
````
