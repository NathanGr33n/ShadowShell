# ShadowShell

A modern Unix shell built from scratch in Rust.

## Status

Phase 6 (polish): history-based autosuggestions, command-line syntax
highlighting, tab completion (files/dirs and `$PATH` executables), and a TOML
config/theme system (`~/.config/shadowshell/config.toml`) with built-in
`onedark` and `nord` themes. Earlier phases cover the core loop, line editing,
job control/pipelines, the animated prompt, and POSIX-style scripting.

## Build

```
cargo build
```

## Run

Interactive:

```
cargo run
```

Script:

```
cargo run -- path/to/script.sh arg1 arg2
```

## Config

Optional file: `~/.config/shadowshell/config.toml`

```toml
theme = "onedark"          # or "nord"
history_capacity = 1000
autosuggestions = true
syntax_highlighting = true
tab_completion = true

# Optional per-color RGB overrides:
# [colors]
# cwd = [97, 175, 239]
# success = [152, 195, 121]
# failure = [224, 108, 117]
```

Malformed config falls back to defaults with a warning.

## Test

```
cargo test
```
