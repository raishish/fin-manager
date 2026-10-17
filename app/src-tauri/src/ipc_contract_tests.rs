//! The UI's `invoke()` calls against the commands the app registers (review T3.3 round 2:
//! a command gained a required argument and its TypeScript wrapper didn't, so the
//! dashboard never loaded, and nothing caught it because TypeScript doesn't type
//! `invoke`'s arguments).
//!
//! This reads the source on both sides:
//! - every `#[tauri::command]` function under `app/src-tauri/src`, with its arguments
//!   (Tauri's camelCase names; `Option<…>` ones are optional; `State`, `Request` and other
//!   injected parameters aren't arguments);
//! - every `invoke('name', …)` call under `app/src` (`.ts` and `.svelte`).
//!
//! and checks that each call names a registered command, passes every required argument,
//! and passes nothing the command doesn't take. Arguments must be an object literal, so
//! they can be read here; only a command taking the raw `Request` body (an import) gets
//! anything else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Command {
    /// `(camelCase name, required)`.
    args: Vec<(String, bool)>,
    /// Takes `tauri::ipc::Request`: the body is raw bytes, not named arguments.
    raw_body: bool,
}

#[derive(Debug)]
struct Call {
    file: String,
    command: String,
    /// `None` without arguments; `Some(Err(text))` when they aren't an object literal.
    args: Option<Result<Vec<String>, String>>,
}

fn files(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "node_modules") {
                files(&path, extensions, out);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.contains(&e))
        {
            out.push(path);
        }
    }
}

/// The text between the bracket at `open` and its match, and the index after it.
fn balanced(s: &str, open: usize, left: u8, right: u8) -> (&str, usize) {
    let b = s.as_bytes();
    let mut depth = 0;
    for (i, &c) in b.iter().enumerate().skip(open) {
        if c == left {
            depth += 1;
        } else if c == right {
            depth -= 1;
            if depth == 0 {
                return (&s[open + 1..i], i + 1);
            }
        }
    }
    panic!("unbalanced brackets");
}

/// Splits on commas outside any brackets.
fn top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Parameters Tauri injects rather than reading from the call's arguments.
fn injected(ty: &str) -> bool {
    let ty = ty.trim_start_matches("tauri::").trim_start_matches("ipc::");
    [
        "State<",
        "Request<",
        "AppHandle",
        "Window",
        "WebviewWindow",
        "Webview",
    ]
    .iter()
    .any(|p| ty.starts_with(p))
}

fn rust_commands() -> BTreeMap<String, Command> {
    let mut sources = Vec::new();
    files(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &["rs"],
        &mut sources,
    );
    let mut commands = BTreeMap::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        let mut at = 0;
        while let Some(found) = text[at..].find("#[tauri::command") {
            let attr = at + found;
            let fn_at = attr + text[attr..].find("fn ").unwrap() + 3;
            let open = fn_at + text[fn_at..].find('(').unwrap();
            let name = text[fn_at..open].trim().to_string();
            let (params, end) = balanced(&text, open, b'(', b')');
            let mut command = Command {
                args: Vec::new(),
                raw_body: false,
            };
            for param in top_level(params) {
                let (arg, ty) = param.split_once(':').unwrap();
                let ty = ty.trim();
                if ty.contains("Request<") {
                    command.raw_body = true;
                }
                if injected(ty) {
                    continue;
                }
                let arg = arg.trim().trim_start_matches("mut ").trim();
                command.args.push((camel(arg), !ty.starts_with("Option<")));
            }
            assert!(
                commands.insert(name.clone(), command).is_none(),
                "two command functions named {name}"
            );
            at = end;
        }
    }
    commands
}

fn ts_calls() -> Vec<Call> {
    let mut sources = Vec::new();
    files(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src"),
        &["ts", "svelte"],
        &mut sources,
    );
    let mut calls = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let b = text.as_bytes();
        let mut at = 0;
        while let Some(found) = text[at..].find("invoke") {
            let start = at + found;
            at = start + "invoke".len();
            let word_start =
                start == 0 || !(b[start - 1].is_ascii_alphanumeric() || b[start - 1] == b'_');
            if !word_start {
                continue;
            }
            let mut i = at;
            let skip_ws = |i: &mut usize| {
                while *i < b.len() && b[*i].is_ascii_whitespace() {
                    *i += 1;
                }
            };
            skip_ws(&mut i);
            if i < b.len() && b[i] == b'<' {
                i = balanced(&text, i, b'<', b'>').1;
                skip_ws(&mut i);
            }
            if i >= b.len() || b[i] != b'(' {
                continue; // `import { invoke }`, a type, a comment.
            }
            i += 1;
            skip_ws(&mut i);
            let quote = b[i];
            assert!(
                matches!(quote, b'\'' | b'"'),
                "{file}: invoke's command must be a string literal"
            );
            let name_end = i + 1 + text[i + 1..].find(quote as char).unwrap();
            let command = text[i + 1..name_end].to_string();
            i = name_end + 1;
            skip_ws(&mut i);
            let args = if b[i] == b')' {
                None
            } else {
                assert_eq!(b[i], b',', "{file}: {command}");
                i += 1;
                skip_ws(&mut i);
                if b[i] == b'{' {
                    let (body, _) = balanced(&text, i, b'{', b'}');
                    let keys: Result<Vec<String>, String> = top_level(body)
                        .into_iter()
                        .map(|item| {
                            if item.starts_with("...") {
                                return Err(format!("a spread ({item})"));
                            }
                            let key = item.split(':').next().unwrap().trim();
                            Ok(key.trim_matches(|c| c == '\'' || c == '"').to_string())
                        })
                        .collect();
                    Some(keys)
                } else {
                    let end = text[i..].find([',', ')']).map_or(text.len(), |e| i + e);
                    Some(Err(text[i..end].trim().to_string()))
                }
            };
            calls.push(Call {
                file: file.clone(),
                command,
                args,
            });
        }
    }
    calls
}

#[test]
fn every_invoke_matches_a_registered_command() {
    let commands = rust_commands();
    let registered: BTreeSet<&str> = crate::COMMANDS.iter().copied().collect();
    for name in &registered {
        assert!(
            commands.contains_key(*name),
            "{name} is registered but no #[tauri::command] fn was found"
        );
    }

    let calls = ts_calls();
    assert!(calls.len() >= 50, "found only {} invoke calls", calls.len());
    let mut problems = Vec::new();
    for call in &calls {
        let at = format!("{}: invoke('{}')", call.file, call.command);
        let Some(command) = commands
            .get(&call.command)
            .filter(|_| registered.contains(call.command.as_str()))
        else {
            problems.push(format!("{at}: no such registered command"));
            continue;
        };
        if command.raw_body {
            if !matches!(call.args, Some(Err(_))) {
                problems.push(format!("{at}: takes the file's bytes as the body"));
            }
            continue;
        }
        let given: BTreeSet<&str> = match &call.args {
            None => BTreeSet::new(),
            Some(Ok(keys)) => keys.iter().map(String::as_str).collect(),
            Some(Err(what)) => {
                problems.push(format!(
                    "{at}: arguments must be an object literal, not `{what}`"
                ));
                continue;
            }
        };
        for (arg, required) in &command.args {
            if *required && !given.contains(arg.as_str()) {
                problems.push(format!("{at}: missing required argument `{arg}`"));
            }
        }
        let known: BTreeSet<&str> = command.args.iter().map(|(a, _)| a.as_str()).collect();
        for key in given.difference(&known) {
            problems.push(format!("{at}: unknown argument `{key}`"));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// The checker itself: the bug it was written for is caught.
#[test]
fn the_checker_catches_a_missing_argument() {
    let commands = rust_commands();
    let dashboard = &commands["dashboard"];
    assert!(dashboard.args.contains(&("today".into(), true)));
    assert!(dashboard.args.contains(&("month".into(), false)));
    assert!(commands["import_file"].raw_body);
    assert!(
        commands["correct_transaction"]
            .args
            .contains(&("transactionId".into(), true)),
        "snake_case arguments are matched in camelCase"
    );
    assert_eq!(
        top_level("month, currency, today: localToday()"),
        ["month", "currency", "today: localToday()"]
    );
}
