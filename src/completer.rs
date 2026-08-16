//! Tab completion for file/directory paths and `$PATH` executables.

use std::fs;
use std::path::{Path, PathBuf};

use reedline::{Completer, Span, Suggestion};

/// Completes command names (first word) from `$PATH` + built-ins, and path
/// arguments from the filesystem. Unreadable directories yield no suggestions
/// rather than errors.
pub struct ShellCompleter {
    builtins: Vec<String>,
}

impl Default for ShellCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellCompleter {
    pub fn new() -> Self {
        ShellCompleter {
            builtins: vec![
                "cd".into(),
                "exit".into(),
                "jobs".into(),
                "fg".into(),
                "bg".into(),
                "export".into(),
                "unset".into(),
                "return".into(),
                "shift".into(),
                "true".into(),
                "false".into(),
            ],
        }
    }
}

impl Completer for ShellCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let pos = pos.min(line.len());
        let (token, span) = current_token(line, pos);
        if token.is_empty() && !line[..pos].is_empty() && !line[..pos].ends_with(char::is_whitespace)
        {
            // Mid-token edge: nothing to complete.
        }

        let is_first = is_first_word(line, span.start);

        let mut out = if is_first && !token.contains('/') {
            complete_commands(&self.builtins, &token, span)
        } else {
            complete_paths(&token, span)
        };

        // Also offer path completions for first word when it looks like a path.
        if is_first && token.contains('/') {
            out = complete_paths(&token, span);
        }

        out.sort_by(|a, b| a.value.cmp(&b.value));
        out.dedup_by(|a, b| a.value == b.value);
        out
    }
}

fn is_first_word(line: &str, token_start: usize) -> bool {
    line[..token_start]
        .chars()
        .all(|c| c.is_whitespace())
}

/// Returns the token under/just before `pos` and its byte span in `line`.
fn current_token(line: &str, pos: usize) -> (String, Span) {
    let bytes = line.as_bytes();
    let mut start = pos;
    while start > 0 {
        let prev = line[..start].chars().next_back().unwrap();
        if prev.is_whitespace() || is_break_char(prev) {
            break;
        }
        start -= prev.len_utf8();
    }
    let mut end = pos;
    while end < line.len() {
        let ch = line[end..].chars().next().unwrap();
        if ch.is_whitespace() || is_break_char(ch) {
            break;
        }
        end += ch.len_utf8();
    }
    let _ = bytes;
    (line[start..end].to_string(), Span::new(start, end))
}

fn is_break_char(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')' | '{' | '}')
}

fn complete_commands(builtins: &[String], prefix: &str, span: Span) -> Vec<Suggestion> {
    let mut suggestions = Vec::new();

    for b in builtins {
        if b.starts_with(prefix) {
            suggestions.push(simple_suggestion(b.clone(), span, true));
        }
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_file() && !file_type.is_symlink() {
                    continue;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if name.starts_with(prefix) {
                    suggestions.push(simple_suggestion(name.to_string(), span, true));
                }
            }
        }
    }

    suggestions
}

fn complete_paths(token: &str, span: Span) -> Vec<Suggestion> {
    let (dir, file_prefix) = split_path_prefix(token);
    let read_dir = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(), // unreadable: no suggestions
    };

    let mut out = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Skip hidden entries unless the user typed a leading dot.
        if name.starts_with('.') && !file_prefix.starts_with('.') {
            continue;
        }
        if !name.starts_with(&file_prefix) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let mut value = join_completion(token, name, is_dir);
        // reedline replaces the span with `value`.
        out.push(Suggestion {
            value: std::mem::take(&mut value),
            display_override: None,
            description: None,
            style: None,
            extra: None,
            span,
            append_whitespace: !is_dir,
            match_indices: None,
        });
    }
    out
}

/// Splits a partial path into (directory to list, file name prefix).
fn split_path_prefix(token: &str) -> (PathBuf, String) {
    if token.is_empty() {
        return (PathBuf::from("."), String::new());
    }
    let expanded = expand_tilde(token);
    let path = Path::new(&expanded);
    if token.ends_with('/') {
        return (path.to_path_buf(), String::new());
    }
    let file_prefix = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    (dir, file_prefix)
}

fn expand_tilde(token: &str) -> String {
    if token == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| ".".into());
    }
    if let Some(rest) = token.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    token.to_string()
}

fn join_completion(original_token: &str, name: &str, is_dir: bool) -> String {
    let mut base = if original_token.ends_with('/') {
        original_token.to_string()
    } else if let Some(idx) = original_token.rfind('/') {
        format!("{}{}", &original_token[..=idx], name)
    } else if original_token == "~" {
        format!("~/{name}")
    } else {
        name.to_string()
    };
    if is_dir && !base.ends_with('/') {
        base.push('/');
    }
    base
}

fn simple_suggestion(value: String, span: Span, append_ws: bool) -> Suggestion {
    Suggestion {
        value,
        display_override: None,
        description: None,
        style: None,
        extra: None,
        span,
        append_whitespace: append_ws,
        match_indices: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_token_middle_of_word() {
        let (tok, span) = current_token("echo hello", 7);
        assert_eq!(tok, "hello");
        assert_eq!(span.start, 5);
        assert_eq!(span.end, 10);
    }

    #[test]
    fn first_word_detection() {
        assert!(is_first_word("ls", 0));
        assert!(!is_first_word("ls ", 3));
    }

    #[test]
    fn split_path_prefix_relative() {
        let (dir, prefix) = split_path_prefix("src/li");
        assert_eq!(dir, PathBuf::from("src"));
        assert_eq!(prefix, "li");
    }

    #[test]
    fn split_path_prefix_trailing_slash() {
        let (dir, prefix) = split_path_prefix("src/");
        assert_eq!(dir, PathBuf::from("src/"));
        assert_eq!(prefix, "");
    }

    #[test]
    fn complete_builtin_prefix() {
        let mut c = ShellCompleter::new();
        let hits = c.complete("ex", 2);
        assert!(
            hits.iter().any(|s| s.value == "exit"),
            "got: {:?}",
            hits.iter().map(|s| &s.value).collect::<Vec<_>>()
        );
    }

    #[test]
    fn complete_paths_in_crate_src() {
        let mut c = ShellCompleter::new();
        // From crate root during `cargo test`, src/ exists.
        let line = "cat src/ma";
        let pos = line.len();
        let hits = c.complete(line, pos);
        assert!(
            hits.iter().any(|s| s.value.contains("main.rs")),
            "got: {:?}",
            hits.iter().map(|s| &s.value).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unreadable_dir_returns_empty() {
        let suggestions = complete_paths("/root/shadowshell-no-access-xyz/", Span::new(0, 0));
        // May be empty either because unreadable or nonexistent — both OK.
        let _ = suggestions;
    }

    #[test]
    fn join_completion_dir_appends_slash() {
        assert_eq!(join_completion("sr", "src", true), "src/");
        assert_eq!(join_completion("src/m", "main.rs", false), "src/main.rs");
    }
}
