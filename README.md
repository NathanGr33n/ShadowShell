# ShadowShell

A modern Unix shell built from scratch in Rust.

## Status

Phase 6 (polish): history-based autosuggestions, command-line syntax
highlighting, tab completion (files/dirs and `$PATH` executables), and a TOML
config/theme system (`~/.config/shadowshell/config.toml`) with built-in
`onedark` and `nord` themes. A live job dashboard in the prompt shows an
ambient spinner/badge for background and stopped jobs. Earlier phases cover
the core loop, line editing, job control/pipelines, the animated prompt, and
POSIX-style scripting.

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

## Live job dashboard

When jobs are backgrounded (`cmd &`) or stopped (Ctrl+Z), the right side of
the prompt shows an ambient indicator that updates while you type or idle:

- one running job — spinner + truncated command (`⠋ sleep 30`)
- multiple jobs — counts (`⠋×2 ⏸×1`)
- stopped job — pause badge (`⏸ [1] vim`)

Finished background jobs still print the usual `[n]+ Done …` line (including
while the line editor is waiting for input).

## Default aliases

Developer-oriented shortcuts are enabled out of the box, including:

| Alias | Expands to |
|-------|------------|
| `ll` | `ls -lah` |
| `la` | `ls -A` |
| `..` | `cd ..` |
| `gs` / `ga` / `gc` / `gp` / `gl` | git status/add/commit/push/pull |
| `cb` / `ct` / `cr` | cargo build/test/run |
| `glog` | `git log --oneline --graph --decorate` |

Manage them interactively:

```
alias                  # list all
alias foo='echo hi'    # define / override
unalias foo            # remove one
unalias -a             # clear all
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
