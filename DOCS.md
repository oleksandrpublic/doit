# do_it — Documentation

An autonomous coding agent powered by local LLMs via [Ollama](https://ollama.com). 

Reads, writes, and fixes code in your repositories. 

Runs on Windows and Linux with no shell dependency, no Python, no cloud APIs.

---

## Table of Contents

1. [Quick Start](#quick-start)
2. [Installation](#installation)
3. [Configuration](#configuration)
4. [CLI Reference](#cli-reference)
5. [Agent Roles](#agent-roles)
6. [Tools](#tools)
7. [Telegram — ask_human and notify](#telegram--ask_human-and-notify)
8. [Browser Tools](#browser-tools)
9. [Agent Self-Improvement](#agent-self-improvement)
10. [How the Agent Works](#how-the-agent-works)
11. [Model Selection and Routing](#model-selection-and-routing)
12. [Sub-agent Architecture](#sub-agent-architecture)
13. [Persistent Memory](#persistent-memory)
14. [Limitations](#limitations)
15. [Tips and Recommendations](#tips-and-recommendations)
16. [Troubleshooting](#troubleshooting)
17. [Project Structure](#project-structure)

---

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

Configuration is loaded from the first file found in this priority order:

1. `--config <path>` — explicit path passed on the command line
2. `./config.toml` — local project config (in the working directory)
3. `~/.do_it/config.toml` — global user config (created automatically on first run)
4. Built-in defaults

On first run, `~/.do_it/` is created with a default `config.toml` and a `system_prompt.md` template.

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

# Optional: Telegram for ask_human / notify
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
--role            Agent role: boss | research | developer | navigator | qa | reviewer | memory
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

# Orchestrate a complex task with sub-agents
do_it run --task "Add OAuth2 login" --role boss --max-steps 60

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
| `boss` | Orchestration — plans tasks, delegates to sub-agents | `memory_read/write`, `tree`, `web_search`, `ask_human`, `spawn_agent`, `notify` |
| `research` | Information gathering | `web_search`, `fetch_url`, `memory_read/write`, `ask_human` |
| `developer` | Reading and writing code | `read/write_file`, `str_replace`, `run_command`, `diff_repo`, `git_*`, AST tools, `github_api`, `test_coverage`, `notify` |
| `navigator` | Exploring codebase structure | `tree`, `list_dir`, `find_files`, `search_in_files`, `find_references`, AST tools |
| `qa` | Testing and verification | `run_command`, `read_file`, `search_in_files`, `diff_repo`, `git_status`, `git_log`, `github_api`, `test_coverage`, `notify` |
| `reviewer` | Static code review — no execution | `read_file`, `search_in_files`, `find_references`, AST tools, `diff_repo`, `git_log`, `memory_read/write`, `ask_human` |
| `memory` | Managing `.ai/` state | `memory_read`, `memory_write` |

### System prompt priority

```
--system-prompt  (highest)
  ↓
--role           → looks for .ai/prompts/<role>.md, falls back to built-in
  ↓
system_prompt in config.toml
  ↓
~/.do_it/system_prompt.md  (global override)
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

### Background Processes

| Tool | Arguments | Description |
|---|---|---|
| `run_background` | `program`, `args[]`, `cwd?`, `id` | Start a process in the background with a named ID |
| `process_status` | `id` | Check if a background process is still running |
| `process_kill` | `id` | Terminate a background process |
| `process_list` |  | List all active background processes |

### Git

| Tool | Arguments | Description |
|---|---|---|
| `git_status` | `short?` | Working tree status and branch info |
| `git_commit` | `message`, `files?`, `allow_empty?` | Stage files and commit |
| `git_log` | `n?`, `path?`, `oneline?` | Commit history, optionally filtered by path |
| `git_stash` | `action`, `message?`, `index?` | Stash management: `push`, `pop`, `list`, `drop`, `show` |
| `git_pull` | `remote?`, `branch?` | Pull from remote repository |
| `git_push` | `remote?`, `branch?`, `force?` | Push to remote repository |

### Internet

| Tool | Arguments | Description |
|---|---|---|
| `web_search` | `query`, `max_results?` | Search via DuckDuckGo (no API key required) |
| `fetch_url` | `url`, `selector?` | Fetch a web page and return readable text |
| `github_api` | `method`, `endpoint`, `body?`, `token?` | GitHub REST API — issues, PRs, branches, commits, file contents |

`github_api` requires a `GITHUB_TOKEN` environment variable (or `token` argument). Responses are automatically filtered to keep context concise — file contents are base64-decoded automatically.

Common endpoints:
```
GET  /repos/{owner}/{repo}/issues              — list open issues
GET  /repos/{owner}/{repo}/issues/{n}          — read single issue
POST /repos/{owner}/{repo}/issues/{n}/comments — post a comment
PATCH /repos/{owner}/{repo}/issues/{n}         — update issue (state/labels)
GET  /repos/{owner}/{repo}/pulls               — list open PRs
POST /repos/{owner}/{repo}/pulls               — create PR
PUT  /repos/{owner}/{repo}/pulls/{n}/merge     — merge PR
GET  /repos/{owner}/{repo}/branches            — list branches
GET  /repos/{owner}/{repo}/contents/{path}     — read file (auto-decoded)
```

### Code Intelligence

Regex-based, supports Rust, TypeScript/JavaScript, Python, C++, Kotlin. Detected by file extension.

| Tool | Arguments | Description |
|---|---|---|
| `get_symbols` | `path`, `kinds?` | List all symbols: fn, struct, class, impl, enum, trait, type, const |
| `outline` | `path` | Structural overview with signatures and line numbers |
| `get_signature` | `path`, `name`, `lines?` | Full signature + doc comment for a named symbol |
| `find_references` | `name`, `dir?`, `ext?` | All usages of a symbol across the codebase |

`kinds` filter examples: `"fn,struct"`, `"class,interface"`, `"fn,method"`.

### Testing

| Tool | Arguments | Description |
|---|---|---|
| `test_coverage` | `dir?`, `threshold?` | Run tests with coverage (auto-detects Rust/Node/Python) |

`test_coverage` detects the project type from `Cargo.toml` / `package.json` / `pyproject.toml` and runs the appropriate tool (`cargo tarpaulin`, `jest --coverage`, `pytest --cov`). Falls back to `cargo test` if tarpaulin is not installed. Returns `success=false` if coverage is below `threshold` (default: 80%).

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
| `user_profile` | `~/.do_it/user_profile.md` (global) |
| `boss_notes` | `~/.do_it/boss_notes.md` (global) |
| any other key | `.ai/knowledge/<key>.md` |

**Persistent knowledge keys used by built-in roles:**

| Key | Written by | Purpose |
|---|---|---|
| `knowledge/lessons_learned` | QA | Project-specific pitfalls and correct patterns |
| `knowledge/decisions` | Boss, Developer | Architectural decisions and rationale |
| `knowledge/qa_report` | QA | Latest test run report |
| `knowledge/review_report` | Reviewer | Latest static code review |
| `knowledge/<role>_result` | Sub-agents | Results from `spawn_agent` calls |

### Communication

| Tool | Arguments | Description |
|---|---|---|
| `ask_human` | `question` | Send a question via Telegram and wait up to 5 min for reply; falls back to console |
| `notify` | `message`, `silent?` | Send a one-way Telegram notification (non-blocking, no waiting) |
| `finish` | `summary`, `success` | Signal task completion |

### Multi-agent

| Tool | Arguments | Description |
|---|---|---|
| `spawn_agent` | `role`, `task`, `memory_key?`, `max_steps?` | Delegate a subtask to a specialised sub-agent |
| `spawn_agents` | `agents[]` | Spawn multiple sub-agents in parallel |

### Browser

Requires `[browser]` to be configured in `config.toml`. The agent speaks CDP — the backend is transparent (Chrome, Lightpanda, or any CDP-compatible server).

| Tool | Arguments | Description |
|---|---|---|
| `screenshot` | `url`, `wait_ms?`, `full_page?` | Navigate to URL, take PNG screenshot; returns file path + base64 for vision model |
| `browser_get_text` | `url`, `selector?`, `wait_ms?` | Fetch page text after JavaScript renders; use instead of `fetch_url` for SPAs |
| `browser_action` | `action`, `selector`, `value?`, `wait_ms?` | Interact with an element: `click`, `type`, `hover`, `clear`, `select` |
| `browser_navigate` | `url`, `wait_ms?` | Navigate and wait for page load; takes implicit screenshot |

If `[browser]` is not configured, tools return a helpful setup message instead of failing silently.

### Self-improvement

| Tool | Arguments | Description |
|---|---|---|
| `tool_request` | `name`, `description`, `motivation`, `priority?` | Request a new tool — appends to `~/.do_it/tool_wishlist.md` |
| `capability_gap` | `context`, `impact` | Report a structural blind spot without a specific solution — appends to wishlist |

`tool_request` and `capability_gap` are available to `boss` and `default` roles. The Boss calls `tool_request` when it encounters a missing capability for the second time in any session, and `capability_gap` when it observes something it structurally cannot do.

See [Sub-agent Architecture](#sub-agent-architecture) for details.

---

## Telegram — ask_human and notify

### Setup

1. Create a bot via [@BotFather](https://t.me/BotFather) → copy the token
2. Send `/start` to your bot
3. Find your `chat_id` via [@userinfobot](https://t.me/userinfobot)
4. Add to `config.toml`:

```toml
telegram_token   = "1234567890:ABCdef-ghijklmnop"
telegram_chat_id = "123456789"
```

### ask_human

Sends a question and waits up to 5 minutes for your reply. Your reply is returned to the agent as the tool result. Falls back to console stdin if Telegram is not configured or unreachable.

Use this when the agent needs a decision before continuing — it will not guess on important choices.

### notify

Sends a one-way message with no waiting. Used for progress updates and completion notices during long autonomous runs. Falls back to stdout if Telegram is not configured.

```json
{ "tool": "notify", "args": { "message": "OAuth implementation complete, running tests..." } }
{ "tool": "notify", "args": { "message": "All tests pass. PR created.", "silent": true } }
```

### External messages (inbox)

To send instructions to the agent before its next run, write to `.ai/state/external_messages.md`. On startup, the agent reads this file, injects it into context, and clears it. Any external process — a webhook, a cron script, another agent — can write to this file.

```bash
echo "## 2024-01-15 10:30
Please also update the README after the refactor." >> .ai/state/external_messages.md
```

The next run will show: `[inbox] 2 external message(s) received`.

---

## Browser Tools

Browser tools give the agent eyes — the ability to see rendered pages, interact with UI elements, and take screenshots. This is essential for JavaScript-heavy applications (React, Vue, Leptos) where `fetch_url` returns an empty shell.

### Setup

The agent speaks CDP (Chrome DevTools Protocol). The backend is transparent — swap `cdp_url` to change from Chrome to Lightpanda or any future CDP-compatible browser without changing any agent code.

```toml
# config.toml
[browser]
# Option 1: connect to a running CDP server
cdp_url = "ws://127.0.0.1:9222"

# Option 2: launch Chrome locally (requires chromiumoxide feature — coming in a future version)
# chrome_path = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"

# Screenshot output directory (default: .ai/screenshots)
# screenshot_dir = ".ai/screenshots"
```

Start a CDP server before running the agent:

```bash
# Chrome (headless)
google-chrome --headless --remote-debugging-port=9222

# Lightpanda — lightweight, designed for AI, 9x less RAM than Chrome
# Linux/macOS binary:
lightpanda serve --host 127.0.0.1 --port 9222

# Docker:
docker run -d --name lightpanda -p 9222:9222 lightpanda/browser:nightly
```

### How the agent uses browser tools

```
screenshot(url)          → PNG saved to .ai/screenshots/
                           base64 returned for vision model input
browser_get_text(url)    → full page text after JS execution
browser_action(...)      → click/type/hover + implicit screenshot
browser_navigate(url)    → navigate + wait + screenshot
```

The Boss uses screenshots to verify UI work directly, without delegating:

```
boss: spawn_agent("developer", "add login form to /login")
boss: screenshot("http://localhost:3080/login")  ← sees result
boss: browser_action("type", "#email", "test@example.com")
boss: browser_action("click", "#submit")
boss: screenshot("http://localhost:3080/dashboard")  ← sees result after login
```

The Developer uses browser tools for visual feedback after UI changes:
```
developer: write_file("src/components/Login.rs", ...)
developer: run_command("trunk", ["build"])
developer: screenshot("http://localhost:3080/login")  ← verify rendering
```

### Vision model integration

`screenshot` returns the image as base64. Pass it to a vision-capable model for deeper analysis:

```bash
# Take a screenshot and describe it
do_it run --task screenshot.png --role developer
```

If `[browser]` is not configured, all browser tools return a setup message explaining what to add to `config.toml`. They never fail silently.

---

## Agent Self-Improvement

The Boss accumulates knowledge about missing capabilities across sessions. Two tools write to `~/.do_it/tool_wishlist.md`:

### tool_request

Called when the Boss encounters a missing capability for the **second time** in any session. Not for first encounters — the agent tries to work around once, then requests.

```json
{
  "tool": "tool_request",
  "args": {
    "name": "run_background",
    "description": "Run a process in the background and keep it alive",
    "motivation": "Need to start trunk serve and then navigate to localhost, but run_command blocks",
    "priority": "high"
  }
}
```

### capability_gap

Called when the Boss observes a structural blind spot — it cannot see or reach something important — and has no specific solution to propose.

```json
{
  "tool": "capability_gap",
  "args": {
    "context": "Developer wrote a Leptos component but I cannot see how it renders without a browser",
    "impact": "Visual bugs and layout issues go undetected until manual review"
  }
}
```

### Reading the wishlist

```bash
cat ~/.do_it/tool_wishlist.md
```

Each entry is timestamped and structured. The wishlist is your primary source for understanding what the agent actually needs — derived from real tasks, not speculation.

---

## How the Agent Works

### Session initialisation (`src/agent.rs`)

```
session_init():
  → increment .ai/state/session_counter.txt
  → read last_session.md              → inject into history as step 0
  → read user_profile.md (Boss only)  → inject into history (global preferences)
  → read boss_notes.md   (Boss only)  → inject into history (cross-project insights)
  → read external_messages.md         → inject into history, then clear the file
  → read/scaffold .ai/project.toml   → inject into history as project context
```

On first run in a new repository, `.ai/project.toml` is scaffolded automatically by detecting `Cargo.toml` / `package.json` / `pyproject.toml` / `go.mod` and reading the GitHub remote from `.git/config`. Edit it freely — it will not be overwritten.

```toml
# .ai/project.toml (auto-generated, edit as needed)
[project]
name     = "my-project"
language = "rust"

[commands]
test  = "cargo test"
build = "cargo build --release"
lint  = "cargo clippy -- -D warnings"

[github]
repo = "owner/my-project"

[agent]
notes = """
- Always run clippy before committing
"""
```

### Main loop

```
if --task is an image:
  → vision model describes it → description becomes effective_task

for each step 1..max_steps:
  1. thinking model → JSON { thought, tool, args }
  2. check role tool allowlist (if role != Default)
  3. if specialist model differs → re-call with specialist
  4. execute tool
  5. record in history
  6. loop detection → notify if stuck
  7. if tool == "finish" → done
```

### Loop detection

After each step the agent checks for two stuck patterns:

- **Repeated failures** — same tool failed 3 times in a row
- **Repeated calls** — same tool with identical args called 4 times in a row

When detected: logs a warning and sends a Telegram notification. The agent does not stop — it continues and may self-correct, but you are informed.

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

## Sub-agent Architecture

`spawn_agent` lets the `boss` role delegate subtasks to specialised sub-agents. Each sub-agent runs in-process with its own history, role prompt, and tool allowlist. Communication between boss and sub-agents goes through the shared `.ai/knowledge/` memory.

### Usage

```json
{
  "tool": "spawn_agent",
  "args": {
    "role": "research",
    "task": "Find the best OAuth2 crates for Axum in 2024",
    "memory_key": "knowledge/oauth_research",
    "max_steps": 15
  }
}
```

After the sub-agent finishes, the boss reads the results:

```json
{ "tool": "memory_read", "args": { "key": "knowledge/oauth_research" } }
```

### Arguments

| Argument | Required | Description |
|---|---|---|
| `role` | yes | Sub-agent role: `research`, `developer`, `navigator`, `qa`, `reviewer`, `memory` |
| `task` | yes | Task description — what the sub-agent should do |
| `memory_key` | no | Where to write results (default: `knowledge/agent_result`) |
| `max_steps` | no | Step limit (default: parent's `max_steps / 2`, min 5) |

Boss cannot spawn another boss (recursion guard).

### Full orchestration example

```bash
do_it run --task "Add OAuth2 login to the API" --role boss --max-steps 80
```

```
boss: reads last_session, plan, decisions, user_profile → writes task breakdown to plan
  │
  ├─ spawn_agent(role="research", task="find best OAuth crates for Axum",
  │              memory_key="knowledge/oauth_research")
  │    └─ research agent searches web, writes findings
  │
  ├─ memory_read("knowledge/oauth_research")
  │
  ├─ spawn_agent(role="navigator", task="find current auth middleware location",
  │              memory_key="knowledge/auth_structure")
  │    └─ navigator explores codebase, maps existing code
  │
  ├─ spawn_agent(role="developer", task="implement OAuth2 per the plan",
  │              memory_key="knowledge/impl_notes")
  │    └─ developer writes code, runs tests, commits
  │
  ├─ spawn_agent(role="reviewer", task="review the OAuth2 implementation",
  │              memory_key="knowledge/review_report")
  │    └─ reviewer reads code, checks decisions.md, writes structured report
  │
  ├─ spawn_agent(role="qa", task="verify all tests pass, check coverage",
  │              memory_key="knowledge/qa_report")
  │    └─ qa runs test_coverage, writes report, appends lessons_learned
  │
  └─ boss: reads review_report + qa_report → notify("OAuth2 complete") → finish
```

---

## Persistent Memory

The agent maintains persistent state in `.ai/` at the repository root. This directory is gitignored by default.

```
.ai/
├── project.toml           ← project config (auto-scaffolded, edit freely)
├── prompts/               ← custom role prompts (override built-ins per project)
│   ├── boss.md
│   ├── developer.md
│   └── qa.md
├── state/
│   ├── current_plan.md        ← boss writes the task plan here
│   ├── last_session.md        ← read on startup, written at end of session
│   ├── session_counter.txt
│   └── external_messages.md  ← external inbox, read and cleared on startup
├── logs/
│   └── history.md
└── knowledge/                 ← agent-written project knowledge
    ├── lessons_learned.md     ← QA appends project-specific patterns after each session
    ├── decisions.md           ← Boss/Developer log architectural decisions + rationale
    └── qa_report.md           ← latest test run
```

### What each file is for

**`last_session.md`** — the agent writes a note to its future self at the end of every session: what was done, what is pending, any important context. Read automatically on next startup.

**`external_messages.md`** — your inbox for the agent. Write anything here before a run; the agent will see it on startup and the file is cleared. Use this for instructions that don't fit as a `--task` flag, or to send notes from an external process.

**`project.toml`** — permanent project context injected every session. Commands for test/build/lint, GitHub repo name, agent conventions. Edit once, used forever.

**`lessons_learned.md`** — QA appends project-specific anti-patterns and correct approaches after each session. The agent reads this before starting work to avoid repeating mistakes.

**`decisions.md`** — Boss and Developer log significant architectural decisions: what was chosen, what alternatives were considered, and why. Consulted before redesigning anything.

**`review_report.md`** — Reviewer writes a structured static analysis after each review session: architectural issues, code smells, convention violations, potential bugs, with per-finding severity ratings.

### Global memory — `~/.do_it/`

Two files in `~/.do_it/` persist across all projects. The `boss` role reads them automatically at the start of every session (if they contain actual content, not just the default comments).

| File | Key | Purpose |
|---|---|---|
| `user_profile.md` | `user_profile` | Your preferences: communication language, tech stack, preferred crates, workflow style. Edit once, applies to all projects. |
| `boss_notes.md` | `boss_notes` | Cross-project insights the Boss accumulates — patterns that work, approaches to avoid, ideas for future projects. |

Boss reads these files and appends to them over time:
- When it learns something stable about you → updates `user_profile` via `memory_write("user_profile", ...)`
- When it discovers a cross-project insight → appends to `boss_notes` via `memory_write("boss_notes", ..., append=true)`

Both files are created with commented templates on first run. They are only injected into context when they contain actual content (not just `#` comment lines).

---

## Limitations

**Context window** — for long sessions reduce `history_window` to 4–5 and `max_output_chars` to 3000.

**`find_files`** — simple patterns only: `*.rs`, `test*`, substring. No `**`, `{a,b}`, or `?`.

**`run_command`** — no `|`, `&&`, `>`. Each command is a separate call with an explicit args array.

**`fetch_url`** — public URLs only, no authentication, no JavaScript rendering. For GitHub use `raw.githubusercontent.com`.

**`web_search`** — DuckDuckGo HTML endpoint, no API key required. Rate limiting possible under heavy use.

**Code intelligence** — regex-based, covers ~95% of real-world cases. Does not handle macros, conditional compilation, or dynamically generated code.

**`test_coverage`** — requires `cargo-tarpaulin` for Rust coverage numbers. Install with `cargo install cargo-tarpaulin`. Falls back to `cargo test` without coverage % if not installed.

**Vision** — qwen3.5 supports images, but current GGUF files in Ollama may not include the mmproj component. Use llama.cpp directly for guaranteed vision support.

**`spawn_agent`** — sub-agents run sequentially by default. Use `spawn_agents` for parallel execution.

---

## Tips and Recommendations

### Choose the right role

```bash
do_it run --task "What does this project do?"             --role navigator
do_it run --task "Find examples of using axum extractors" --role research
do_it run --task "Add validation to src/handlers.rs"      --role developer
do_it run --task "Do all tests pass?"                     --role qa
do_it run --task "Plan the auth module refactor"          --role boss
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

### Using GitHub API

Set `GITHUB_TOKEN` in your environment (classic token with `repo` scope is sufficient):

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
do_it run --task "Find all open bugs and fix the highest priority one" --role developer
```

The agent can list issues, read them, post comments, create PRs, and read file contents directly from GitHub — useful when working across repositories.

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

**`LLM response has no JSON`** — the model responded outside of JSON format. Try a larger model, set `temperature = 0.0`, or use a role to reduce the tool count.

**`str_replace: old_str found N times`** — provide more context to make `old_str` unique:
```json
{ "old_str": "fn process(x: i32) -> i32 {\n    x + 1", "new_str": "..." }
```

**`ask_human via Telegram: no reply received within 5 minutes`** — verify the bot token and chat_id, and that you have sent `/start` to the bot. The agent continues with an error and falls back to console.

**`fetch_url: HTTP 403` or empty result** — the site blocks bots or requires JavaScript. Use direct API endpoints: `raw.githubusercontent.com` instead of `github.com`.

**`github_api: no token found`** — set `GITHUB_TOKEN` environment variable or pass `"token"` in args.

**`test_coverage: cargo tarpaulin not found`** — install with `cargo install cargo-tarpaulin`. The tool falls back to `cargo test` without coverage numbers.

**Agent is looping** — the built-in loop detector will notify you via Telegram. You can also reduce `history_window` to 4, restart with a more specific task, or use a larger model.

**Sub-agent stuck** — if `spawn_agent` takes too long, the sub-agent is limited by `max_steps`. Pass a smaller `max_steps` explicitly, or break the task into smaller pieces.

---

## Project Structure

```
do_it/
├── Cargo.toml           name="do_it", edition="2024", version="0.3.0"
├── config.toml          runtime configuration (models, Telegram, [browser])
├── README.md            project overview
├── DOCS.md              this file
├── LICENSE              MIT
├── .gitignore
├── .ai/                 agent memory (created automatically, gitignored)
│   ├── project.toml     project config (auto-scaffolded)
│   ├── prompts/         custom role prompts
│   ├── state/           plan, last_session, session_counter, external_messages
│   ├── logs/            history.md
│   ├── screenshots/     browser tool output (PNG files)
│   └── knowledge/       lessons_learned, decisions, qa_report, review_report, sub-agent results
└── src/
    ├── main.rs          CLI: run | config | roles; --role flag; image detection
    ├── agent.rs         main loop, session_init (user_profile + boss_notes injection), loop detection
    ├── config.rs        AgentConfig, BrowserConfig, Role enum + allowlists, ModelRouter, global helpers
    ├── history.rs       sliding window context manager
    ├── shell.rs         Ollama HTTP client: chat, chat_with_image, check_models
    ├── tools.rs         all tools including browser stubs and self-improvement tools
    └── prompts/         built-in role prompts compiled into the binary via include_str!
        ├── default.md
        ├── boss.md      orchestrator with browser eyes and wishlist rules
        ├── developer.md
        ├── navigator.md
        ├── qa.md
        ├── reviewer.md
        ├── research.md
        └── memory.md
```

Global config at `~/.do_it/` (created on first run):

```
~/.do_it/
├── config.toml          global defaults (overridden by local config.toml)
├── system_prompt.md     global default prompt override
├── user_profile.md      your preferences — Boss reads on every session start
├── boss_notes.md        cross-project insights accumulated by Boss
└── tool_wishlist.md     agent-requested capabilities — review to prioritise dev work
```

### Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | async runtime |
| `reqwest` | HTTP — Ollama, fetch_url, web_search, Telegram API, GitHub API |
| `serde` / `serde_json` | JSON serialization |
| `toml` | config.toml / project.toml parsing |
| `clap` | CLI argument parsing |
| `walkdir` | recursive filesystem traversal |
| `regex` | search_in_files, find_references, AST parsers |
| `base64` | image encoding for vision; GitHub file contents decoding |
| `anyhow` | error handling |
| `tracing` / `tracing-subscriber` | structured logging |
