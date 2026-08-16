//! Syntax highlighting for the interactive command line via reedline's
//! [`Highlighter`] trait: valid external/built-in commands, unknown
//! commands, quoted strings, operators, and comments.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nu_ansi_term::Style;
use reedline::{Highlighter, StyledText};

use crate::aliases;
use crate::config::Theme;

/// Built-in command names highlighted as valid even without a PATH entry.
const BUILTINS: &[&str] = &[
    "cd", "exit", "jobs", "fg", "bg", "export", "unset", "return", "shift", "alias", "unalias", "help", ":", "true", "false",
];

/// Highlights buffer text using the active theme and a cached PATH index.
pub struct ShellHighlighter {
    theme: Theme,
    /// Lazily built set of executable basenames found on `$PATH`.
    path_cache: Mutex<Option<HashSet<String>>>,
}

impl ShellHighlighter {
    pub fn new(theme: Theme) -> Self {
        ShellHighlighter {
            theme,
            path_cache: Mutex::new(None),
        }
    }

    fn path_executables(&self) -> HashSet<String> {
        let mut guard = self.path_cache.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(scan_path_executables());
        }
        guard.clone().unwrap_or_default()
    }

    fn is_valid_command(&self, name: &str) -> bool {
        if BUILTINS.contains(&name) {
            return true;
        }
        if aliases::default_alias_names().any(|a| a == name) {
            return true;
        }
        // Absolute/relative path: valid if the file exists and is executable-ish.
        if name.contains('/') {
            let p = Path::new(name);
            return p.is_file();
        }
        self.path_executables().contains(name)
    }
}

impl Highlighter for ShellHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut out = StyledText::new();
        let mut chars = line.char_indices().peekable();
        let mut first_word = true;

        while let Some((i, c)) = chars.peek().copied() {
            match c {
                '#' if is_comment_start(line, i) => {
                    // Rest of line is a comment.
                    let rest = &line[i..];
                    out.push((comment_style(&self.theme), rest.to_string()));
                    break;
                }
                '\'' | '"' => {
                    let (token, consumed) = take_quoted(line, i, c);
                    out.push((string_style(&self.theme), token));
                    advance(&mut chars, consumed);
                    first_word = false;
                }
                ch if is_operator_char(ch) => {
                    let (token, consumed) = take_operators(line, i);
                    let reset = token.contains('|')
                        || token.contains(';')
                        || token == "&&"
                        || token == "||"
                        || token == "&";
                    out.push((operator_style(&self.theme), token));
                    advance(&mut chars, consumed);
                    if reset {
                        first_word = true;
                    }
                }
                ch if ch.is_whitespace() => {
                    out.push((Style::new(), ch.to_string()));
                    chars.next();
                }
                _ => {
                    let (token, consumed) = take_word(line, i);
                    let style = if first_word {
                        first_word = false;
                        if self.is_valid_command(&token) {
                            valid_cmd_style(&self.theme)
                        } else {
                            unknown_cmd_style(&self.theme)
                        }
                    } else {
                        Style::new()
                    };
                    out.push((style, token));
                    advance(&mut chars, consumed);
                }
            }
        }
        out
    }
}

fn is_comment_start(line: &str, idx: usize) -> bool {
    // Comment when `#` is at a token boundary (start or after whitespace).
    if idx == 0 {
        return true;
    }
    line[..idx]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_whitespace())
}

fn is_operator_char(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')' | '{' | '}')
}

fn take_quoted(line: &str, start: usize, quote: char) -> (String, usize) {
    let bytes = line.as_bytes();
    let mut i = start + quote.len_utf8();
    while i < line.len() {
        let ch = line[i..].chars().next().unwrap();
        if ch == '\\' && quote == '"' {
            i += 1;
            if i < line.len() {
                i += line[i..].chars().next().unwrap().len_utf8();
            }
            continue;
        }
        i += ch.len_utf8();
        if ch == quote {
            break;
        }
        let _ = bytes; // silence unused in some builds
    }
    (line[start..i].to_string(), i - start)
}

fn take_operators(line: &str, start: usize) -> (String, usize) {
    let rest = &line[start..];
    // Prefer multi-char operators.
    for op in ["||", "&&", ">>", "2>>", "2>", "<<"] {
        if rest.starts_with(op) {
            return (op.to_string(), op.len());
        }
    }
    let ch = rest.chars().next().unwrap();
    (ch.to_string(), ch.len_utf8())
}

fn take_word(line: &str, start: usize) -> (String, usize) {
    let mut end = start;
    for (offset, ch) in line[start..].char_indices() {
        if ch.is_whitespace() || is_operator_char(ch) || ch == '\'' || ch == '"' || ch == '#' {
            end = start + offset;
            break;
        }
        end = start + offset + ch.len_utf8();
    }
    (line[start..end].to_string(), end - start)
}

fn advance(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, bytes: usize) {
    if bytes == 0 {
        chars.next();
        return;
    }
    let target = chars.peek().map(|(i, _)| *i).unwrap_or(0) + bytes;
    while chars.peek().is_some_and(|(i, _)| *i < target) {
        chars.next();
    }
}

fn valid_cmd_style(theme: &Theme) -> Style {
    Style::new().fg(theme.command_valid.to_nu_ansi())
}
fn unknown_cmd_style(theme: &Theme) -> Style {
    Style::new().fg(theme.command_unknown.to_nu_ansi())
}
fn string_style(theme: &Theme) -> Style {
    Style::new().fg(theme.string.to_nu_ansi())
}
fn operator_style(theme: &Theme) -> Style {
    Style::new().fg(theme.operator.to_nu_ansi())
}
fn comment_style(theme: &Theme) -> Style {
    Style::new().fg(theme.comment.to_nu_ansi()).italic()
}

fn scan_path_executables() -> HashSet<String> {
    let mut set = HashSet::new();
    let Some(path) = std::env::var_os("PATH") else {
        return set;
    };
    for dir in std::env::split_paths(&path) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue, // unreadable dir: skip
        };
        for entry in entries.flatten() {
            let path: PathBuf = entry.path();
            if path.is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                set.insert(name.to_string());
            }
        }
    }
    // Always include common shell utilities used in tests even if PATH is odd.
    for b in BUILTINS {
        set.insert((*b).to_string());
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::theme_onedark;

    fn highlight(line: &str) -> StyledText {
        ShellHighlighter::new(theme_onedark()).highlight(line, 0)
    }

    #[test]
    fn known_builtin_is_valid_colored() {
        let h = ShellHighlighter::new(theme_onedark());
        assert!(h.is_valid_command("cd"));
        assert!(h.is_valid_command("true"));
    }

    #[test]
    fn unknown_command_detected() {
        let h = ShellHighlighter::new(theme_onedark());
        assert!(!h.is_valid_command("shadowshell-definitely-not-a-command-xyz"));
    }

    #[test]
    fn take_quoted_handles_double() {
        let (tok, n) = take_quoted(r#"echo "hi there""#, 5, '"');
        assert_eq!(tok, r#""hi there""#);
        assert_eq!(n, tok.len());
    }

    #[test]
    fn take_word_stops_at_space() {
        let (tok, _) = take_word("ls -la", 0);
        assert_eq!(tok, "ls");
    }

    #[test]
    fn highlight_produces_output_for_simple_line() {
        // Smoke: highlighting a known builtin must not panic.
        let _styled = highlight("cd /tmp");
    }

    #[test]
    fn comment_start_rules() {
        assert!(is_comment_start("# hi", 0));
        assert!(is_comment_start("echo # hi", 5));
        assert!(!is_comment_start("a#b", 1));
    }
}
