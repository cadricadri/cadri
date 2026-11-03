#!/usr/bin/env bash
# Hosted installer: ensure uv, sync this plugin tree into ~/.cadri, put `cadri` on PATH.
set -euo pipefail

CADRI_HOME="${CADRI_HOME:-$HOME/.cadri}"
INSTALL_DIR="${CADRI_HOME}/install"
BIN_DIR="${HOME}/.local/bin"
REPO_DEFAULT="https://github.com/cadricadri/cadri"
# When run from a checkout, install that tree; otherwise clone/download.

ensure_uv() {
  if command -v uv >/dev/null 2>&1; then
    return 0
  fi
  echo "Installing uv…"
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="${HOME}/.local/bin:${PATH}"
}

copy_tree() {
  local src="$1"
  mkdir -p "$INSTALL_DIR"
  rsync -a --delete --exclude '.venv' --exclude '__pycache__' --exclude '.pytest_cache' "$src/" "$INSTALL_DIR/"
}

main() {
  ensure_uv
  mkdir -p "$BIN_DIR" "$CADRI_HOME"

  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
  if [[ -f "$SCRIPT_DIR/pyproject.toml" && -d "$SCRIPT_DIR/shared" ]]; then
    echo "Installing from local checkout: $SCRIPT_DIR"
    copy_tree "$SCRIPT_DIR"
  elif [[ -n "${CADRI_PLUGIN_TGZ:-}" ]]; then
    echo "Installing from $CADRI_PLUGIN_TGZ"
    tmp="$(mktemp -d)"
    curl -fsSL "$CADRI_PLUGIN_TGZ" | tar -xz -C "$tmp"
    # tarball may contain a top-level directory
    inner="$(find "$tmp" -maxdepth 2 -name pyproject.toml | head -1 | xargs dirname)"
    copy_tree "$inner"
  else
    echo "No local plugin tree; cloning ${REPO_DEFAULT} (plugin/)…"
    tmp="$(mktemp -d)"
    git clone --depth 1 "$REPO_DEFAULT" "$tmp/repo"
    if [[ -d "$tmp/repo/plugin" ]]; then
      copy_tree "$tmp/repo/plugin"
    elif [[ -d "$tmp/repo/cadri/plugin" ]]; then
      copy_tree "$tmp/repo/cadri/plugin"
    else
      echo "Could not find plugin/ in repository" >&2
      exit 1
    fi
  fi

  cd "$INSTALL_DIR"
  uv sync --quiet
  cat >"$BIN_DIR/cadri" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export PATH="\${HOME}/.local/bin:\${PATH}"
cd "$INSTALL_DIR"
exec uv run --quiet python bin/cadri_cli.py "\$@"
EOF
  chmod +x "$BIN_DIR/cadri" "$INSTALL_DIR/bin/cadri" "$INSTALL_DIR/bin/cadri_cli.py" 2>/dev/null || true

  echo
  echo "Cadri installed. Ensure $BIN_DIR is on your PATH, then run:"
  echo "  cadri init"
  echo
  echo "Hosted default: CADRI_URL=https://app.cadri.io"
  echo "Localhost:      CADRI_URL=http://127.0.0.1:8080 cadri init --local"
}

main "$@"
