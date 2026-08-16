//! Shell aliases: word-replacement shortcuts applied to the first word of a
//! simple command. Includes a built-in set of developer-oriented defaults.

use std::collections::HashMap;

/// Built-in aliases useful for day-to-day development. Seeded into every new
/// shell session; users can override or remove them with `alias` / `unalias`.
pub const DEFAULT_ALIASES: &[(&str, &str)] = &[
    // Navigation / listing
    ("ll", "ls -lah"),
    ("la", "ls -A"),
    ("l", "ls -CF"),
    ("..", "cd .."),
    ("...", "cd ../.."),
    ("md", "mkdir -p"),
    // Safer / nicer defaults
    ("grep", "grep --color=auto"),
    ("egrep", "egrep --color=auto"),
    ("fgrep", "fgrep --color=auto"),
    ("cls", "clear"),
    // Git shortcuts
    ("g", "git"),
    ("gs", "git status"),
    ("ga", "git add"),
    ("gc", "git commit"),
    ("gp", "git push"),
    ("gl", "git pull"),
    ("gd", "git diff"),
    ("gb", "git branch"),
    ("gco", "git checkout"),
    ("glog", "git log --oneline --graph --decorate"),
    // Cargo / Rust
    ("c", "cargo"),
    ("cb", "cargo build"),
    ("ct", "cargo test"),
    ("cr", "cargo run"),
    ("cc", "cargo check"),
    ("cclip", "cargo clippy"),
    // Misc
    ("please", "sudo"),
    ("ports", "ss -tulpn"),
];

/// Builds a map of the built-in developer aliases.
pub fn default_alias_map() -> HashMap<String, String> {
    DEFAULT_ALIASES
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Names of all default aliases (for highlighter / completer).
pub fn default_alias_names() -> impl Iterator<Item = &'static str> {
    DEFAULT_ALIASES.iter().map(|(k, _)| *k)
}

/// Splits an alias value into words on ASCII whitespace.
/// Defaults only use simple space-separated tokens (no quoting).
pub fn split_alias_value(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Whether `name` is a valid alias name. Allows letters, digits, `_`, `.`,
/// `-`, and `+` so names like `..` and `gco` work.
pub fn is_valid_alias_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // Reject pure operators / shell metacharacters as alias names.
    if name.chars().all(|c| matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')' | '{' | '}')) {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '+'))
}

/// Applies alias expansion to the command word list.
/// Only the first word is expanded; expansion is recursive with a visit set
/// so `alias a=b` / `alias b=a` cannot loop forever.
pub fn expand_command_words(
    program: String,
    args: Vec<String>,
    aliases: &HashMap<String, String>,
) -> (String, Vec<String>) {
    let mut program = program;
    let mut args = args;
    let mut seen = std::collections::HashSet::new();

    loop {
        if !seen.insert(program.clone()) {
            break;
        }
        let Some(value) = aliases.get(&program) else {
            break;
        };
        let mut words = split_alias_value(value);
        if words.is_empty() {
            break;
        }
        program = words.remove(0);
        words.append(&mut args);
        args = words;
    }

    (program, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map_contains_common_dev_aliases() {
        let map = default_alias_map();
        assert_eq!(map.get("ll").map(String::as_str), Some("ls -lah"));
        assert_eq!(map.get("gs").map(String::as_str), Some("git status"));
        assert_eq!(map.get("cb").map(String::as_str), Some("cargo build"));
        assert_eq!(map.get("..").map(String::as_str), Some("cd .."));
    }

    #[test]
    fn split_alias_value_on_spaces() {
        assert_eq!(
            split_alias_value("ls -lah"),
            vec!["ls".to_string(), "-lah".to_string()]
        );
    }

    #[test]
    fn expand_prepends_alias_words() {
        let mut map = HashMap::new();
        map.insert("ll".into(), "ls -lah".into());
        let (prog, args) = expand_command_words("ll".into(), vec!["src".into()], &map);
        assert_eq!(prog, "ls");
        assert_eq!(args, vec!["-lah".to_string(), "src".to_string()]);
    }

    #[test]
    fn expand_is_recursive() {
        let mut map = HashMap::new();
        map.insert("ll".into(), "l -a".into());
        map.insert("l".into(), "ls -CF".into());
        let (prog, args) = expand_command_words("ll".into(), vec![], &map);
        assert_eq!(prog, "ls");
        assert_eq!(args, vec!["-CF".to_string(), "-a".to_string()]);
    }

    #[test]
    fn expand_breaks_cycles() {
        let mut map = HashMap::new();
        map.insert("a".into(), "b".into());
        map.insert("b".into(), "a".into());
        let (prog, args) = expand_command_words("a".into(), vec!["x".into()], &map);
        // Stops when a name is seen twice; final program is one of the cycle.
        assert!(prog == "a" || prog == "b");
        assert_eq!(args, vec!["x".to_string()]);
    }

    #[test]
    fn valid_alias_names() {
        assert!(is_valid_alias_name("ll"));
        assert!(is_valid_alias_name(".."));
        assert!(is_valid_alias_name("g-co"));
        assert!(!is_valid_alias_name(""));
        assert!(!is_valid_alias_name("a b"));
        assert!(!is_valid_alias_name("|"));
    }
}
