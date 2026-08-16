# ShadowShell user guide

A short guide for day-to-day use. For a one-page overview see the [README](../README.md).
In the shell, `help` and `help <topic>` cover the same ground interactively.

## Install

**From a git checkout (recommended while developing):**

```bash
./install.sh
# or
cargo install --path . --locked
```

Ensure `~/.cargo/bin` or `~/.local/bin` is on your `PATH`.

**Run without installing:**

```bash
cargo run --release
```

## Starting the shell

```bash
shadowshell                 # interactive
shadowshell --welcome       # tips banner
shadowshell -c 'echo hi'    # one-shot command
shadowshell script.sh a b   # run a script ($0=script, $1=a, $2=b)
```

### Try it from your current shell

Just run `shadowshell`. Exit with `exit` or Ctrl+D on an empty line.

### Set as login shell (optional)

```bash
BIN="$(command -v shadowshell)"
grep -qxF "$BIN" /etc/shells || echo "$BIN" | sudo tee -a /etc/shells
chsh -s "$BIN"
```

Log out and back in. Keep a second terminal open until you confirm it works.

## Line editing

| Key | Action |
|-----|--------|
| ↑ / ↓ | History |
| Ctrl+R | Reverse history search |
| Tab | Complete files, commands, aliases |
| Shift+Tab | Previous completion |
| → (at EOL) | Accept autosuggestion |
| Ctrl+C | Cancel line |
| Ctrl+D | EOF / exit on empty line |
| `\` at end of line | Continue on next line |

See also: `help keys`.

## Aliases

```text
alias                 # list
alias gco=git checkout
unalias gco
unalias -a            # clear all (including defaults for this session)
```

Built-in defaults include `ll`, `la`, git (`gs`, `ga`, `gc`, …), and cargo (`cb`, `ct`, `cr`).
Full list: `alias` or `help aliases`.

## Jobs

```text
sleep 60 &            # background
jobs
fg                    # foreground latest
bg %1                 # resume job 1 in background
```

Ctrl+Z stops the foreground job. The **right prompt** shows a live badge while
jobs run or are stopped; finished jobs print `[n]+ Done …` (also while idle).

See: `help jobs`.

## Configuration

File: `~/.config/shadowshell/config.toml`  
Created on first interactive run from `config.toml.example`.

```toml
theme = "onedark"       # or "nord"
history_capacity = 1000
autosuggestions = true
syntax_highlighting = true
tab_completion = true

[personality]
enabled = true

[personality.rust]
accent = [222, 163, 90]
```

Malformed files fall back to defaults with a warning. See `help config`.

## Directory personality

Entering a tree that contains a project marker (searched upward) lightly adapts
the shell:

| Marker | Kind | Badge |
|--------|------|-------|
| `Cargo.toml` | Rust | `rs` |
| `package.json` | Node | `js` |
| `pyproject.toml` / `requirements.txt` | Python | `py` |
| `go.mod` | Go | `go` |
| `build.zig` | Zig | `zig` |
| `CMakeLists.txt` | CMake | `c` |

Effects: prompt accent, optional badge, temporary aliases, Tab priority.  
Your own `alias` names are never overwritten. Leaving the tree reverts injections.

See: `help personality`.

## Scripting

Supports variables, `export`, `if` / `while` / `for` / `case`, functions,
pipelines, redirection, `&&` / `||`, and `$(…)`.

```bash
shadowshell -c 'x=1; echo $x'
shadowshell ./tool.sh arg1
```

Not a full bash clone. See `help scripting`.

## Finding commands

If a command is missing, ShadowShell may print `did you mean …?`.

```text
which ls              # resolve a name (builtin, alias, or PATH)
type gs               # same idea, slightly more detail
```

## Getting help

```text
help
help keys | aliases | jobs | config | personality | scripting
shadowshell --help
```

## Platform

Linux and macOS only.
