use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
    pub file: String,
    pub container: Option<String>,
    pub signature: Option<String>,
}

pub fn get_symbols(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = str_arg(args, "path")?;
    let full_path = super::core::resolve(root, &path)?;
    let source = std::fs::read_to_string(&full_path).map_err(|e| anyhow::anyhow!("get_symbols: {e}"))?;
    let lang = args.get("lang").and_then(|v| v.as_str());
    let detected = lang.unwrap_or_else(|| detect_lang(&full_path));
    let symbols = match detected { "rust" => parse_rust(&source), "python" => parse_python(&source), "ts" | "js" => parse_ts_js(&source), _ => vec![] };
    let mut out = format!("Symbols in {} ({}):\n", full_path.file_name().unwrap_or_default().to_string_lossy(), detected);
    for s in &symbols { out.push_str(&format!("  {} {} @{}", s.kind, s.name, s.line)); if let Some(c) = &s.container { out.push_str(&format!(" (in {})", c)); } out.push('\n'); }
    if symbols.is_empty() { out.push_str("  (none)\n"); }
    Ok(ToolResult::ok(out))
}

pub fn outline(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = str_arg(args, "path")?;
    let full_path = super::core::resolve(root, &path)?;
    let source = std::fs::read_to_string(&full_path).map_err(|e| anyhow::anyhow!("outline: {e}"))?;
    let lang = args.get("lang").and_then(|v| v.as_str());
    let detected = lang.unwrap_or_else(|| detect_lang(&full_path));
    let symbols = match detected { "rust" => parse_rust(&source), "python" => parse_python(&source), "ts" | "js" => parse_ts_js(&source), _ => vec![] };
    let mut out = format!("Outline of {}:\n", full_path.display());
    let mut by_cont: std::collections::BTreeMap<Option<String>, Vec<&Symbol>> = std::collections::BTreeMap::new();
    for s in &symbols { by_cont.entry(s.container.clone()).or_default().push(s); }
    for (cont, syms) in by_cont { if let Some(c) = cont { out.push_str(&format!("{} {{\n", c)); for s in syms { out.push_str(&format!("  {} {}\n", s.kind, s.name)); } out.push_str("}\n"); } else { for s in syms { out.push_str(&format!("{} {}\n", s.kind, s.name)); } } }
    Ok(ToolResult::ok(out))
}

pub fn get_signature(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = str_arg(args, "path")?;
    let sym = str_arg(args, "symbol")?;
    let full_path = super::core::resolve(root, &path)?;
    let source = std::fs::read_to_string(&full_path).map_err(|e| anyhow::anyhow!("get_signature: {e}"))?;
    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains(&format!("fn {}", sym)) || line.contains(&format!("struct {}", sym)) || line.contains(&format!("class {}", sym)) || line.contains(&format!("def {}", sym)) {
            let mut sig = line.trim().to_string();
            let mut bc = 0; let mut pc = 0;
            for ch in line.chars() { if ch == '{' { bc += 1; } else if ch == '}' { bc -= 1; } else if ch == '(' { pc += 1; } else if ch == ')' { pc -= 1; } }
            let mut ci = i;
            while (bc > 0 || pc > 0) && ci + 1 < lines.len() { ci += 1; sig.push(' '); sig.push_str(lines[ci].trim()); for ch in lines[ci].chars() { if ch == '{' { bc += 1; } else if ch == '}' { bc -= 1; } else if ch == '(' { pc += 1; } else if ch == ')' { pc -= 1; } } }
            return Ok(ToolResult::ok(format!("Signature of '{}' @{}:\n{}", sym, i + 1, sig)));
        }
    }
    Ok(ToolResult { output: format!("Symbol '{}' not found", sym), success: false })
}

pub fn find_references(args: &Value, root: &Path) -> Result<ToolResult> {
    let sym = str_arg(args, "symbol")?;
    let search_root = if let Some(p) = args.get("root").and_then(|v| v.as_str()) { super::core::resolve(root, p)? } else { root.to_path_buf() };
    let ext = args.get("ext").and_then(|v| v.as_str());
    let mut refs = Vec::new();
    for entry in walkdir::WalkDir::new(&search_root).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
        let p = entry.path();
        if let Some(e) = ext { if !p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default().ends_with(e) { continue; } }
        if let Some(e) = p.extension().and_then(|e| e.to_str()) { if !matches!(e, "rs"|"py"|"ts"|"tsx"|"js"|"jsx"|"go"|"rb"|"java"|"c"|"cpp"|"h"|"hpp") { continue; } } else { continue; }
        if let Ok(content) = std::fs::read_to_string(p) {
            for (ln, line) in content.lines().enumerate() {
                if line.contains(&sym) { refs.push((p.strip_prefix(&search_root).unwrap_or(p).to_string_lossy().to_string(), ln + 1, line.trim().to_string())); }
            }
        }
    }
    if refs.is_empty() { Ok(ToolResult { output: format!("No refs for '{}'", sym), success: false }) }
    else { let mut out = format!("Refs to '{}' ({}):\n", sym, refs.len()); for (f, l, s) in refs { out.push_str(&format!("{}:{} {}\n", f, l, s)); } Ok(ToolResult::ok(out)) }
}

fn detect_lang(p: &Path) -> &str { match p.extension().and_then(|e| e.to_str()) { Some("rs") => "rust", Some("py") => "python", Some("ts") | Some("tsx") => "ts", Some("js") | Some("jsx") => "js", _ => "unknown" } }

fn parse_rust(src: &str) -> Vec<Symbol> {
    let mut syms = Vec::new(); let mut cont: Option<String> = None; let mut bd = 0;
    for (ln, line) in src.lines().enumerate() {
        let t = line.trim();
        for ch in line.chars() { if ch == '{' { bd += 1; } else if ch == '}' { bd -= 1; } }
        if bd == 0 { cont = None; }
        if t.is_empty() || t.starts_with("//") { continue; }
        if let Some(c) = t.strip_prefix("pub fn ").or_else(|| t.strip_prefix("fn ")) {
            if let Some(ne) = c.find('(') { let n = c[..ne].trim(); syms.push(Symbol { name: n.to_string(), kind: "fn".into(), line: ln + 1, file: String::new(), container: cont.clone(), signature: Some(t.to_string()) }); }
        }
        if let Some(c) = t.strip_prefix("pub struct ").or_else(|| t.strip_prefix("struct ")) {
            if let Some(ne) = c.find(|c: char| c.is_whitespace() || c == '{') { let n = c[..ne].trim(); cont = Some(n.to_string()); syms.push(Symbol { name: n.to_string(), kind: "struct".into(), line: ln + 1, file: String::new(), container: None, signature: Some(t.to_string()) }); }
        }
        if let Some(c) = t.strip_prefix("pub enum ").or_else(|| t.strip_prefix("enum ")) {
            if let Some(ne) = c.find(|c: char| c.is_whitespace() || c == '{') { let n = c[..ne].trim(); syms.push(Symbol { name: n.to_string(), kind: "enum".into(), line: ln + 1, file: String::new(), container: None, signature: Some(t.to_string()) }); }
        }
        if let Some(c) = t.strip_prefix("pub trait ").or_else(|| t.strip_prefix("trait ")) {
            if let Some(ne) = c.find(|c: char| c.is_whitespace() || c == '{') { let n = c[..ne].trim(); cont = Some(n.to_string()); syms.push(Symbol { name: n.to_string(), kind: "trait".into(), line: ln + 1, file: String::new(), container: None, signature: Some(t.to_string()) }); }
        }
        if let Some(c) = t.strip_prefix("impl ") {
            if let Some(ne) = c.find(|c: char| c.is_whitespace() || c == '{' || c == '<') { let n = c[..ne].trim(); cont = Some(n.to_string()); syms.push(Symbol { name: n.to_string(), kind: "impl".into(), line: ln + 1, file: String::new(), container: None, signature: Some(t.to_string()) }); }
        }
    }
    syms
}

fn parse_python(src: &str) -> Vec<Symbol> {
    let mut syms = Vec::new(); let mut cstack: Vec<String> = Vec::new(); let mut istack: Vec<usize> = Vec::new();
    for (ln, line) in src.lines().enumerate() {
        if line.trim().is_empty() || line.trim().starts_with('#') { continue; }
        let ind = line.chars().take_while(|c| c.is_whitespace()).count();
        while let Some(li) = istack.last() { if ind <= *li { istack.pop(); cstack.pop(); } else { break; } }
        let t = line.trim();
        if let Some(c) = t.strip_prefix("class ") {
            if let Some(ne) = c.find(|c: char| c == ':' || c.is_whitespace()) { let n = c[..ne].trim(); istack.push(ind); cstack.push(n.to_string()); syms.push(Symbol { name: n.to_string(), kind: "class".into(), line: ln + 1, file: String::new(), container: None, signature: Some(t.to_string()) }); }
        }
        if let Some(c) = t.strip_prefix("def ").or_else(|| t.strip_prefix("async def ")) {
            if let Some(ne) = c.find('(') { let n = c[..ne].trim(); let cont = cstack.last().cloned(); let k = if cont.is_some() { "method" } else { "fn" }; syms.push(Symbol { name: n.to_string(), kind: k.into(), line: ln + 1, file: String::new(), container: cont, signature: Some(t.to_string()) }); }
        }
    }
    syms
}

fn parse_ts_js(src: &str) -> Vec<Symbol> {
    let mut syms = Vec::new(); let mut bd = 0; let mut cont: Option<String> = None;
    for (ln, line) in src.lines().enumerate() {
        let t = line.trim();
        for ch in line.chars() { if ch == '{' { bd += 1; } else if ch == '}' { bd -= 1; } }
        if bd == 0 { cont = None; }
        if t.is_empty() || t.starts_with("//") { continue; }
        if let Some(c) = t.strip_prefix("export class ").or_else(|| t.strip_prefix("class ")) {
            if let Some(ne) = c.find(|c: char| c.is_whitespace() || c == '{') { let n = c[..ne].trim(); cont = Some(n.to_string()); syms.push(Symbol { name: n.to_string(), kind: "class".into(), line: ln + 1, file: String::new(), container: None, signature: Some(t.to_string()) }); }
        }
        if let Some(c) = t.strip_prefix("export function ").or_else(|| t.strip_prefix("function ")) {
            if let Some(ne) = c.find('(') { let n = c[..ne].trim(); syms.push(Symbol { name: n.to_string(), kind: "fn".into(), line: ln + 1, file: String::new(), container: cont.clone(), signature: Some(t.to_string()) }); }
        }
        if t.contains("const ") && t.contains("=>") {
            if let Some(start) = t.find("const ") {
                let rest = &t[start + 6..];
                if let Some(eq) = rest.find('=') {
                    let n = rest[..eq].trim();
                    syms.push(Symbol { name: n.to_string(), kind: "arrow".into(), line: ln + 1, file: String::new(), container: cont.clone(), signature: Some(t.to_string()) });
                }
            }
        }
    }
    syms
}
