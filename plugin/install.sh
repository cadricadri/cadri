#!/usr/bin/env bash
# Install the Cadri CLI: uv, a uv-managed Python, and the plugin tree in ~/.cadri/install.
#
#   curl -fsSL https://cadri.io/install.sh | bash
#
# Installs the newest GitHub release by default. CADRI_REF=<branch, tag or commit>
# installs that source instead, and CADRI_PLUGIN_TGZ=<url> a specific tarball.
# Run from a checkout (bash plugin/install.sh) to install that tree.
set -euo pipefail

CADRI_HOME="${CADRI_HOME:-$HOME/.cadri}"
INSTALL_DIR="$CADRI_HOME/install"
BIN_DIR="$HOME/.local/bin"
REPO="cadricadri/cadri"
RELEASE_URL="https://github.com/$REPO/releases/latest/download/cadri-plugin.tar.gz"
PYTHON_VERSION="3.12"
USER_PATH="$PATH"

say() { printf '%s\n' "$*"; }
die() { printf 'cadri: %s\n' "$*" >&2; exit 1; }

ensure_uv() {
  if command -v uv >/dev/null 2>&1; then
    return
  fi
  if [[ -x "$HOME/.local/bin/uv" ]]; then
    export PATH="$HOME/.local/bin:$PATH"
    return
  fi
  say "Installing uv…"
  curl -LsSf https://astral.sh/uv/install.sh | env UV_NO_MODIFY_PATH=1 sh >/dev/null
  export PATH="$HOME/.local/bin:$PATH"
  command -v uv >/dev/null 2>&1 || die "uv did not install; see https://docs.astral.sh/uv/"
}

# Prints the directory holding the plugin tree to install.
fetch_source() {
  local self="${BASH_SOURCE[0]:-}"
  if [[ -n "$self" && -f "$self" && "$(basename "$self")" == "install.sh" ]]; then
    local here
    here="$(cd "$(dirname "$self")" && pwd)"
    if [[ -f "$here/pyproject.toml" && -f "$here/cadri/cli.py" ]]; then
      say "Installing from this checkout: $here" >&2
      printf '%s\n' "$here"
      return
    fi
  fi
  local url="${CADRI_PLUGIN_TGZ:-$RELEASE_URL}"
  if [[ -n "${CADRI_REF:-}" ]]; then
    url="https://codeload.github.com/$REPO/tar.gz/$CADRI_REF"
  fi
  say "Downloading $url" >&2
  curl -fsSL "$url" | tar -xz -C "$WORK" || die "download failed: $url"
  local cli
  cli="$(find "$WORK" -maxdepth 4 -path '*/bin/cadri_cli.py' | head -1)"
  [[ -n "$cli" ]] || die "no plugin tree in $url"
  dirname "$(dirname "$cli")"
}

# Without the Command Line Tools, /usr/bin/python3 is a stub that opens their installer,
# and uv runs every python3 on PATH while it looks for interpreters.
uv_quiet() {
  if [[ "$(uname -s)" != "Darwin" ]] || xcode-select -p >/dev/null 2>&1; then
    uv "$@"
    return
  fi
  local dir path=""
  local IFS=:
  for dir in $PATH; do
    [[ "$dir" == /usr/bin ]] || path="${path:+$path:}$dir"
  done
  PATH="$path" uv "$@"
}

copy_tree() {
  local src="$1"
  mkdir -p "$INSTALL_DIR"
  # Keep the existing virtualenv so upgrades only re-sync it.
  find "$INSTALL_DIR" -mindepth 1 -maxdepth 1 ! -name .venv -exec rm -rf {} +
  (cd "$src" && tar -cf - --exclude .venv --exclude __pycache__ --exclude .pytest_cache .) | (cd "$INSTALL_DIR" && tar -xf -)
}

# Agent hooks run $CADRI_HOME/bin/cadri by absolute path; ~/.local/bin/cadri points at it.
write_shim() {
  local shim="$CADRI_HOME/bin/cadri"
  mkdir -p "$CADRI_HOME/bin" "$BIN_DIR"
  cat >"$shim" <<EOF
#!/bin/sh
exec "$INSTALL_DIR/.venv/bin/python" "$INSTALL_DIR/bin/cadri_cli.py" "\$@"
EOF
  chmod +x "$shim"
  ln -sf "$shim" "$BIN_DIR/cadri"
}

ensure_path() {
  case ":$USER_PATH:" in
    *":$BIN_DIR:"*) return ;;
  esac
  local rc="$HOME/.zshrc"
  [[ "${SHELL:-}" == */bash ]] && rc="$HOME/.bash_profile"
  if ! grep -qs '\.local/bin' "$rc"; then
    printf '\n# Added by the Cadri installer\nexport PATH="$HOME/.local/bin:$PATH"\n' >>"$rc"
    say "Added ~/.local/bin to PATH in $rc (new terminals pick it up)."
  fi
}

main() {
  [[ "$(uname -s)" == "Darwin" || "$(uname -s)" == "Linux" ]] || die "Cadri supports macOS and Linux"
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  ensure_uv
  local src
  src="$(fetch_source)"
  copy_tree "$src"
  say "Setting up Python ${PYTHON_VERSION}…"
  UV_PYTHON_PREFERENCE=only-managed uv_quiet sync --quiet --project "$INSTALL_DIR" --python "$PYTHON_VERSION"
  write_shim
  ensure_path
  local version
  version="$(cat "$INSTALL_DIR/VERSION" 2>/dev/null || echo source)"
  say ""
  say "Cadri $version installed to $INSTALL_DIR."
  say "Next: $BIN_DIR/cadri init"
}

main "$@"
