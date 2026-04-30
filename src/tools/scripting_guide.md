# run_script — When and How to Use It

`run_script` executes a sandboxed [Rhai](https://rhai.rs) script inside the
agent process.  It is **not** a shell — it cannot launch programs, touch the
network, or escape the repository root.  It is a fast, safe computation layer
that runs in **≤ 30 seconds**.

---

## When to use run_script

Use `run_script` when you need to **compute, transform, or validate data** that
would otherwise require many read → think → write cycles.

| Situation | Without run_script | With run_script |
|---|---|---|
| Count lines matching a pattern across a file | read_file → count mentally → write result | one script, instant |
| Parse a JSON config and extract nested values | read_file → manual extraction | `parse_json` + field access |
| Validate that every item in a list satisfies a rule | multiple reads + mental checks | loop + `log` + return bool |
| Generate a repetitive block (e.g. 20 match arms) | write_file with manual text | script builds the string |
| Check a regex against extracted text | read_file → mental match | `regex_match` |
| Compute a hash for change detection / deduplication | impossible without shell | `fnv64` |
| Summarise a CSV: count rows, check headers | read_file → mental parse | `read_lines` + loop |
| Generate and write a file from template/data | write_file with manual construction | `write_text` + loop (allow_write: true) |

**Do NOT use run_script for:**
- Running build tools, tests, linters → use `run_command`
- Reading files into context for later tools → use `read_file`
- Writing source code → use `write_file` / `str_replace`
- Anything that needs the network or env vars

---

## Available host functions

```rhai
// File I/O (sandboxed to repo root)
let lines = read_lines("src/lib.rs");   // → Array of strings, one per line
let text  = read_text("data.csv");      // → full file as one string

// Directory inspection (sandboxed to repo root)
let entries = list_dir("src");          // → Array of entry names (sorted), e.g. ["agent", "lib.rs"]
let exists  = file_exists("Cargo.toml"); // → bool

// File write (only when allow_write: true is passed to run_script)
write_text("out/report.txt", result);  // → creates parent dirs, returns "written: ..."

// Pattern matching
regex_match("^v\\d+\\.\\d+", version)  // → bool

// Data
let obj = parse_json(text);             // → Map/Array/scalar from JSON string

// Hashing (non-cryptographic — change detection / deduplication only)
let h = fnv64("hello");              // → FNV-1a 64-bit hash as 16-char hex string

// Logging (shown in tool output under "Logs:")
log("checked " + count.to_string() + " entries");
```

---

## Practical patterns

### Count lines matching a regex
```rhai
let lines = read_lines("src/agent/loops/mod.rs");
let count = 0;
for line in lines {
    if regex_match("unwrap\\(\\)", line) { count += 1; }
}
log("unwrap() call count: " + count.to_string());
count
```

### Validate a JSON config field
```rhai
let cfg = parse_json(read_text("config.toml.json"));
let ok = cfg["max_tokens"] >= 1024 && cfg["temperature"] <= 1.0;
if !ok { log("INVALID config values"); }
ok
```

### Build a repetitive code snippet
```rhai
let variants = ["Ollama", "OpenAI", "Anthropic"];
let mut arms = "";
for v in variants {
    arms += "            BackendKind::" + v + " => \"" + v.to_lower() + "\",\n";
}
arms
```

### Summarise a CSV (first-pass, no shell needed)
```rhai
let lines = read_lines("data/metrics.csv");
let header = lines[0];
let rows   = lines.len() - 1;
log("columns: " + header);
log("data rows: " + rows.to_string());
rows
```

### Generate a file from a template (allow_write: true)
```rhai
let variants = ["Ollama", "OpenAI", "Anthropic"];
let mut body = "pub enum Backend {\n";
for v in variants {
    body += "    " + v + ",\n";
}
body += "}\n";
write_text("src/backend_gen.rs", body)
```

---

## Limits (sandbox)

| Limit | Value |
|---|---|
| Wall-clock timeout | 30 seconds |
| Operations | 200 000 |
| Array size | 10 000 items |
| String size | 64 KB |
| Call depth | 32 |
| Variables | 256 |
| No network, no env, no shell | — |

Scripts that exceed any limit fail safely — the agent receives a `failure`
result and should try a different approach.

---

## Return value

The **last expression** in the script is the return value.  It appears in the
tool output as `Result: <value>`.  Use `log(...)` for intermediate output.

```rhai
// BAD  — returns unit, no useful result
let x = 42;

// GOOD — last expression is the answer
let x = 42;
x
```

---

## Decision rule for the agent

> **If you can answer a question or produce a value by computing over data
> you already have (or can read from a file), and no external process is
> needed, reach for `run_script` before reaching for `run_command`.**

`run_script` is instant and leaves no side-effects.  It is always safe to try.
