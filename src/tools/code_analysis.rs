use super::core::{ToolResult, resolve, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use tree_sitter::{Node, Parser};

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
    let full_path = resolve(root, &path)?;
    let source =
        std::fs::read_to_string(&full_path).map_err(|e| anyhow::anyhow!("get_symbols: {e}"))?;
    let lang = args.get("lang").and_then(|v| v.as_str());
    let detected = lang.unwrap_or_else(|| detect_lang(&full_path));
    let symbols = match detected {
        "rust" => parse_rust(&source),
        "python" => parse_python(&source),
        "ts" | "js" => parse_ts_js(&source),
        _ => vec![],
    };
    let mut out = format!(
        "Symbols in {} ({}):\n",
        full_path.file_name().unwrap_or_default().to_string_lossy(),
        detected
    );
    for s in &symbols {
        out.push_str(&format!("  {} {} @{}", s.kind, s.name, s.line));
        if let Some(c) = &s.container {
            out.push_str(&format!(" (in {})", c));
        }
        out.push('\n');
    }
    if symbols.is_empty() {
        out.push_str("  (none)\n");
    }
    Ok(ToolResult::ok(out))
}

pub fn outline(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = str_arg(args, "path")?;
    let full_path = resolve(root, &path)?;
    let source =
        std::fs::read_to_string(&full_path).map_err(|e| anyhow::anyhow!("outline: {e}"))?;
    let lang = args.get("lang").and_then(|v| v.as_str());
    let detected = lang.unwrap_or_else(|| detect_lang(&full_path));
    let symbols = match detected {
        "rust" => parse_rust(&source),
        "python" => parse_python(&source),
        "ts" | "js" => parse_ts_js(&source),
        _ => vec![],
    };
    let mut out = format!("Outline of {}:\n", full_path.display());
    let mut by_cont: std::collections::BTreeMap<Option<String>, Vec<&Symbol>> =
        std::collections::BTreeMap::new();
    for s in &symbols {
        by_cont.entry(s.container.clone()).or_default().push(s);
    }
    for (cont, syms) in by_cont {
        if let Some(c) = cont {
            out.push_str(&format!("{} {{\n", c));
            for s in syms {
                out.push_str(&format!("  {} {}\n", s.kind, s.name));
            }
            out.push_str("}\n");
        } else {
            for s in syms {
                out.push_str(&format!("{} {}\n", s.kind, s.name));
            }
        }
    }
    Ok(ToolResult::ok(out))
}

pub fn get_signature(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = str_arg(args, "path")?;
    let sym = args
        .get("symbol")
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: symbol"))?;
    let full_path = resolve(root, &path)?;
    let source =
        std::fs::read_to_string(&full_path).map_err(|e| anyhow::anyhow!("get_signature: {e}"))?;
    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains(&format!("fn {}", sym))
            || line.contains(&format!("struct {}", sym))
            || line.contains(&format!("class {}", sym))
            || line.contains(&format!("def {}", sym))
        {
            let mut sig = line.trim().to_string();
            let mut bc = 0;
            let mut pc = 0;
            for ch in line.chars() {
                if ch == '{' {
                    bc += 1;
                } else if ch == '}' {
                    bc -= 1;
                } else if ch == '(' {
                    pc += 1;
                } else if ch == ')' {
                    pc -= 1;
                }
            }
            let mut ci = i;
            while (bc > 0 || pc > 0) && ci + 1 < lines.len() {
                ci += 1;
                sig.push(' ');
                sig.push_str(lines[ci].trim());
                for ch in lines[ci].chars() {
                    if ch == '{' {
                        bc += 1;
                    } else if ch == '}' {
                        bc -= 1;
                    } else if ch == '(' {
                        pc += 1;
                    } else if ch == ')' {
                        pc -= 1;
                    }
                }
            }
            return Ok(ToolResult::ok(format!(
                "Signature of '{}' @{}:\n{}",
                sym,
                i + 1,
                sig
            )));
        }
    }
    Ok(ToolResult {
        output: format!("Symbol '{}' not found", sym),
        success: false,
    })
}

pub fn find_references(args: &Value, root: &Path) -> Result<ToolResult> {
    let sym = args
        .get("symbol")
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: symbol"))?;
    let search_root = if let Some(p) = args
        .get("root")
        .or_else(|| args.get("dir"))
        .and_then(|v| v.as_str())
    {
        resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let ext = args.get("ext").and_then(|v| v.as_str());
    let mut refs = Vec::new();
    for entry in walkdir::WalkDir::new(&search_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let p = entry.path();
        if let Some(e) = ext {
            if !p
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
                .ends_with(e)
            {
                continue;
            }
        }
        if let Some(e) = p.extension().and_then(|e| e.to_str()) {
            if !matches!(
                e,
                "rs" | "py"
                    | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "go"
                    | "rb"
                    | "java"
                    | "c"
                    | "cpp"
                    | "h"
                    | "hpp"
            ) {
                continue;
            }
        } else {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(p) {
            for (ln, line) in content.lines().enumerate() {
                if line.contains(&sym) {
                    refs.push((
                        p.strip_prefix(&search_root)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .to_string(),
                        ln + 1,
                        line.trim().to_string(),
                    ));
                }
            }
        }
    }
    if refs.is_empty() {
        Ok(ToolResult {
            output: format!("No refs for '{}'", sym),
            success: false,
        })
    } else {
        let mut out = format!("Refs to '{}' ({}):\n", sym, refs.len());
        for (f, l, s) in refs {
            out.push_str(&format!("{}:{} {}\n", f, l, s));
        }
        Ok(ToolResult::ok(out))
    }
}

fn detect_lang(p: &Path) -> &str {
    match p.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts") | Some("tsx") => "ts",
        Some("js") | Some("jsx") => "js",
        _ => "unknown",
    }
}

fn parse_rust(src: &str) -> Vec<Symbol> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("Error loading Rust grammar");
    let tree = parser.parse(src, None).unwrap();
    let mut syms = Vec::new();
    fn walk(node: Node, src: &str, cont: &Option<String>, syms: &mut Vec<Symbol>) {
        match node.kind() {
            "function_item" | "associated_function" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "fn".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: cont.clone(),
                    signature: Some(src_to_node_utf8(node, src)),
                });
            }
            "struct_item" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "struct".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: None,
                    signature: Some(src_to_node_utf8(node, src)),
                });
                let mut cur = node.walk();
                for ch in node.children(&mut cur) {
                    walk(ch, src, &Some(name.to_string()), syms);
                }
                return;
            }
            "enum_item" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "enum".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: None,
                    signature: Some(src_to_node_utf8(node, src)),
                });
            }
            "trait_item" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "trait".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: None,
                    signature: Some(src_to_node_utf8(node, src)),
                });
            }
            "impl_item" => {
                let name = node
                    .child_by_field_name("trait")
                    .or_else(|| node.child_by_field_name("type"))
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "impl".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: None,
                    signature: Some(src_to_node_utf8(node, src)),
                });
                let mut cur = node.walk();
                for ch in node.children(&mut cur) {
                    walk(ch, src, &Some(name.to_string()), syms);
                }
                return;
            }
            _ => {}
        }
        let mut cur = node.walk();
        for ch in node.children(&mut cur) {
            walk(ch, src, cont, syms);
        }
    }
    walk(tree.root_node(), src, &None, &mut syms);
    syms
}
fn src_to_node_utf8(node: Node, src: &str) -> String {
    node.utf8_text(src.as_bytes()).unwrap_or("").to_string()
}

fn parse_python(src: &str) -> Vec<Symbol> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Error loading Python grammar");
    let tree = parser.parse(src, None).unwrap();
    let mut syms = Vec::new();
    fn walk(node: Node, src: &str, cont: &Option<String>, syms: &mut Vec<Symbol>) {
        match node.kind() {
            "class_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "class".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: None,
                    signature: Some(src_to_node_utf8(node, src)),
                });
                let mut cur = node.walk();
                for ch in node.children(&mut cur) {
                    walk(ch, src, &Some(name.to_string()), syms);
                }
                return;
            }
            "function_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                let k = if cont.is_some() { "method" } else { "fn" };
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: k.into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: cont.clone(),
                    signature: Some(src_to_node_utf8(node, src)),
                });
            }
            _ => {}
        }
        let mut cur = node.walk();
        for ch in node.children(&mut cur) {
            walk(ch, src, cont, syms);
        }
    }
    walk(tree.root_node(), src, &None, &mut syms);
    syms
}

fn parse_ts_js(src: &str) -> Vec<Symbol> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .expect("Error loading TypeScript grammar");
    let tree = parser.parse(src, None).unwrap();
    let mut syms = Vec::new();
    fn walk(node: Node, src: &str, cont: &Option<String>, syms: &mut Vec<Symbol>) {
        match node.kind() {
            "class_declaration" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "class".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: None,
                    signature: Some(src_to_node_utf8(node, src)),
                });
                let mut cur = node.walk();
                for ch in node.children(&mut cur) {
                    walk(ch, src, &Some(name.to_string()), syms);
                }
                return;
            }
            "function_declaration" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| n.utf8_text(src.as_bytes()).unwrap_or("unknown"))
                    .unwrap_or("unknown");
                syms.push(Symbol {
                    name: name.to_string(),
                    kind: "fn".into(),
                    line: node.start_position().row + 1,
                    file: String::new(),
                    container: cont.clone(),
                    signature: Some(src_to_node_utf8(node, src)),
                });
            }
            "lexical_declaration" => {
                let mut cur = node.walk();
                for ch in node.children(&mut cur) {
                    if ch.kind() == "variable_declarator" {
                        if let Some(nm) = ch.child_by_field_name("name") {
                            if ch
                                .child_by_field_name("value")
                                .map(|v| v.kind())
                                .as_ref()
                                .map(|k| *k == "arrow_function")
                                .unwrap_or(false)
                            {
                                let name = nm.utf8_text(src.as_bytes()).unwrap_or("unknown");
                                syms.push(Symbol {
                                    name: name.to_string(),
                                    kind: "arrow".into(),
                                    line: ch.start_position().row + 1,
                                    file: String::new(),
                                    container: cont.clone(),
                                    signature: Some(src_to_node_utf8(ch, src)),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        let mut cur = node.walk();
        for ch in node.children(&mut cur) {
            walk(ch, src, cont, syms);
        }
    }
    walk(tree.root_node(), src, &None, &mut syms);
    syms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_lang_rust() {
        let path = Path::new("test.rs");
        assert_eq!(detect_lang(path), "rust");
    }

    #[test]
    fn test_detect_lang_python() {
        let path = Path::new("test.py");
        assert_eq!(detect_lang(path), "python");
    }

    #[test]
    fn test_detect_lang_typescript() {
        let path = Path::new("test.ts");
        assert_eq!(detect_lang(path), "ts");
    }

    #[test]
    fn test_detect_lang_javascript() {
        let path = Path::new("test.js");
        assert_eq!(detect_lang(path), "js");
    }

    #[test]
    fn test_detect_lang_unknown() {
        let path = Path::new("test.xyz");
        assert_eq!(detect_lang(path), "unknown");
    }

    #[test]
    fn test_get_symbols_missing_path() {
        let args = serde_json::json!({});
        let temp_dir = tempfile::TempDir::new().unwrap();
        let result = get_symbols(&args, temp_dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: path")
        );
    }

    #[test]
    fn test_get_symbols_file_not_found() {
        let args = serde_json::json!({ "path": "nonexistent.rs" });
        let temp_dir = tempfile::TempDir::new().unwrap();
        let result = get_symbols(&args, temp_dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("get_symbols:"));
    }

    #[test]
    fn test_outline_missing_path() {
        let args = serde_json::json!({});
        let temp_dir = tempfile::TempDir::new().unwrap();
        let result = outline(&args, temp_dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: path")
        );
    }

    #[test]
    fn test_get_signature_missing_path() {
        let args = serde_json::json!({ "symbol": "test" });
        let temp_dir = tempfile::TempDir::new().unwrap();
        let result = get_signature(&args, temp_dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: path")
        );
    }

    #[test]
    fn test_get_signature_missing_symbol() {
        let args = serde_json::json!({ "path": "test.rs" });
        let temp_dir = tempfile::TempDir::new().unwrap();
        let result = get_signature(&args, temp_dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: symbol")
        );
    }

    #[test]
    fn test_find_references_missing_symbol() {
        let args = serde_json::json!({});
        let temp_dir = tempfile::TempDir::new().unwrap();
        let result = find_references(&args, temp_dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: symbol")
        );
    }

    #[test]
    fn test_parse_rust_basic() {
        let source = "fn main() { }";
        let symbols = parse_rust(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "main");
        assert_eq!(symbols[0].kind, "fn");
        assert_eq!(symbols[0].line, 1);
    }

    #[test]
    fn test_parse_rust_struct() {
        let source = "struct Point { x: i32, y: i32 }";
        let symbols = parse_rust(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Point");
        assert_eq!(symbols[0].kind, "struct");
    }

    #[test]
    fn test_parse_rust_impl() {
        let source = "impl Point { fn new() -> Self { Point { x: 0, y: 0 } } }";
        let symbols = parse_rust(source);
        assert!(symbols.iter().any(|s| s.kind == "impl"));
        assert!(symbols.iter().any(|s| s.kind == "fn" && s.name == "new"));
    }

    #[test]
    fn test_parse_python_class() {
        let source = "class MyClass:\n    pass";
        let symbols = parse_python(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "MyClass");
        assert_eq!(symbols[0].kind, "class");
    }

    #[test]
    fn test_parse_python_function() {
        let source = "def my_func():\n    pass";
        let symbols = parse_python(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "my_func");
        assert_eq!(symbols[0].kind, "fn");
    }

    #[test]
    fn test_parse_ts_class() {
        let source = "class MyClass {}";
        let symbols = parse_ts_js(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "MyClass");
        assert_eq!(symbols[0].kind, "class");
    }

    #[test]
    fn test_parse_ts_function() {
        let source = "function myFunc() {}";
        let symbols = parse_ts_js(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "myFunc");
        assert_eq!(symbols[0].kind, "fn");
    }

    #[test]
    fn test_parse_ts_arrow() {
        let source = "const myArrow = () => {};";
        let symbols = parse_ts_js(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "myArrow");
        assert_eq!(symbols[0].kind, "arrow");
    }

    #[test]
    fn test_symbol_struct() {
        let sym = Symbol {
            name: "Test".to_string(),
            kind: "struct".to_string(),
            line: 10,
            file: "test.rs".to_string(),
            container: None,
            signature: Some("struct Test { }".to_string()),
        };
        assert_eq!(sym.name, "Test");
        assert_eq!(sym.kind, "struct");
        assert_eq!(sym.line, 10);
    }
}
