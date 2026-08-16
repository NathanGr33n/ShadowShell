# ShadowShell

A modern, batteries-included Unix shell for developers (Linux/macOS) — fish-like
editing, git/cargo aliases, project-aware colors, and a live jobs badge out of
the box.

## Quick start

```bash
# From this repo (installs to ~/.local/bin by default)
./install.sh
# or: cargo install --path . --locked   # → ~/.cargo/bin

# If the install dir is not on PATH yet:
export PATH="$HOME/.local/bin:$PATH"   # install.sh default
# export PATH="$HOME/.cargo/bin:$PATH" # cargo install

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
| `which gs` | Resolve alias / builtin / PATH |

```bash
shadowshell --help
shadowshell --version
shadowshell --welcome
shadowshell --theme nord          # session-only theme preview
shadowshell -c 'echo hi'
shadowshell script.sh args...
```

### Optional: default shell

```bash
# Ensure the binary is on your PATH (~/.local/bin or ~/.cargo/bin)
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

### Prebuilt releases

Pushing a `v*` tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml),
which builds Linux (`x86_64`) and macOS (`x86_64`, `aarch64`) archives and attaches
them to a GitHub Release with SHA-256 checksums.

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

Preview a built-in theme without editing the file:

```bash
shadowshell --theme nord
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

## Docs

- [User guide](docs/user-guide.md) — install, keys, jobs, config, personality
- In-shell: `help`, `help <topic>`
- Example config: [`config.toml.example`](config.toml.example)

## License

MIT — see [LICENSE](LICENSE).

## Platform

Linux and macOS only. Not a full bash replacement — advanced scripting may differ.
