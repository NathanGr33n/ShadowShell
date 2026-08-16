//! Shell environment: variables, export flags, positional parameters, and
//! the last-command status (`$?`). Seeded from the process environment at
//! startup; exported names are pushed into child process environments.

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;

/// In-process shell environment shared by the interactive loop, scripts,
/// and the interpreter.
#[derive(Debug, Clone)]
pub struct ShellEnv {
    vars: HashMap<String, String>,
    exported: HashSet<String>,
    /// `$0` — shell or script name.
    pub shell_name: String,
    /// `$1`, `$2`, … positional parameters.
    positionals: Vec<String>,
    /// Last pipeline / command exit status (`$?`).
    last_status: i32,
}

impl Default for ShellEnv {
    fn default() -> Self {
        Self::from_process_env("shadowshell")
    }
}

impl ShellEnv {
    /// Builds an environment seeded from the current process environment.
    /// Every inherited variable is marked exported (POSIX login/shell start).
    pub fn from_process_env(shell_name: impl Into<String>) -> Self {
        let mut vars = HashMap::new();
        let mut exported = HashSet::new();
        for (key, value) in env::vars() {
            exported.insert(key.clone());
            vars.insert(key, value);
        }
        ShellEnv {
            vars,
            exported,
            shell_name: shell_name.into(),
            positionals: Vec::new(),
            last_status: 0,
        }
    }

    /// Empty environment for unit tests (no process env inheritance).
    #[cfg(test)]
    pub fn empty_for_test() -> Self {
        ShellEnv {
            vars: HashMap::new(),
            exported: HashSet::new(),
            shell_name: "shadowshell".to_string(),
            positionals: Vec::new(),
            last_status: 0,
        }
    }

    /// Returns the value of `name`, or `None` if unset.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// Sets `name` to `value` without changing its export flag.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(name.into(), value.into());
    }

    /// Removes `name` from the variable map and export set.
    pub fn unset(&mut self, name: &str) {
        self.vars.remove(name);
        self.exported.remove(name);
    }

    /// Marks `name` as exported. If `value` is `Some`, assigns it first.
    /// Exporting an unset name with no value is a no-op on the value map
    /// but still records the export flag (bash-compatible for later assign).
    pub fn export(&mut self, name: impl Into<String>, value: Option<String>) {
        let name = name.into();
        if let Some(value) = value {
            self.vars.insert(name.clone(), value);
        }
        self.exported.insert(name);
    }

    /// Returns whether `name` is marked for export to child processes.
    pub fn is_exported(&self, name: &str) -> bool {
        self.exported.contains(name)
    }

    /// Iterates exported name/value pairs that currently have a value.
    pub fn exported_pairs(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.exported.iter().filter_map(|name| {
            self.vars
                .get(name)
                .map(|value| (name.as_str(), value.as_str()))
        })
    }

    /// Builds the environment block for a child `Command`: only exported
    /// variables that are set.
    pub fn child_env(&self) -> Vec<(OsString, OsString)> {
        self.exported_pairs()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    /// Last command status (`$?`).
    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    /// Updates `$?`.
    pub fn set_last_status(&mut self, code: i32) {
        self.last_status = code;
    }

    /// Number of positional parameters (`$#`).
    pub fn positional_count(&self) -> usize {
        self.positionals.len()
    }

    /// Returns `$n` for `n >= 1`, or `None` if out of range.
    pub fn positional(&self, n: usize) -> Option<&str> {
        if n == 0 {
            return Some(self.shell_name.as_str());
        }
        self.positionals.get(n - 1).map(String::as_str)
    }

    /// Replaces all positional parameters (`$1`…).
    pub fn set_positionals(&mut self, args: impl IntoIterator<Item = String>) {
        self.positionals = args.into_iter().collect();
    }

    /// `shift [n]`: drops the first `n` positionals (default 1). Returns
    /// `false` if fewer than `n` positionals are present (bash: non-zero).
    pub fn shift(&mut self, n: usize) -> bool {
        if n > self.positionals.len() {
            return false;
        }
        self.positionals.drain(0..n);
        true
    }

    /// Special parameter expansion used by the expander. Returns `None`
    /// only for unknown special names (callers treat ordinary unset vars
    /// as empty via [`Self::get`]).
    pub fn expand_special(&self, name: &str) -> Option<String> {
        match name {
            "?" => Some(self.last_status.to_string()),
            "#" => Some(self.positionals.len().to_string()),
            "$" => Some(std::process::id().to_string()),
            "0" => Some(self.shell_name.clone()),
            digits if digits.chars().all(|c| c.is_ascii_digit()) => {
                let n: usize = digits.parse().ok()?;
                Some(self.positional(n).unwrap_or("").to_string())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_round_trip() {
        let mut env = ShellEnv::empty_for_test();
        env.set("FOO", "bar");
        assert_eq!(env.get("FOO"), Some("bar"));
    }

    #[test]
    fn unset_removes_value_and_export() {
        let mut env = ShellEnv::empty_for_test();
        env.export("FOO", Some("bar".into()));
        env.unset("FOO");
        assert_eq!(env.get("FOO"), None);
        assert!(!env.is_exported("FOO"));
    }

    #[test]
    fn export_marks_existing_var() {
        let mut env = ShellEnv::empty_for_test();
        env.set("FOO", "bar");
        env.export("FOO", None);
        assert!(env.is_exported("FOO"));
        assert_eq!(env.get("FOO"), Some("bar"));
    }

    #[test]
    fn child_env_only_includes_exported_set_vars() {
        let mut env = ShellEnv::empty_for_test();
        env.set("LOCAL", "1");
        env.export("EXPORTED", Some("2".into()));
        env.export("EMPTY_FLAG", None); // no value yet
        let child: HashMap<_, _> = env
            .child_env()
            .into_iter()
            .map(|(k, v)| {
                (
                    k.into_string().unwrap(),
                    v.into_string().unwrap(),
                )
            })
            .collect();
        assert_eq!(child.get("EXPORTED").map(String::as_str), Some("2"));
        assert!(!child.contains_key("LOCAL"));
        assert!(!child.contains_key("EMPTY_FLAG"));
    }

    #[test]
    fn last_status_defaults_to_zero() {
        let env = ShellEnv::empty_for_test();
        assert_eq!(env.last_status(), 0);
        assert_eq!(env.expand_special("?").as_deref(), Some("0"));
    }

    #[test]
    fn positionals_and_specials() {
        let mut env = ShellEnv::empty_for_test();
        env.shell_name = "script.sh".into();
        env.set_positionals(["a".into(), "b".into(), "c".into()]);
        assert_eq!(env.positional_count(), 3);
        assert_eq!(env.positional(0), Some("script.sh"));
        assert_eq!(env.positional(1), Some("a"));
        assert_eq!(env.positional(3), Some("c"));
        assert_eq!(env.positional(4), None);
        assert_eq!(env.expand_special("#").as_deref(), Some("3"));
        assert_eq!(env.expand_special("0").as_deref(), Some("script.sh"));
        assert_eq!(env.expand_special("2").as_deref(), Some("b"));
        assert_eq!(env.expand_special("9").as_deref(), Some(""));
    }

    #[test]
    fn shift_drops_prefix() {
        let mut env = ShellEnv::empty_for_test();
        env.set_positionals(["a".into(), "b".into(), "c".into()]);
        assert!(env.shift(1));
        assert_eq!(env.positional(1), Some("b"));
        assert!(env.shift(2));
        assert_eq!(env.positional_count(), 0);
        assert!(!env.shift(1));
    }

    #[test]
    fn dollar_special_is_pid() {
        let env = ShellEnv::empty_for_test();
        assert_eq!(
            env.expand_special("$").as_deref(),
            Some(std::process::id().to_string().as_str())
        );
    }
}
