#!/usr/bin/env bash
# Install ShadowShell for the current user.
# Usage:
#   curl -fsSL …/install.sh | bash          # if hosted
#   ./install.sh                            # from a git checkout
#   ./install.sh --prefix ~/.local
set -u

PREFIX="${PREFIX:-$HOME/.local}"
REPO_URL="${SHADOWSHELL_REPO:-https://github.com/NathanGr33n/ShadowShell}"

info()  { printf '==> %s\n' "$*"; }
warn()  { printf 'warning: %s\n' "$*" >&2; }
die()   { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="${2:-}"; shift 2 || die "--prefix needs a path" ;;
    -h|--help)
      cat <<EOF
install.sh — install ShadowShell for the current user

Options:
  --prefix DIR   Install prefix (default: ~/.local)
  -h, --help     Show this help

Requires: rustc/cargo (https://rustup.rs), git (for remote install).
Binary installed to: \$PREFIX/bin/shadowshell
EOF
      exit 0
      ;;
    *) die "unknown option: $1" ;;
  esac
done

BIN_DIR="$PREFIX/bin"
mkdir -p "$BIN_DIR" || die "could not create $BIN_DIR"

if ! command -v cargo >/dev/null 2>&1; then
  die "cargo not found. Install Rust from https://rustup.rs then re-run."
fi

# Prefer installing from the current checkout when this script lives in a repo.
SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd || true)"
if [ -n "${SCRIPT_DIR:-}" ] && [ -f "$SCRIPT_DIR/Cargo.toml" ]; then
  info "Building from local checkout: $SCRIPT_DIR"
  (
    cd "$SCRIPT_DIR" || exit 1
    cargo install --path . --locked --root "$PREFIX" --force
  ) || die "cargo install failed"
else
  command -v git >/dev/null 2>&1 || die "git not found (needed for remote install)"
  TMP="$(mktemp -d "${TMPDIR:-/tmp}/shadowshell-install.XXXXXX")"
  cleanup() { rm -rf "$TMP"; }
  trap cleanup EXIT
  info "Cloning $REPO_URL"
  git clone --depth 1 "$REPO_URL" "$TMP/src" || die "git clone failed"
  info "Building release binary"
  (
    cd "$TMP/src" || exit 1
    cargo install --path . --locked --root "$PREFIX" --force
  ) || die "cargo install failed"
fi

BINARY="$BIN_DIR/shadowshell"
[ -x "$BINARY" ] || die "expected executable at $BINARY"

info "Installed: $BINARY"
"$BINARY" --version || true

case ":$PATH:" in
  *":$BIN_DIR:"*)
    info "$BIN_DIR is already on PATH"
    ;;
  *)
    warn "$BIN_DIR is not on your PATH"
    cat <<EOF >&2

Add one of these to your shell rc (~/.bashrc, ~/.zshrc, …), then open a new terminal:
  export PATH="$BIN_DIR:\$PATH"

Until then, run the full path:
  $BINARY
EOF
    ;;
esac

cat <<EOF

Next steps:
  shadowshell                 # start the shell (after PATH)
  shadowshell --help
  shadowshell --welcome       # tips banner
  shadowshell --theme nord    # try a theme for this session

Prebuilt binaries (optional): GitHub Releases on v* tags.

Optional — set as login shell (after PATH works):
  grep -qxF "$BINARY" /etc/shells || echo "$BINARY" | sudo tee -a /etc/shells
  chsh -s "$BINARY"
EOF
