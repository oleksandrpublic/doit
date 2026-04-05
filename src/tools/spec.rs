use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Real,
    Stub,
    Experimental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDispatchKind {
    Runtime,
    AgentLoop,
}

/// Optional capability group — tools in these groups are excluded from
/// role allowlists unless the group is enabled in AgentConfig::tool_groups.
/// None means the tool is always available to its allowed_roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolGroup {
    Browser,
    Background,
    Github,
}

impl ToolGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Background => "background",
            Self::Github => "github",
        }
    }

    pub fn tool_group_from_str(s: &str) -> Option<Self> {
        match s {
            "browser" => Some(Self::Browser),
            "background" => Some(Self::Background),
            "github" => Some(Self::Github),
            _ => None,
        }
    }
}

pub struct ToolSpec {
    pub canonical_name: &'static str,
    pub aliases: &'static [&'static str],
    pub status: ToolStatus,
    pub dispatch: ToolDispatchKind,
    /// Roles that may use this tool when its group (if any) is enabled.
    pub allowed_roles: &'static [&'static str],
    /// If Some, the tool is only available when this group is in AgentConfig::tool_groups.
    pub group: Option<ToolGroup>,
    pub prompt_category: &'static str,
    pub prompt_line: &'static str,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tool registry
//
// Core tool budget per role (optional groups excluded, target ≤ 12):
//
//   boss:      11  — orchestration, memory, web_search, tree/project_map
//   developer: 18  — write/run/git/memory/notify/script/targeted-test/format/preview/multi/fuzzy helpers  (no fs-search — use navigator)
//   navigator: 11  — read/search/code-intelligence
//   qa:        15  — read-subset/run/diff/git-read/memory/coverage/script/targeted-test
//   reviewer:   9  — read-subset/code-intelligence/diff
//   research:   6  — web/memory (unchanged)
//   memory:     3  — memory only (unchanged)
//
// Optional groups (enabled via config.toml tool_groups = ["browser", ...]):
//   browser    → boss(+4), developer(+2), qa(+2), reviewer(+2)
//   background → boss(+4), developer(+4)
//   github     → developer(+1), qa(+1)
// ─────────────────────────────────────────────────────────────────────────────

const TOOL_SPECS: &[ToolSpec] = &[
    // ── Filesystem read ──────────────────────────────────────────────────────
    // navigator: full read access
    // qa/reviewer: read files they're inspecting
    // developer: read_file only — for targeted reads after navigator maps the codebase
    ToolSpec { canonical_name: "read_file",         aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer", "navigator", "qa", "reviewer"],            group: None,                        prompt_category: "Filesystem",        prompt_line: "- read_file(path, start_line?, end_line?)            — View a file with line numbers" },
    ToolSpec { canonical_name: "open_file_region",  aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator"],                         group: None,                        prompt_category: "Filesystem",        prompt_line: "- open_file_region(path, line, before?, after?)      — Focused region around a line" },
    ToolSpec { canonical_name: "list_dir",          aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator"],                                           group: None,                        prompt_category: "Filesystem",        prompt_line: "- list_dir(path?)                                    — List directory contents" },
    ToolSpec { canonical_name: "find_files",        aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator"],                                           group: None,                        prompt_category: "Filesystem",        prompt_line: "- find_files(pattern, dir?)                          — Find files by name or glob" },
    ToolSpec { canonical_name: "search_in_files",   aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator", "qa", "reviewer"],                         group: None,                        prompt_category: "Filesystem",        prompt_line: "- search_in_files(pattern, dir?, ext?)               — Regex search across file contents" },
    ToolSpec { canonical_name: "tree",              aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "navigator"],                                   group: None,                        prompt_category: "Filesystem",        prompt_line: "- tree(dir?, depth?, ignore?)                        — Recursive directory tree" },

    // ── Filesystem write ─────────────────────────────────────────────────────
    ToolSpec { canonical_name: "write_file",        aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Filesystem",        prompt_line: "- write_file(path, content)                          — Overwrite a file completely" },
    ToolSpec { canonical_name: "str_replace",       aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Filesystem",        prompt_line: "- str_replace(path, old_str, new_str)                — Replace a unique string in a file" },
    ToolSpec { canonical_name: "apply_patch_preview", aliases: &[],                   status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Filesystem",        prompt_line: "- apply_patch_preview(path, content? | old_str, new_str) — Preview an edit as a diff [experimental]" },
    ToolSpec { canonical_name: "str_replace_multi",    aliases: &[],                   status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Filesystem",        prompt_line: "- str_replace_multi(path, edits[])                   — Apply multiple replacements in one call" },
    ToolSpec { canonical_name: "str_replace_fuzzy",    aliases: &[],                   status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Filesystem",        prompt_line: "- str_replace_fuzzy(path, old_str, new_str)           — Replace with whitespace-tolerant matching [experimental]" },

    // ── Execution ────────────────────────────────────────────────────────────
    ToolSpec { canonical_name: "run_command",       aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer", "qa"],                                     group: None,                        prompt_category: "Execution",         prompt_line: "- run_command(program, args[], cwd?, timeout_secs?)   — Run an executable (no shell)" },
    ToolSpec { canonical_name: "format_changed_files_only", aliases: &[],             status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Execution",         prompt_line: "- format_changed_files_only(dir?, check_only?)        — Format changed Rust files only [experimental]" },
    ToolSpec { canonical_name: "diff_repo",         aliases: &["workspace_diff"],     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["qa", "reviewer"],                         group: None,                        prompt_category: "Execution",         prompt_line: "- diff_repo(base?, staged?, stat?)                   — Git diff vs HEAD or a ref" },
    ToolSpec { canonical_name: "read_test_failure", aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["qa"],                                                  group: None,                        prompt_category: "Execution",         prompt_line: "- read_test_failure(path?, test?, index?)            — Extract a failing test block from a log" },
    ToolSpec { canonical_name: "test_coverage",     aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["qa"],                                              group: None,                        prompt_category: "Execution",         prompt_line: "- test_coverage(dir?, threshold?, timeout_secs?)     — Run tests with coverage" },
    ToolSpec { canonical_name: "run_targeted_test", aliases: &[],                     status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer", "qa"],                                     group: None,                        prompt_category: "Execution",         prompt_line: "- run_targeted_test(path?, test?, kind?, target?)     — Run a narrow Rust test target [experimental]" },
    ToolSpec { canonical_name: "run_script",        aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer", "qa", "navigator"],                        group: None,                        prompt_category: "Execution",         prompt_line: "- run_script(script, dir?)                            — Compute/transform/validate data in a sandboxed Rhai script (read_lines, read_text, regex_match, regex_find_all, parse_json, sha256, log). Use instead of run_command for pure data work." },

    // ── Git ──────────────────────────────────────────────────────────────────
    ToolSpec { canonical_name: "git_status",        aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer", "qa"],                                     group: None,                        prompt_category: "Git",               prompt_line: "- git_status(short?)                                 — Working tree status and branch info" },
    ToolSpec { canonical_name: "git_commit",        aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Git",               prompt_line: "- git_commit(message, files?, allow_empty?)          — Stage files and commit" },
    ToolSpec { canonical_name: "git_log",           aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["qa", "reviewer"],                         group: None,                        prompt_category: "Git",               prompt_line: "- git_log(n?, path?, oneline?)                       — Recent commit history" },
    ToolSpec { canonical_name: "git_pull",          aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer", "qa"],                                     group: None,                        prompt_category: "Git",               prompt_line: "- git_pull(remote?, branch?)                         — Fetch remote changes safely" },
    ToolSpec { canonical_name: "git_push",          aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer"],                                           group: None,                        prompt_category: "Git",               prompt_line: "- git_push(remote?, branch?, force?)                 — Push to remote (requires consent)" },
    ToolSpec { canonical_name: "git_stash",         aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &[],                                           group: None,                        prompt_category: "Git",               prompt_line: "- git_stash(action, message?, index?)                — Manage git stashes" },

    // ── Internet ─────────────────────────────────────────────────────────────
    ToolSpec { canonical_name: "web_search",        aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "research"],                                    group: None,                        prompt_category: "Internet",          prompt_line: "- web_search(query, max_results?)                    — Search the web" },
    ToolSpec { canonical_name: "fetch_url",         aliases: &["web_fetch"],          status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["research"],                                            group: None,                        prompt_category: "Internet",          prompt_line: "- fetch_url(url, selector?)                          — Read a web page or documentation" },
    ToolSpec { canonical_name: "github_api",        aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["developer", "qa"],                                     group: Some(ToolGroup::Github),     prompt_category: "Internet",          prompt_line: "- github_api(method, endpoint, body?, token?)        — GitHub REST API" },

    // ── Code Intelligence ────────────────────────────────────────────────────
    // navigator and reviewer only — developer uses read_file + spawn_agent(navigator)
    ToolSpec { canonical_name: "get_symbols",       aliases: &["analyze_code"],       status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator", "reviewer"],                               group: None,                        prompt_category: "Code Intelligence", prompt_line: "- get_symbols(path, kinds?)                          — List symbols in a file" },
    ToolSpec { canonical_name: "outline",           aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator", "reviewer"],                               group: None,                        prompt_category: "Code Intelligence", prompt_line: "- outline(path)                                      — Structural outline with signatures" },
    ToolSpec { canonical_name: "get_signature",     aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["reviewer"],                               group: None,                        prompt_category: "Code Intelligence", prompt_line: "- get_signature(path, name, lines?)                   — Symbol signature and docs" },
    ToolSpec { canonical_name: "find_references",   aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator", "reviewer"],                               group: None,                        prompt_category: "Code Intelligence", prompt_line: "- find_references(name, dir?, ext?)                   — All usages of a symbol" },
    ToolSpec { canonical_name: "project_map",       aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "navigator"],                 group: None,                        prompt_category: "Code Intelligence", prompt_line: "- project_map(dir?, depth?)                          — Project layout summary" },
    ToolSpec { canonical_name: "find_entrypoints",  aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &[],                                           group: None,                        prompt_category: "Code Intelligence", prompt_line: "- find_entrypoints(dir?, depth?, limit?)             — Find app/CLI/web/test entrypoints" },
    ToolSpec { canonical_name: "trace_call_path",   aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["navigator"],                               group: None,                        prompt_category: "Code Intelligence", prompt_line: "- trace_call_path(symbol, dir?, depth?)               — Caller chain for a symbol" },

    // ── Communication ────────────────────────────────────────────────────────
    ToolSpec { canonical_name: "ask_human",         aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "research", "reviewer"],                  group: None,                        prompt_category: "Communication",     prompt_line: "- ask_human(question)                                — Ask for clarification or report a blocker" },
    ToolSpec { canonical_name: "notify",            aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer", "qa"],                             group: None,                        prompt_category: "Communication",     prompt_line: "- notify(message, silent?)                           — Send a one-way progress notification" },

    // ── Memory ───────────────────────────────────────────────────────────────
    ToolSpec { canonical_name: "memory_read",       aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "research", "developer", "navigator", "qa", "reviewer", "memory"], group: None,                   prompt_category: "Memory",            prompt_line: "- memory_read(key)                                   — Read a memory entry" },
    ToolSpec { canonical_name: "memory_write",      aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "research", "developer", "navigator", "qa", "reviewer", "memory"], group: None,                   prompt_category: "Memory",            prompt_line: "- memory_write(key, content, append?)                — Write or append a memory entry" },
    ToolSpec { canonical_name: "memory_delete",     aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "memory"],                                      group: None,                   prompt_category: "Memory",            prompt_line: "- memory_delete(key)                                 — Delete a memory entry" },

    // ── Orchestration ────────────────────────────────────────────────────────
    ToolSpec { canonical_name: "spawn_agent",       aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss"],                                                group: None,                        prompt_category: "Orchestration",     prompt_line: "- spawn_agent(role, task, memory_key?, max_steps?)   — Delegate a subtask to a sub-agent" },
    ToolSpec { canonical_name: "spawn_agents",      aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss"],                                                group: None,                        prompt_category: "Orchestration",     prompt_line: "- spawn_agents(agents[], timeout_secs?)              — Delegate parallel subtasks" },

    // ── Self-Improvement ─────────────────────────────────────────────────────
    ToolSpec { canonical_name: "tool_request",      aliases: &["self_improve"],       status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss"],                                                group: None,                        prompt_category: "Self-Improvement",  prompt_line: "- tool_request(name, description, motivation, priority?) — Record a missing capability" },
    ToolSpec { canonical_name: "capability_gap",    aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss"],                                                group: None,                        prompt_category: "Self-Improvement",  prompt_line: "- capability_gap(context, impact)                    — Report a structural blind spot" },

    // ── Browser (optional group: tool_groups = ["browser"]) ──────────────────
    ToolSpec { canonical_name: "browser_action",    aliases: &["browser_automation"], status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer"],                                   group: Some(ToolGroup::Browser),    prompt_category: "Browser",           prompt_line: "- browser_action(action, selector, value?, wait_ms?) — Interact with a page element [experimental]" },
    ToolSpec { canonical_name: "browser_get_text",  aliases: &[],                     status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer", "qa", "reviewer"],                 group: Some(ToolGroup::Browser),    prompt_category: "Browser",           prompt_line: "- browser_get_text(url, selector?, wait_ms?)         — Read rendered page content [experimental]" },
    ToolSpec { canonical_name: "browser_navigate",  aliases: &[],                     status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer"],                                   group: Some(ToolGroup::Browser),    prompt_category: "Browser",           prompt_line: "- browser_navigate(url, wait_ms?)                    — Navigate and wait for load [experimental]" },
    ToolSpec { canonical_name: "screenshot",        aliases: &[],                     status: ToolStatus::Experimental, dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer", "qa", "reviewer"],                 group: Some(ToolGroup::Browser),    prompt_category: "Browser",           prompt_line: "- screenshot(url, wait_ms?, full_page?)              — Take a screenshot [experimental]" },

    // ── Background Processes (optional group: tool_groups = ["background"]) ──
    ToolSpec { canonical_name: "run_background",    aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer"],                                   group: Some(ToolGroup::Background), prompt_category: "Background",        prompt_line: "- run_background(id, program, args?, cwd?)           — Start a background process" },
    ToolSpec { canonical_name: "process_status",    aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer"],                                   group: Some(ToolGroup::Background), prompt_category: "Background",        prompt_line: "- process_status(id, pid?)                            — Check a background process" },
    ToolSpec { canonical_name: "process_list",      aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer"],                                   group: Some(ToolGroup::Background), prompt_category: "Background",        prompt_line: "- process_list()                                     — List background processes" },
    ToolSpec { canonical_name: "process_kill",      aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::Runtime,   allowed_roles: &["boss", "developer"],                                   group: Some(ToolGroup::Background), prompt_category: "Background",        prompt_line: "- process_kill(id, pid?)                              — Stop a background process" },

    // ── Completion ───────────────────────────────────────────────────────────
    ToolSpec { canonical_name: "finish",            aliases: &[],                     status: ToolStatus::Real,         dispatch: ToolDispatchKind::AgentLoop, allowed_roles: &["boss", "research", "developer", "navigator", "qa", "reviewer", "memory"], group: None, prompt_category: "Completion", prompt_line: "- finish(summary, success)                           — Signal task completion" },
];

pub fn all_tool_specs() -> &'static [ToolSpec] {
    TOOL_SPECS
}

pub fn find_tool_spec(name: &str) -> Option<&'static ToolSpec> {
    let normalized = name.trim();
    TOOL_SPECS
        .iter()
        .find(|spec| spec.canonical_name == normalized || spec.aliases.contains(&normalized))
}

pub fn canonical_tool_name(name: &str) -> Option<&'static str> {
    find_tool_spec(name).map(|spec| spec.canonical_name)
}

pub fn tool_status(name: &str) -> Option<ToolStatus> {
    find_tool_spec(name).map(|spec| spec.status)
}

/// Returns the allowlist for a role given enabled optional groups.
/// Core tools are always included; group tools only when the group is enabled.
pub fn allowed_tools_for_role_with_groups(
    role_name: &str,
    enabled_groups: &[String],
) -> Vec<&'static str> {
    TOOL_SPECS
        .iter()
        .filter(|spec| {
            if !spec.allowed_roles.contains(&role_name) {
                return false;
            }
            match spec.group {
                None => true, // core tool — always included
                Some(group) => enabled_groups.iter().any(|g| g == group.as_str()),
            }
        })
        .map(|spec| spec.canonical_name)
        .collect()
}

/// Backwards-compatible wrapper — returns core tools only (no optional groups).
pub fn allowed_tools_for_role(role_name: &str) -> Vec<&'static str> {
    allowed_tools_for_role_with_groups(role_name, &[])
}

pub fn extract_tool_names_from_prompt(prompt: &str) -> Vec<String> {
    prompt
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("- ")?;
            let open_paren = rest.find('(')?;
            let candidate = &rest[..open_paren];
            if candidate.is_empty()
                || !candidate
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
            {
                return None;
            }
            Some(candidate.to_string())
        })
        .collect()
}

fn prompt_status_tag(status: ToolStatus) -> Option<&'static str> {
    match status {
        ToolStatus::Real => None,
        ToolStatus::Stub => Some(" [limited]"),
        ToolStatus::Experimental => Some(" [experimental]"),
    }
}

/// Render the tool catalog for a role, filtered by enabled optional groups.
pub fn render_tool_catalog_for_role_with_groups(
    role_name: Option<&str>,
    enabled_groups: &[String],
) -> String {
    let specs: Vec<&ToolSpec> = TOOL_SPECS
        .iter()
        .filter(|spec| {
            let role_ok = match role_name {
                Some(role) => spec.allowed_roles.contains(&role),
                None => true,
            };
            if !role_ok {
                return false;
            }
            match spec.group {
                None => true,
                Some(group) => enabled_groups.iter().any(|g| g == group.as_str()),
            }
        })
        .collect();

    let mut out = String::from("## Available tools\n\n");
    let mut current_category = "";
    let mut has_limited = false;
    let mut has_experimental = false;

    for spec in &specs {
        if spec.prompt_category != current_category {
            if !current_category.is_empty() {
                out.push('\n');
            }
            current_category = spec.prompt_category;
            out.push_str(&format!("### {}\n", current_category));
        }
        out.push_str(spec.prompt_line);
        if let Some(tag) = prompt_status_tag(spec.status) {
            // tag already in prompt_line for optional-group tools; skip double-tag
            if !spec.prompt_line.contains(tag.trim()) {
                out.push_str(tag);
            }
        }
        out.push('\n');
        match spec.status {
            ToolStatus::Stub => has_limited = true,
            ToolStatus::Experimental => has_experimental = true,
            ToolStatus::Real => {}
        }
    }

    if has_limited || has_experimental {
        out.push_str("\n### Capability notes\n");
        if has_limited {
            out.push_str(
                "- `[limited]` — stub or partial implementation. Prefer fallback strategies.\n",
            );
        }
        if has_experimental {
            out.push_str("- `[experimental]` — may have environment or contract limitations. Verify results.\n");
        }
    }

    out.trim_end().to_string()
}

/// Backwards-compatible wrapper — core tools only, no optional groups.
pub fn render_tool_catalog_for_role(role_name: Option<&str>) -> String {
    render_tool_catalog_for_role_with_groups(role_name, &[])
}

pub fn inject_tool_catalog(prompt: &str, role_name: Option<&str>) -> String {
    inject_tool_catalog_with_groups(prompt, role_name, &[])
}

pub fn inject_tool_catalog_with_groups(
    prompt: &str,
    role_name: Option<&str>,
    enabled_groups: &[String],
) -> String {
    let start = match prompt.find("## Available tools") {
        Some(idx) => idx,
        None => return prompt.to_string(),
    };
    let after_start = &prompt[start + "## Available tools".len()..];
    let next_heading_rel = after_start
        .find("\n## ")
        .map(|idx| idx + "## Available tools".len() + 1);
    let end = next_heading_rel
        .map(|idx| start + idx)
        .unwrap_or(prompt.len());

    let mut out = String::new();
    out.push_str(&prompt[..start]);
    out.push_str(&render_tool_catalog_for_role_with_groups(
        role_name,
        enabled_groups,
    ));
    if end < prompt.len() {
        out.push('\n');
        out.push_str(&prompt[end..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_to_canonical_names() {
        assert_eq!(canonical_tool_name("web_fetch"), Some("fetch_url"));
        assert_eq!(canonical_tool_name("workspace_diff"), Some("diff_repo"));
        assert_eq!(canonical_tool_name("analyze_code"), Some("get_symbols"));
        assert_eq!(canonical_tool_name("self_improve"), Some("tool_request"));
        assert_eq!(
            canonical_tool_name("browser_automation"),
            Some("browser_action")
        );
    }

    #[test]
    fn finish_is_agent_loop_tool() {
        let spec = find_tool_spec("finish").unwrap();
        assert_eq!(spec.dispatch, ToolDispatchKind::AgentLoop);
        assert_eq!(spec.status, ToolStatus::Real);
    }

    #[test]
    fn canonical_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in all_tool_specs() {
            assert!(
                seen.insert(spec.canonical_name),
                "duplicate: {}",
                spec.canonical_name
            );
        }
    }

    #[test]
    fn core_tool_counts_within_budget() {
        // No optional groups enabled — verify core counts
        let budgets = [
            ("boss", 13),
            ("developer", 18),
            ("navigator", 15), // navigator is read-only so 15 is acceptable
            ("qa", 15),
            ("reviewer", 12),
            ("research", 8),
            ("memory", 4),
        ];
        for (role, max) in budgets {
            let count = allowed_tools_for_role(role).len();
            assert!(
                count <= max,
                "role '{}' has {} core tools, budget is {}",
                role,
                count,
                max
            );
        }
    }

    #[test]
    fn optional_groups_add_tools_correctly() {
        let groups_none: Vec<String> = vec![];
        let groups_browser = vec!["browser".to_string()];
        let groups_all = vec![
            "browser".to_string(),
            "background".to_string(),
            "github".to_string(),
        ];

        let dev_core = allowed_tools_for_role_with_groups("developer", &groups_none).len();
        let dev_browser = allowed_tools_for_role_with_groups("developer", &groups_browser).len();
        let dev_all = allowed_tools_for_role_with_groups("developer", &groups_all).len();

        assert!(
            dev_browser > dev_core,
            "browser group should add tools to developer"
        );
        assert!(dev_all > dev_browser, "all groups should add more tools");

        // browser group must not appear in navigator
        let nav_browser = allowed_tools_for_role_with_groups("navigator", &groups_browser);
        assert!(
            !nav_browser.contains(&"screenshot"),
            "navigator should not get browser tools"
        );
    }

    #[test]
    fn developer_has_no_code_intelligence_tools() {
        let dev_tools = allowed_tools_for_role("developer");
        let ci_tools = [
            "get_symbols",
            "outline",
            "get_signature",
            "find_references",
            "trace_call_path",
        ];
        for tool in ci_tools {
            assert!(
                !dev_tools.contains(&tool),
                "developer should not have code intelligence tool '{}' — use navigator sub-agent",
                tool
            );
        }
    }

    #[test]
    fn developer_has_no_fs_search_tools() {
        let dev_tools = allowed_tools_for_role("developer");
        let search_tools = ["search_in_files", "find_files", "list_dir"];
        for tool in search_tools {
            assert!(
                !dev_tools.contains(&tool),
                "developer should not have fs-search tool '{}' — use navigator sub-agent",
                tool
            );
        }
    }

    #[test]
    fn boss_has_no_direct_code_tools() {
        let boss_tools = allowed_tools_for_role("boss");
        let forbidden = [
            "write_file",
            "str_replace",
            "run_command",
            "read_file",
            "get_symbols",
            "outline",
            "git_commit",
        ];
        for tool in forbidden {
            assert!(
                !boss_tools.contains(&tool),
                "boss should not have direct code tool '{}' — delegate to sub-agents",
                tool
            );
        }
    }

    #[test]
    fn renders_role_catalog_without_optional_groups() {
        let rendered = render_tool_catalog_for_role(Some("research"));
        assert!(rendered.contains("web_search(query, max_results?)"));
        assert!(rendered.contains("fetch_url(url, selector?)"));
        assert!(!rendered.contains("git_commit("));
        assert!(!rendered.contains("screenshot("));
    }

    #[test]
    fn renders_optional_groups_when_enabled() {
        let groups = vec!["browser".to_string()];
        let rendered = render_tool_catalog_for_role_with_groups(Some("developer"), &groups);
        assert!(rendered.contains("screenshot("));
        assert!(rendered.contains("browser_navigate("));
        // capability notes appear because browser tools are [experimental]
        assert!(rendered.contains("[experimental]"));
        assert!(rendered.contains("### Capability notes"));
    }

    #[test]
    fn renders_status_tags_and_capability_notes_with_groups() {
        // real background tools should render without capability notes
        let groups = vec!["background".to_string()];
        let rendered = render_tool_catalog_for_role_with_groups(Some("developer"), &groups);
        assert!(rendered.contains("run_background("));
        assert!(!rendered.contains("[limited]"));
    }

    #[test]
    fn injects_catalog_into_prompt() {
        let prompt = "Header\n\n## Available tools\n- old_tool(x)\n\n## Rules\n1. Test";
        let injected = inject_tool_catalog(prompt, Some("memory"));
        assert!(injected.contains("memory_read(key)"));
        assert!(!injected.contains("old_tool(x)"));
        assert!(injected.contains("## Rules"));
    }

    #[test]
    fn extracts_tool_names_from_prompt_bullets_only() {
        let prompt = "
- read_file(path, start_line?, end_line?)
- browser_action(action, selector, value?)
- finish(summary, success)
  1. spawn_agent(role=\"developer\", task=\"x\")
- \"plan\" -> not a tool
";
        let extracted = extract_tool_names_from_prompt(prompt);
        assert_eq!(extracted, vec!["read_file", "browser_action", "finish"]);
    }
}
