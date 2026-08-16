//! Word expansion: tilde, parameter (`$VAR`, `${VAR}`, specials), field
//! splitting on `IFS`, and basic pathname globbing. Command substitution is
//! handled by the interpreter (needs recursive execution) via a callback.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::env::ShellEnv;
use crate::parser::{Word, WordPart};

/// Expands a single word into one or more fields.
///
/// `command_sub` is invoked for each `$(...)`; it should return captured
/// stdout with trailing newlines already trimmed.
pub fn expand_word(
    word: &Word,
    env: &ShellEnv,
    mut command_sub: impl FnMut(&str) -> String,
) -> Vec<String> {
    // (is_unquoted, text) after parameter/command expansion.
    let mut chunks: Vec<(bool, String)> = Vec::new();

    for part in &word.parts {
        match part {
            WordPart::SingleQuoted(s) => chunks.push((false, s.clone())),
            WordPart::DoubleQuoted(s) => {
                let expanded = expand_in_string(s, env, &mut command_sub);
                chunks.push((false, expanded));
            }
            WordPart::Unquoted(s) => {
                let expanded = expand_in_string(s, env, &mut command_sub);
                chunks.push((true, expanded));
            }
        }
    }

    // Tilde: only at the start of an unquoted first chunk.
    if let Some((true, first)) = chunks.first_mut()
        && let Some(expanded) = try_tilde_expand(first, env)
    {
        *first = expanded;
    }

    let any_unquoted = chunks.iter().any(|(u, _)| *u);
    if !any_unquoted {
        let joined: String = chunks.into_iter().map(|(_, s)| s).collect();
        return vec![joined];
    }

    let ifs = env.get("IFS").unwrap_or(" \t\n");
    let fields = split_chunks(&chunks, ifs);

    // Unquoted empty expansion (e.g. unset `$VAR`) still produces one empty field.
    if fields.is_empty() {
        return vec![String::new()];
    }

    fields
        .into_iter()
        .flat_map(|(field, glob_ok)| {
            if glob_ok && has_glob_meta(&field) {
                let matches = glob_expand(&field);
                if matches.is_empty() {
                    vec![field] // nullglob off
                } else {
                    matches
                }
            } else {
                vec![field]
            }
        })
        .collect()
}

/// Expands a list of words and flattens the resulting fields.
pub fn expand_words(
    words: &[Word],
    env: &ShellEnv,
    mut command_sub: impl FnMut(&str) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    for w in words {
        out.extend(expand_word(w, env, &mut command_sub));
    }
    out
}

/// Expand a word to a single string (assignment RHS, redirect targets).
/// Multiple split fields are joined with a space.
pub fn expand_word_unsplit(
    word: &Word,
    env: &ShellEnv,
    command_sub: impl FnMut(&str) -> String,
) -> String {
    expand_word(word, env, command_sub).join(" ")
}

/// Like [`expand_word_unsplit`] but never applies pathname globbing.
/// Used for `case` patterns so `*` remains a pattern metacharacter.
pub fn expand_word_no_glob(
    word: &Word,
    env: &ShellEnv,
    mut command_sub: impl FnMut(&str) -> String,
) -> String {
    let mut chunks: Vec<(bool, String)> = Vec::new();
    for part in &word.parts {
        match part {
            WordPart::SingleQuoted(s) => chunks.push((false, s.clone())),
            WordPart::DoubleQuoted(s) => {
                chunks.push((false, expand_in_string(s, env, &mut command_sub)));
            }
            WordPart::Unquoted(s) => {
                chunks.push((true, expand_in_string(s, env, &mut command_sub)));
            }
        }
    }
    if let Some((true, first)) = chunks.first_mut()
        && let Some(expanded) = try_tilde_expand(first, env)
    {
        *first = expanded;
    }
    chunks.into_iter().map(|(_, s)| s).collect()
}

fn expand_in_string(
    input: &str,
    env: &ShellEnv,
    command_sub: &mut dyn FnMut(&str) -> String,
) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    name.push(ch);
                }
                if !closed {
                    out.push_str("${");
                    out.push_str(&name);
                    continue;
                }
                out.push_str(&resolve_param(&name, env));
            }
            Some('(') => {
                chars.next();
                let mut depth = 1;
                let mut body = String::new();
                for ch in chars.by_ref() {
                    if ch == '(' {
                        depth += 1;
                        body.push(ch);
                    } else if ch == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        body.push(ch);
                    } else {
                        body.push(ch);
                    }
                }
                out.push_str(&command_sub(&body));
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let mut name = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        name.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str(&resolve_param(&name, env));
            }
            Some(c) if c.is_ascii_digit() => {
                let mut name = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_digit() {
                        name.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str(&resolve_param(&name, env));
            }
            Some('?' | '#' | '$' | '!' | '-' | '*') => {
                let name = chars.next().unwrap().to_string();
                out.push_str(&resolve_param(&name, env));
            }
            _ => out.push('$'),
        }
    }
    out
}

fn resolve_param(name: &str, env: &ShellEnv) -> String {
    if let Some(special) = env.expand_special(name) {
        return special;
    }
    env.get(name).unwrap_or("").to_string()
}

fn try_tilde_expand(s: &str, env: &ShellEnv) -> Option<String> {
    if !s.starts_with('~') {
        return None;
    }
    let rest = &s[1..];
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let home = env
        .get("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    if rest.is_empty() {
        Some(home.display().to_string())
    } else {
        let stripped = rest.strip_prefix('/').unwrap_or(rest);
        Some(home.join(stripped).display().to_string())
    }
}

/// Splits chunks into fields. `glob_ok` is true only when the field was
/// produced entirely from unquoted text.
fn split_chunks(chunks: &[(bool, String)], ifs: &str) -> Vec<(String, bool)> {
    let ifs_set: HashSet<char> = ifs.chars().collect();
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut glob_ok = true;
    let mut in_field = false;

    for (is_unquoted, text) in chunks {
        if !*is_unquoted {
            current.push_str(text);
            glob_ok = false;
            in_field = true;
            continue;
        }
        for c in text.chars() {
            if ifs_set.contains(&c) {
                if in_field {
                    fields.push((std::mem::take(&mut current), glob_ok));
                    glob_ok = true;
                    in_field = false;
                }
            } else {
                current.push(c);
                in_field = true;
            }
        }
    }
    if in_field {
        fields.push((current, glob_ok));
    }
    fields
}

fn has_glob_meta(s: &str) -> bool {
    s.chars().any(|c| c == '*' || c == '?' || c == '[')
}

fn glob_expand(pattern: &str) -> Vec<String> {
    match glob::glob(pattern) {
        Ok(paths) => paths
            .filter_map(|e| e.ok())
            .map(|p| p.display().to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(vars: &[(&str, &str)]) -> ShellEnv {
        let mut env = ShellEnv::empty_for_test();
        for (k, v) in vars {
            env.set(*k, *v);
        }
        env.set("HOME", "/home/test");
        env
    }

    fn word_unquoted(s: &str) -> Word {
        Word {
            parts: vec![WordPart::Unquoted(s.into())],
        }
    }

    fn no_sub(_: &str) -> String {
        String::new()
    }

    #[test]
    fn literal_word() {
        let env = env_with(&[]);
        assert_eq!(
            expand_word(&word_unquoted("hello"), &env, no_sub),
            vec!["hello"]
        );
    }

    #[test]
    fn param_expansion() {
        let env = env_with(&[("FOO", "bar")]);
        assert_eq!(
            expand_word(&word_unquoted("$FOO"), &env, no_sub),
            vec!["bar"]
        );
        assert_eq!(
            expand_word(&word_unquoted("${FOO}"), &env, no_sub),
            vec!["bar"]
        );
    }

    #[test]
    fn special_status() {
        let mut env = env_with(&[]);
        env.set_last_status(42);
        assert_eq!(expand_word(&word_unquoted("$?"), &env, no_sub), vec!["42"]);
    }

    #[test]
    fn unset_is_empty() {
        let env = env_with(&[]);
        assert_eq!(expand_word(&word_unquoted("$NOPE"), &env, no_sub), vec![""]);
    }

    #[test]
    fn single_quotes_suppress_expansion() {
        let env = env_with(&[("FOO", "bar")]);
        let w = Word {
            parts: vec![WordPart::SingleQuoted("$FOO".into())],
        };
        assert_eq!(expand_word(&w, &env, no_sub), vec!["$FOO"]);
    }

    #[test]
    fn double_quotes_expand_params() {
        let env = env_with(&[("FOO", "bar")]);
        let w = Word {
            parts: vec![WordPart::DoubleQuoted("x$FOO".into())],
        };
        assert_eq!(expand_word(&w, &env, no_sub), vec!["xbar"]);
    }

    #[test]
    fn field_split_on_spaces() {
        let env = env_with(&[("A", "one two")]);
        assert_eq!(
            expand_word(&word_unquoted("$A"), &env, no_sub),
            vec!["one", "two"]
        );
    }

    #[test]
    fn double_quotes_suppress_split() {
        let env = env_with(&[("A", "one two")]);
        let w = Word {
            parts: vec![WordPart::DoubleQuoted("$A".into())],
        };
        assert_eq!(expand_word(&w, &env, no_sub), vec!["one two"]);
    }

    #[test]
    fn tilde_expands_at_start() {
        let env = env_with(&[]);
        assert_eq!(
            expand_word(&word_unquoted("~/docs"), &env, no_sub),
            vec!["/home/test/docs"]
        );
    }

    #[test]
    fn command_substitution_callback() {
        let env = env_with(&[]);
        let result = expand_word(&word_unquoted("x$(echo hi)y"), &env, |body| {
            assert_eq!(body, "echo hi");
            "hi".into()
        });
        assert_eq!(result, vec!["xhiy"]);
    }

    #[test]
    fn expand_word_unsplit_joins() {
        let env = env_with(&[("A", "one two")]);
        assert_eq!(
            expand_word_unsplit(&word_unquoted("$A"), &env, no_sub),
            "one two"
        );
    }
}
