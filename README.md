# do_it

[![Crates.io](https://img.shields.io/crates/v/do_it.svg)](https://crates.io/crates/do_it)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

An autonomous coding agent powered by local or cloud LLMs. Reads, writes, and fixes code in your repositories. Works on Windows and Linux with no shell dependency, no Python.

Supports **Ollama** (local), **OpenAI-compatible**, and **Anthropic-compatible** backends — including self-hosted services and third-party providers such as MiniMax.

Inspired by [mini-swe-agent](https://mini-swe-agent.com/latest/) — a minimal, transparent approach to software engineering agents.

**do_it** extends that foundation with persistent memory, multi-role orchestration, sub-agents, a live terminal UI, and an optional tool surface controlled per role.

Most of the new features were designed and implemented by [Claude Sonnet 4.6](https://www.anthropic.com/claude).

---

## Features

- **Pluggable LLM backends** — Ollama (local), OpenAI-compatible, Anthropic-compatible; configure per project or per action type
- **Local-first option** — runs entirely on your machine via Ollama, no cloud required
- **Cross-platform** — Windows (MSVC) and Linux, no shell operators, no Python
- **Agent roles** — focused tool sets and prompts per task type: `boss`, `research`, `developer`, `navigator`, `qa`, `reviewer`, `memory`
- **Role budgets** — each named role has ≤ 12–14 core tools; smaller models stay focused and produce better output
- **Optional tool groups** — `browser`, `background`, `github` added only when configured in `config.toml`
- **Sub-agent orchestration** — `boss` delegates to specialised sub-agents via `spawn_agent` / `spawn_agents`; results flow through shared `.ai/knowledge/` memory
- **Live terminal UI** — three-panel Ratatui TUI: progress stats, scrollable step log, status bar; falls back to plain text in CI
- **Persistent memory** — `.ai/` hierarchy: session notes, task plan, knowledge base, architectural decisions, lessons learned
- **Session artifacts** — markdown session reports plus structured `session-NNN.trace.json` traces for lightweight replay, inspection, and safety diagnostics; sensitive tokens in task/summary text and write-tool output are redacted before any artifact is written
- **Telegram integration** — `ask_human` for blocking questions (TUI suspends cleanly), `notify` for non-blocking updates
- **GitHub integration** — `github_api` tool for issues, PRs, branches, commits (optional group)
- **Browser tools** — `screenshot`, `browser_get_text`, `browser_action`, `browser_navigate` via CDP (optional group)
- **Sandboxed scripting** — experimental `run_script` tool for quick parsing, JSON inspection, and lightweight automation
- **Model routing** — different models per action type (thinking, coding, search, execution, vision)
- **Vision support** — pass an image as `--task` for visual debugging
- **Agent self-improvement** — Boss records missing capabilities to `~/.do_it/tool_wishlist.md`

---

## For Those Who Like Surprises

This is a program that writes itself.

At the beginning, it needed help — ideas and the first steps.
Once everything started working, the model began improving and evolving the system on its own.

All you need is proper configuration and a sufficiently capable model, for example `qwen3.5:cloud`.
And of course — constraints and oversight.

Just configure it and tell it what you would like to add, improve, or change.

And don't forget to check `tool_wishlist.md`.

---

## Quick Start

```bash
# 1. Install
cargo install do_it

# 2. Initialise a project (interactive — choose backend, URL, model, API key)
cd /path/to/project
do_it init

# 3. Run
do_it run --task "Find and fix the bug in src/parser.rs"

# With a role (recommended for smaller models)
do_it run --task "Add input validation to handlers.rs" --role developer

# Orchestrate a complex task with sub-agents
do_it run --task "Add OAuth2 login to the API" --role boss --max-steps 80
```

For Ollama specifically:
```bash
ollama pull qwen3.5:cloud
do_it init --backend ollama --model qwen3.5:cloud --yes
```

For OpenAI or compatible service:
```bash
do_it init --backend openai --llm-url https://api.openai.com --model gpt-4o
# prompts for API key interactively
```

---

## Roles

Each role restricts the agent to a focused set of tools and a role-specific system prompt. This is critical for smaller models — 12 tools instead of 30+ significantly improves output quality and reduces hallucinations.

| Role | Purpose | Core tools |
|---|---|---|
| `default` | No restrictions | all tools |
| `boss` | Orchestration — plans, delegates, never writes code directly | `memory`, `tree`, `project_map`, `web_search`, `ask_human`, `notify`, `spawn_agent/s`, `tool_request`, `capability_gap` |
| `research` | Information gathering | `web_search`, `fetch_url`, `memory`, `ask_human` |
| `developer` | Write and run code — uses navigator sub-agent for exploration | `read_file`, `write_file`, `str_replace`, `apply_patch_preview`, `run_command`, `run_targeted_test`, `format_changed_files_only`, `run_script`, `git_*`, `memory`, `notify` |
| `navigator` | Explore codebase structure — read-only | `read_file`, `list_dir`, `find_files`, `search_in_files`, `tree`, `get_symbols`, `outline`, `find_references`, `project_map`, `trace_call_path`, `memory` |
| `qa` | Run tests, coverage, check diffs, find regressions | `read_file`, `search_in_files`, `run_command`, `run_script`, `test_coverage`, `diff_repo`, `read_test_failure`, `git_*`, `memory`, `notify` |
| `reviewer` | Static code review — no execution | `read_file`, `search_in_files`, `diff_repo`, `git_log`, `get_symbols`, `outline`, `get_signature`, `find_references`, `ask_human`, `memory` |
| `memory` | Managing `.ai/` state | `memory_read`, `memory_write`, `memory_delete` |

```bash
do_it roles   # list all roles and their tool allowlists
```

### Optional tool groups

Enable additional tool sets in `config.toml`:

```toml
tool_groups = ["browser", "github"]   # add browser tools and GitHub API
# tool_groups = ["browser", "background", "github"]  # all optional groups
```

| Group | Tools added | Roles |
|---|---|---|
| `browser` | `screenshot`, `browser_get_text`, `browser_action`, `browser_navigate` | boss, developer, qa, reviewer |
| `background` | `run_background`, `process_status`, `process_list`, `process_kill` | boss, developer |
| `github` | `github_api` | developer, qa |

---

## Sub-agent Orchestration

The `boss` role delegates all technical work to specialised sub-agents:

```bash
do_it run --task "Add OAuth2 login" --role boss --max-steps 80
```

```
boss: reads last_session, plan, decisions, user_profile
  │
  ├─ spawn_agents([
  │    { role: "research",  task: "find best OAuth crates for Axum",  key: "knowledge/oauth"     }
  │    { role: "navigator", task: "locate existing auth middleware",   key: "knowledge/structure" }
  │  ])                                              ← parallel, independent
  │
  ├─ spawn_agent("developer", "implement OAuth per the plan",          key: "knowledge/impl")
  ├─ spawn_agent("reviewer",  "review the OAuth implementation",       key: "knowledge/review")
  ├─ spawn_agent("qa",        "verify all tests pass",                 key: "knowledge/qa")
  └─ notify("OAuth complete") → finish
```

Sub-agents run in-process with isolated history and tool allowlists. The boss only reads results — it never writes code directly.

---

## Live TUI

When running in an interactive terminal, do_it shows a three-panel live view:

```
┌─────────────────────────────────────────────────────────────┐
│  do_it   task: "Add OAuth2 login"   role: boss   0:03:21    │
├──────────────────────┬──────────────────────────────────────┤
│  PROGRESS            │  STEP LOG                            │
│  Step:  7 / 50  ████ │  step 1  ✓  project_map  → found     │
│  Role:  boss         │  step 2  ✓  spawn_agents → started   │
│  Elapsed: 0:03:21    │  step 3  ✓  memory_read  → loaded    │
│  ETA:   ~0:08:00     │  step 4  ·  spawn_agent  running...  │
│  Tokens step:        │                                      │
│    in  2,847 out 312 │                                      │
│  Tokens total:       │                                      │
│    in 18,442 out 2k  │                                      │
├──────────────────────┴──────────────────────────────────────┤
│  boss → spawning developer for OAuth implementation  q=quit │
└─────────────────────────────────────────────────────────────┘
```

Keys: `q` / `Ctrl-C` = graceful stop, `↑↓` = scroll step log.
Falls back to plain text output in CI or when stdout is not a TTY.

---

## Persistent Memory

```
.ai/
├── project.toml           ← auto-scaffolded, edit freely
├── prompts/               ← custom role prompt overrides per project
├── state/
│   ├── current_plan.md
│   ├── last_session.md    ← agent reads this on startup
│   ├── task_state.json    ← structured working memory, survives interruption
│   └── external_messages.md  ← external inbox, cleared on startup
├── logs/
│   ├── history.md
│   ├── session-NNN.md         ← per-session markdown report: steps, tools, outcome, path-sensitivity summary
│   └── session-NNN.trace.json ← structured session trace: start, turns, finish, path-sensitivity diagnostics
└── knowledge/
    ├── lessons_learned.md
    ├── decisions.md
    └── qa_report.md
```

`do_it status` surfaces these artifacts directly:
- session report count and latest report filenames
- structured trace count and latest trace path
- compact path-sensitivity summary from the latest trace, when present
- last session note, current plan, wishlist summary, and knowledge keys

Final plain-text session close-out also shows a compact `Safety : ...` line when the session performed path-sensitive writes.

Global memory in `~/.do_it/` persists across all projects:

| File | Purpose |
|---|---|
| `user_profile.md` | Your preferences: language, stack, workflow style |
| `boss_notes.md` | Cross-project insights accumulated by Boss |
| `tool_wishlist.md` | Missing capabilities recorded via `tool_request` / `capability_gap` |

---

## Configuration

```toml
# config.toml

# ── LLM backend ───────────────────────────────────────────────────────────────
# llm_backend: "ollama" | "openai" | "anthropic"
llm_backend      = "ollama"
llm_url          = "http://localhost:11434"
# llm_api_key    = ""          # or set LLM_API_KEY env var

model            = "qwen3.5:cloud"
temperature      = 0.0
max_tokens       = 4096
history_window   = 8
max_output_chars = 6000
log_level        = "info"    # error | warn | info | debug | trace
log_format       = "text"    # text | json

# Optional: enable additional tool groups
# tool_groups = ["browser", "github"]

# Optional: different models per action type
[models]
coding    = "qwen3-coder-next:cloud"
search    = "qwen3.5:9b"
execution = "qwen3.5:9b"

# Optional: Telegram
# telegram_token   = "..."
# telegram_chat_id = "..."

# Optional: browser backend
# [browser]
# cdp_url = "ws://127.0.0.1:9222"
```

Config priority: `--config` → `./config.toml` → `~/.do_it/config.toml` → built-in defaults.

The `llm_api_key` field can also be supplied via the `LLM_API_KEY` environment variable — useful for CI or when you don't want keys in config files.

```bash
do_it config   # show resolved config
do_it roles    # list roles and their tool counts
```

---

## CLI

```
do_it run  --task <text|file|image>
           --repo <path>           (default: .)
           --role <role>           (default: unrestricted)
           --config <path>
           --system-prompt <text|file>
           --max-steps <n>         (default: 30)

do_it config [--config <path>]
do_it roles
do_it status [--repo <path>]
```

---

## Current Status

**Version:** 0.3.2

Real sub-agent delegation, live TUI, role budgets, optional tool groups, and pluggable LLM backends are all working. Extended testing in progress before next crates.io publish.

Recent hardening (2026-03-28):
- **Pluggable LLM backend** — `llm_backend = "ollama" | "openai" | "anthropic"` in config.toml; API key via `llm_api_key` or `LLM_API_KEY` env var; compatible with any OpenAI- or Anthropic-compatible service
- **`do_it init` updated** — now prompts for backend, URL, model, and API key; generates a correct `config.toml` for any backend
- `boss` role now reliably orchestrates multi-step tasks — loop detection threshold raised
- UTF-8 safe throughout — no panics on non-ASCII task text or file contents
- `memory_write` supports `append=true` and namespaced keys (`knowledge/decisions` etc.)
- `memory_delete` added
- HTTP timeouts on all LLM backends — a hung model no longer blocks the agent forever
- `run_command` output always includes stderr — cargo warnings and rustfmt diagnostics are visible
- TUI panic hook — terminal is restored cleanly on crash

### Planned next
- More iteration helpers around the Rust-first edit/test loop (smarter diff helpers, patch-shaping)
- Node/Python fallback branches for helper tools after the Rust-first path is solid

`run_command` accepts only bare executable names from `PATH`, enforces timeout/arg/env limits, and blocks risky environment overrides.
