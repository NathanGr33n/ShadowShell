# ShadowShell

A modern, batteries-included Unix shell for developers (Linux/macOS) — fish-like
editing, git/cargo aliases, project-aware colors, and a live jobs badge out of
the box.

## Quick start

```bash
# From this repo
cargo install --path . --locked

# Run
shadowshell
```

Try inside the shell:

| Try | What happens |
|-----|----------------|
| `help` | Topics and tips |
| `ll` | Alias → `ls -lah` |
| `gs` | Alias → `git status` |
| `sleep 3 &` | Job badge on the right prompt |
| Tab | Completions |
| Right arrow | Accept autosuggestion |

```bash
shadowshell --help
shadowshell --version
shadowshell -c 'echo hi'
shadowshell script.sh args...
```

### Optional: default shell

```bash
# Ensure the binary is on your PATH (cargo install uses ~/.cargo/bin)
which shadowshell

# Add to valid login shells (once, system-wide)
echo "$(which shadowshell)" | sudo tee -a /etc/shells
chsh -s "$(which shadowshell)"
```

Or try without changing login shell: run `shadowshell` from your current terminal.

## Install (developers)

```bash
cargo build --release
./target/release/shadowshell
# or
cargo install --path . --locked
```

## Configuration

On first interactive run, ShadowShell creates:

`~/.config/shadowshell/config.toml`

(from `config.toml.example` in this repo). Edit themes, colors, and personality:

```toml
theme = "onedark"          # or "nord"
history_capacity = 1000
autosuggestions = true
syntax_highlighting = true
tab_completion = true

[personality]
enabled = true

[personality.rust]
accent = [222, 163, 90]
```

Malformed config falls back to defaults with a warning. In-shell: `help config`.

## Features

### Line editing
History, Ctrl+R search, multiline input, autosuggestions, syntax highlighting,
Tab completion. See `help keys`.

### Default aliases
`ll`, `la`, git (`gs`/`ga`/`gc`/…), cargo (`cb`/`ct`/`cr`), and more.
`alias` / `unalias` to manage. See `help aliases`.

### Live job dashboard
Background (`&`) and stopped (Ctrl+Z) jobs show an ambient badge on the right
prompt; Done lines print when jobs finish (including while idle). See `help jobs`.

### Directory personality
Entering a project tree (`Cargo.toml`, `package.json`, `go.mod`, …) shifts the
prompt accent, may add temporary aliases, and prioritizes related Tab completions.
See `help personality`.

### Scripting
POSIX-style variables, control flow, functions, pipelines, redirection.
`shadowshell script.sh` or `shadowshell -c '…'`. See `help scripting`.

## Test

```bash
cargo test
```

## Platform

Linux and macOS only. Not a full bash replacement — advanced scripting may differ.
