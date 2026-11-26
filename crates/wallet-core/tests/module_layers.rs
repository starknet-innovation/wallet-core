//! Imports may loop inside one top-level module of `src/` (`send`, `history`, ...) but never
//! across two: each top-level module must stay movable into a crate of its own as a file move.
//!
//! Ceiling: this reads source text, not the compiler's module tree. It sees `crate::`, `super::`
//! and `use child::` paths (braces included); a path built by a macro or spelled some other way
//! can slip past. `cargo modules` is the upgrade if that matters.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

type Graph = BTreeMap<String, BTreeSet<String>>;

/// Every non-test source file under `dir`, as `module path -> text before #[cfg(test)]`, with
/// `//` comments stripped.
fn sources(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(root, &path, out);
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "tests.rs" {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap().with_extension("");
        let mut parts: Vec<&str> = rel.iter().map(|p| p.to_str().unwrap()).collect();
        if parts.last() == Some(&"mod") {
            parts.pop();
        }
        if parts == ["lib"] {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let text = text.split("#[cfg(test)]").next().unwrap_or_default();
        let text: Vec<&str> = text
            .lines()
            .map(|l| l.split("//").next().unwrap_or_default())
            .collect();
        out.insert(parts.join("::"), text.join("\n"));
    }
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Reads one use-tree at `*i` (`a::b`, `a::{b, c::{d}}`), pushing each leaf path onto `out`.
fn tree(s: &[u8], i: &mut usize, prefix: &[String], out: &mut Vec<Vec<String>>) {
    let mut path = prefix.to_vec();
    loop {
        while s.get(*i).is_some_and(u8::is_ascii_whitespace) {
            *i += 1;
        }
        if s.get(*i) == Some(&b'{') {
            *i += 1;
            loop {
                while s.get(*i).is_some_and(u8::is_ascii_whitespace) {
                    *i += 1;
                }
                match s.get(*i) {
                    None => return,
                    Some(b'}') => {
                        *i += 1;
                        return;
                    }
                    _ => tree(s, i, &path, out),
                }
                // Skip what a leaf leaves behind (`as Name`, `*`) up to the separator.
                while s.get(*i).is_some_and(|&b| b != b',' && b != b'}') {
                    *i += 1;
                }
                if s.get(*i) == Some(&b',') {
                    *i += 1;
                }
            }
        }
        let start = *i;
        while s.get(*i).copied().is_some_and(is_ident) {
            *i += 1;
        }
        if start == *i {
            break;
        }
        path.push(String::from_utf8_lossy(&s[start..*i]).into_owned());
        if !s[*i..].starts_with(b"::") {
            break;
        }
        *i += 2;
    }
    out.push(path);
}

/// The longest prefix of `path` that is a module file.
fn resolve<'a>(files: &'a BTreeMap<String, String>, path: &[String]) -> Option<&'a str> {
    (1..=path.len()).rev().find_map(|n| {
        files
            .get_key_value(&path[..n].join("::"))
            .map(|(k, _)| k.as_str())
    })
}

/// Each occurrence of `word` in `s` that does not continue an identifier.
fn starts<'a>(s: &'a [u8], word: &'a str) -> impl Iterator<Item = usize> + 'a {
    (0..s.len())
        .filter(move |&i| s[i..].starts_with(word.as_bytes()) && (i == 0 || !is_ident(s[i - 1])))
}

fn edges(files: &BTreeMap<String, String>) -> Graph {
    let mut graph = Graph::new();
    for (module, text) in files {
        let s = text.as_bytes();
        let base: Vec<String> = module.split("::").map(String::from).collect();
        let mut paths = Vec::new();
        for at in starts(s, "crate::") {
            tree(s, &mut (at + "crate::".len()), &[], &mut paths);
        }
        for at in starts(s, "super::") {
            if at >= 7 && &s[at - 7..at] == b"super::" {
                continue; // the tail of a chain already read
            }
            let mut i = at;
            let mut up = 0;
            while s[i..].starts_with(b"super::") {
                i += "super::".len();
                up += 1;
            }
            tree(
                s,
                &mut i,
                &base[..base.len().saturating_sub(up)],
                &mut paths,
            );
        }
        // `use child::..` and `use self::child::..`: only paths that name a real child count.
        for at in starts(s, "use") {
            let mut i = at + "use".len();
            if !s.get(i).is_some_and(u8::is_ascii_whitespace) {
                continue;
            }
            while s.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            if s[i..].starts_with(b"self::") {
                i += "self::".len();
            }
            let mut children = Vec::new();
            tree(s, &mut i, &base, &mut children);
            paths.extend(children.into_iter().filter(|p| {
                p.len() > base.len() && files.contains_key(&p[..=base.len()].join("::"))
            }));
        }
        let targets = graph.entry(module.clone()).or_default();
        for path in &paths {
            if let Some(target) = resolve(files, path) {
                if target != module {
                    targets.insert(target.to_owned());
                }
            }
        }
    }
    graph
}

/// Tarjan's strongly connected components with more than one node.
fn cycles(graph: &Graph) -> Vec<Vec<String>> {
    struct Walk<'a> {
        graph: &'a Graph,
        index: BTreeMap<&'a str, usize>,
        low: BTreeMap<&'a str, usize>,
        stack: Vec<&'a str>,
        on_stack: BTreeSet<&'a str>,
        out: Vec<Vec<String>>,
    }
    fn visit<'a>(w: &mut Walk<'a>, v: &'a str) {
        let n = w.index.len();
        w.index.insert(v, n);
        w.low.insert(v, n);
        w.stack.push(v);
        w.on_stack.insert(v);
        for t in &w.graph[v] {
            let t = t.as_str();
            if !w.index.contains_key(t) {
                visit(w, t);
                let low = w.low[v].min(w.low[t]);
                w.low.insert(v, low);
            } else if w.on_stack.contains(t) {
                let low = w.low[v].min(w.index[t]);
                w.low.insert(v, low);
            }
        }
        if w.low[v] == w.index[v] {
            let mut component = Vec::new();
            while let Some(t) = w.stack.pop() {
                w.on_stack.remove(t);
                component.push(t.to_owned());
                if t == v {
                    break;
                }
            }
            if component.len() > 1 {
                component.sort();
                w.out.push(component);
            }
        }
    }
    let mut w = Walk {
        graph,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        stack: Vec::new(),
        on_stack: BTreeSet::new(),
        out: Vec::new(),
    };
    for v in graph.keys() {
        if !w.index.contains_key(v.as_str()) {
            visit(&mut w, v);
        }
    }
    w.out
}

fn top(module: &str) -> &str {
    module.split("::").next().unwrap_or(module)
}

#[test]
fn imports_do_not_loop_across_top_level_modules() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = BTreeMap::new();
    sources(&src, &src, &mut files);
    let graph = edges(&files);
    assert!(
        graph.values().map(BTreeSet::len).sum::<usize>() > 100,
        "parsed too few imports"
    );

    let mut report = String::new();
    for component in cycles(&graph) {
        let tops: BTreeSet<&str> = component.iter().map(|m| top(m)).collect();
        if tops.len() < 2 {
            continue;
        }
        report += &format!("\nloop across {tops:?}: {component:?}\n");
        for from in &component {
            for to in graph[from].iter().filter(|t| component.contains(t)) {
                report += &format!("    {from} -> {to}\n");
            }
        }
    }
    assert!(
        report.is_empty(),
        "imports loop across top-level modules; break one edge:{report}"
    );
}

#[test]
fn use_trees_expand_braces() {
    let mut paths = Vec::new();
    let s = b"crate::{felt, send::{calls::Call, plan as p}, Felt};";
    tree(s, &mut "crate::".len(), &[], &mut paths);
    let paths: Vec<String> = paths.iter().map(|p| p.join("::")).collect();
    assert_eq!(paths, ["felt", "send::calls::Call", "send::plan", "Felt"]);
}
