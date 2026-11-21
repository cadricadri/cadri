#!/usr/bin/env bash
# Install the Cadri CLI: uv, a uv-managed Python, and the plugin tree at ~/.cadri/install.
# ~/.cadri/install is a symlink to ~/.cadri/versions/<version>.
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

version_name() {
  local file="$1/VERSION" version="source"
  if [[ -f "$file" ]]; then
    version="$(tr -cd 'A-Za-z0-9._-' <"$file")"
    [[ -n "$version" ]] || version="source"
  fi
  printf '%s\n' "$version"
}

# ~/.cadri/install points at ~/.cadri/versions/<version>. Reinstalling the same
# version keeps that directory's virtualenv. An older layout, where install was
# a real directory, is moved aside first.
place_tree() {
  local src="$1" version dest link previous=""
  version="$(version_name "$src")"
  dest="$CADRI_HOME/versions/$version"
  link="$CADRI_HOME/install"
  mkdir -p "$CADRI_HOME/versions"
  if [[ -L "$link" ]]; then
    previous="$(readlink "$link")"
  elif [[ -d "$link" ]]; then
    local old parked
    old="$(version_name "$link")"
    parked="$CADRI_HOME/versions/$old"
    if [[ "$old" == "$version" && ! -d "$dest" ]]; then
      mv "$link" "$dest"
    elif [[ ! -e "$parked" ]]; then
      mv "$link" "$parked"
      previous="$parked"
    else
      rm -rf "$link"
    fi
  fi
  mkdir -p "$dest"
  find "$dest" -mindepth 1 -maxdepth 1 ! -name .venv -exec rm -rf {} +
  (cd "$src" && tar -cf - --exclude .venv --exclude __pycache__ --exclude .pytest_cache .) | (cd "$dest" && tar -xf -)
  ln -sfn "$dest" "$link"
  local dir
  for dir in "$CADRI_HOME/versions"/*; do
    [[ -d "$dir" ]] || continue
    [[ "$dir" == "$dest" || "$dir" == "$previous" ]] && continue
    rm -rf "$dir"
  done
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
  place_tree "$src"
  say "Setting up Python ${PYTHON_VERSION}…"
  UV_PYTHON_PREFERENCE=only-managed uv_quiet sync --quiet --project "$INSTALL_DIR" --python "$PYTHON_VERSION"
  write_shim
  ensure_path
  local version
  version="$(cat "$INSTALL_DIR/VERSION" 2>/dev/null || echo source)"
  say ""
  say "Cadri $version installed to $INSTALL_DIR."
  # A machine that already ran `cadri init` gets its harness copies refreshed.
  # The bootstrap has no virtualenv python yet when uv is stubbed, and a fresh
  # machine has nothing to refresh: both still end on `cadri init`.
  if [[ -x "$INSTALL_DIR/.venv/bin/python" ]]; then
    local applied
    applied="$("$CADRI_HOME/bin/cadri" update --apply 2>/dev/null || true)"
    if [[ -n "${applied// /}" ]]; then
      printf '%s\n' "$applied"
      return
    fi
  fi
  say "Next: $BIN_DIR/cadri init"
}

main "$@"
