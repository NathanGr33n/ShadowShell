# ShadowShell

A modern, batteries-included Unix shell for developers on **Linux** and **macOS**.

Fish-like line editing, sensible git/cargo aliases, project-aware colors, and a live
jobs badge — without giving up familiar shell scripting.

**Repository:** [github.com/NathanGr33n/ShadowShell](https://github.com/NathanGr33n/ShadowShell)

## Why ShadowShell?

| Built in | What you get |
|----------|----------------|
| Line editing | History, Ctrl+R, multiline, autosuggestions, syntax highlighting, Tab completion |
| Defaults | `ll`, git (`gs`/`ga`/`gc`/…), cargo (`cb`/`ct`/`cr`), and more |
| Live jobs | Background (`&`) / stopped (Ctrl+Z) badge on the right prompt; Done notices while idle |
| Personality | Detects `Cargo.toml`, `package.json`, `go.mod`, … and adapts accents, aliases, completions |
| Scripting | Variables, control flow, functions, pipes, redirection — `script.sh` or `-c` |
| Onboarding | First-run welcome, `help` topics, `which` / `type`, did-you-mean |

Not a full bash clone — advanced bash-isms may differ. For day-to-day dev use, it aims to feel ready out of the box.

## Quick start

### From a git checkout

```bash
git clone https://github.com/NathanGr33n/ShadowShell.git
cd ShadowShell
./install.sh
# default install: ~/.local/bin/shadowshell
```

If `~/.local/bin` is not on your `PATH` yet:

```bash
export PATH="$HOME/.local/bin:$PATH"
# add that line to ~/.bashrc or ~/.zshrc for permanence
```

Then:

```bash
shadowshell
```

### Other install options

```bash
# Cargo (binary → ~/.cargo/bin)
cargo install --path . --locked
export PATH="$HOME/.cargo/bin:$PATH"

# Build and run in-tree
cargo build --release
./target/release/shadowshell

# Prebuilt archives (when a v* release exists)
# GitHub → Releases → Linux x86_64 or macOS x86_64 / aarch64 (.tar.gz + .sha256)
```

Requires a recent Rust toolchain for source installs ([rustup](https://rustup.rs)).

## Try it

Inside the shell:

| Command / key | What happens |
|---------------|----------------|
| `help` | Topic index |
| `ll` | Alias → `ls -lah` |
| `gs` | Alias → `git status` |
| `sleep 3 &` | Live job badge on the right prompt |
| Tab | Completions (files, commands, aliases) |
| Right arrow | Accept autosuggestion (at end of line) |
| `which gs` | Resolve alias / builtin / PATH |
| `type gs` | Same idea, slightly more detail |

From your current terminal:

```bash
shadowshell --help
shadowshell --version
shadowshell --welcome              # tips banner (also shown on first run)
shadowshell --theme nord           # session-only theme (onedark | nord)
shadowshell -c 'echo hi'
shadowshell script.sh args...
```

### Optional: login shell

```bash
# Confirm the binary is on PATH
command -v shadowshell

BIN="$(command -v shadowshell)"
grep -qxF "$BIN" /etc/shells || echo "$BIN" | sudo tee -a /etc/shells
chsh -s "$BIN"
```

Or just run `shadowshell` from your existing terminal — no login-shell change required.

## Configuration

On first interactive run, ShadowShell writes:

`~/.config/shadowshell/config.toml`

(from [`config.toml.example`](config.toml.example)). Example:

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

Preview a theme without editing the file:

```bash
shadowshell --theme nord
```

Malformed config falls back to defaults with a warning. In-shell: `help config`.

## Features

### Line editing
History, Ctrl+R search, multiline input, autosuggestions, syntax highlighting,
and Tab completion (reedline). See `help keys`.

### Default aliases
`ll`, `la`, git (`gs`/`ga`/`gc`/…), cargo (`cb`/`ct`/`cr`), directory shortcuts, and more.
Manage with `alias` / `unalias`. See `help aliases`.

### Live job dashboard
Background (`&`) and stopped (Ctrl+Z) jobs show an ambient badge on the right
prompt. Finished jobs print Done lines (including while the prompt is idle).
See `help jobs`.

### Directory personality
Entering a project tree (`Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`,
`build.zig`, `CMakeLists.txt`, …) can shift the prompt accent, inject temporary
aliases, and prioritize related Tab completions. Your own aliases are never
overwritten. See `help personality`.

### Scripting
POSIX-style variables, `export`, `if` / `while` / `for` / `case`, functions,
pipelines, redirection, `&&` / `||`, and `$(…)`.

```bash
shadowshell script.sh arg1
shadowshell -c 'x=1; echo $x'
```

See `help scripting`.

### Finding commands
Unknown commands may get did-you-mean suggestions. Use `which` / `type` to resolve
names. See `help finding`.

## Development

```bash
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo build --release --locked
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy (`-D warnings`), tests, release
build, and CLI smoke checks on Ubuntu and macOS for pushes/PRs to `Master` / `main`.

### Releases

Push a version tag to publish prebuilt binaries:

```bash
git tag v0.1.0
git push origin v0.1.0
```

[`.github/workflows/release.yml`](.github/workflows/release.yml) builds:

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

and attaches `.tar.gz` archives plus SHA-256 checksums to the GitHub Release.

## Docs

- [User guide](docs/user-guide.md) — install, keys, jobs, config, personality
- In-shell: `help`, `help <topic>` (`keys`, `aliases`, `jobs`, `config`, …)
- Example config: [`config.toml.example`](config.toml.example)
- Install script: [`install.sh`](install.sh)

## Platform

- **Supported:** Linux, macOS
- **Not supported:** Windows (native)
- **License:** MIT — see [LICENSE](LICENSE)

## Status

ShadowShell is under active development (v0.1.x). Feedback and issues are welcome
on [GitHub](https://github.com/NathanGr33n/ShadowShell).
