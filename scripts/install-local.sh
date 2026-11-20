#!/usr/bin/env bash
# Local Cadri: prerequisites, engine, web UI, and hooks pointed at localhost.
#
#   curl -fsSL https://cadri.io/install-local.sh | bash
#
# From a checkout: bash scripts/install-local.sh
# That uses this tree. The curl form clones https://github.com/cadricadri/cadri
# into ~/.cadri/src (override with CADRI_SRC). CADRI_REF selects a branch or tag.
#
# Served as https://cadri.io/install-local.sh. When this file changes, copy it to
# landingpage/install-local.sh and deploy the landing page.
#
# The engine database matches ./scripts/run (engine/data/transcripts.db). This starts
# that engine itself, then runs `cadri init --local` without --start-engine.
set -euo pipefail

CADRI_HOME="${CADRI_HOME:-$HOME/.cadri}"
REPO="https://github.com/cadricadri/cadri.git"
ROOT=""
ENGINE_PID=""
WEBUI_PID=""

say() { printf '%s\n' "$*"; }
die() { printf 'cadri: %s\n' "$*" >&2; exit 1; }

# Child processes inherit stdin. Under `curl | bash` that stdin is this script,
# so every child is pointed at /dev/null.

checkout_ready() {
  [[ -f "$1/scripts/run" && -d "$1/engine" && -f "$1/plugin/install.sh" && -f "$1/webui/package.json" ]]
}

git_ready() {
  command -v git >/dev/null 2>&1 || return 1
  if [[ "$(uname -s)" == "Darwin" && "$(command -v git)" == /usr/bin/git ]]; then
    xcode-select -p >/dev/null 2>&1 || return 1
  fi
}

resolve_root() {
  local self="${BASH_SOURCE[0]:-}"
  if [[ -n "$self" && -f "$self" ]]; then
    local here
    here="$(cd "$(dirname "$self")/.." && pwd)"
    if checkout_ready "$here"; then
      say "Using this checkout: $here"
      ROOT="$here"
      return
    fi
  fi
  if [[ -n "${CADRI_SRC:-}" ]]; then
    checkout_ready "$CADRI_SRC" || die "CADRI_SRC is not a Cadri checkout: $CADRI_SRC"
    say "Using CADRI_SRC: $CADRI_SRC"
    ROOT="$CADRI_SRC"
    return
  fi
  local dest="$CADRI_HOME/src"
  if checkout_ready "$dest"; then
    say "Updating $dest"
    if ! git -C "$dest" pull --ff-only </dev/null; then
      say "Could not fast-forward $dest. Using the checkout as it is."
    fi
    ROOT="$dest"
    return
  fi
  if [[ -e "$dest" ]]; then
    die "$dest exists and is not a Cadri checkout. Move it aside or set CADRI_SRC."
  fi
  git_ready || die "git is required to clone Cadri. On a Mac, run xcode-select --install, then run this again."
  say "Cloning $REPO into $dest"
  mkdir -p "$(dirname "$dest")"
  git clone --branch "${CADRI_REF:-main}" "$REPO" "$dest" </dev/null
  ROOT="$dest"
}

node_ok() {
  local bin="$1" major
  [[ -n "$bin" && -x "$bin" ]] || return 1
  major="$("$bin" -v 2>/dev/null || true)"
  major="${major#v}"
  major="${major%%.*}"
  [[ "$major" =~ ^[0-9]+$ && "$major" -ge 20 ]]
}

ensure_node() {
  local current
  current="$(command -v node 2>/dev/null || true)"
  if node_ok "$current"; then
    return
  fi
  if node_ok "$HOME/.local/node/bin/node"; then
    export PATH="$HOME/.local/node/bin:$PATH"
    return
  fi
  local os arch target
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os-$arch" in
    Darwin-arm64) target=darwin-arm64 ;;
    Darwin-x86_64) target=darwin-x64 ;;
    Linux-x86_64) target=linux-x64 ;;
    Linux-aarch64) target=linux-arm64 ;;
    *) die "Install Node.js 20 or newer, then run this again ($os $arch)." ;;
  esac
  say "Installing Node.js..."
  local work sums name hash file
  work="$(mktemp -d)"
  sums="$(curl -fsSL https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt </dev/null)"
  name="$(printf '%s\n' "$sums" | awk -v t="$target" '$2 ~ ("^node-v[0-9.]+-" t "\\.tar\\.gz$") { print $2; exit }')"
  [[ -n "$name" ]] || die "no Node.js build for $target"
  hash="$(printf '%s\n' "$sums" | awk -v n="$name" '$2 == n { print $1; exit }')"
  file="$work/$name"
  curl -fsSL "https://nodejs.org/dist/latest-v22.x/$name" -o "$file" </dev/null
  if command -v shasum >/dev/null 2>&1; then
    printf '%s  %s\n' "$hash" "$file" | shasum -a 256 -c - >/dev/null
  elif command -v sha256sum >/dev/null 2>&1; then
    printf '%s  %s\n' "$hash" "$file" | sha256sum -c - >/dev/null
  else
    die "need shasum or sha256sum to verify Node.js"
  fi
  rm -rf "$HOME/.local/node"
  mkdir -p "$HOME/.local/node" "$HOME/.local/bin"
  tar -xzf "$file" -C "$HOME/.local/node" --strip-components=1
  rm -rf "$work"
  export PATH="$HOME/.local/node/bin:$PATH"
  node_ok "$(command -v node)" || die "Node.js did not install"
  say "Installed Node.js $(node -v) to $HOME/.local/node"
}

ensure_rust() {
  if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  if command -v cargo >/dev/null 2>&1; then
    return
  fi
  say "Installing Rust..."
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
  command -v cargo >/dev/null 2>&1 || die "Rust did not install; see https://rustup.rs"
}

ensure_linker() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    xcode-select -p >/dev/null 2>&1 || die "Rust needs the Command Line Tools. Run xcode-select --install, then run this again."
    return
  fi
  command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || die "Rust needs a C compiler. Install gcc, then run this again."
}

listening() {
  local status=0
  curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$1/" >/dev/null 2>&1 || status=$?
  [[ "$status" -ne 7 ]]
}

alive() {
  [[ "$1" =~ ^[0-9]+$ ]] && kill -0 "$1" 2>/dev/null
}

wait_http() {
  local url="$1" pid="${2:-}" label="$3" log="$4" i=0
  while [[ "$i" -lt 600 ]]; do
    if [[ -n "$pid" ]] && ! alive "$pid"; then
      die "$label exited before it was ready. Log: $log"
    fi
    if curl -fsS --max-time 2 "$url" >/dev/null 2>&1; then
      return 0
    fi
    i=$((i + 1))
    if (( i > 0 && i % 30 == 0 )); then
      say "Still waiting for ${label}..."
    fi
    sleep 1
  done
  die "$label did not become ready. Log: $log"
}

start_engine() {
  local log="$CADRI_HOME/engine.log" pidfile="$CADRI_HOME/engine.pid" old db
  mkdir -p "$CADRI_HOME"
  if curl -fsS --max-time 2 http://127.0.0.1:8080/healthz >/dev/null 2>&1; then
    say "Engine already running at http://127.0.0.1:8080"
    return
  fi
  if [[ -f "$pidfile" ]]; then
    old="$(tr -d '[:space:]' <"$pidfile")"
    if alive "$old"; then
      say "Engine is already starting (pid $old). Log: $log"
      ENGINE_PID="$old"
      return
    fi
  fi
  if listening 8080; then
    die "Port 8080 is in use, and http://127.0.0.1:8080/healthz is not responding. Stop that process, then run this again."
  fi
  say "Starting the engine. The first Rust build can take a few minutes. Log: $log"
  mkdir -p "$ROOT/engine/data"
  db="$ROOT/engine/data/transcripts.db"
  nohup bash -c '
    export PATH="$HOME/.cargo/bin:$PATH"
    cd "$1" || exit 1
    export CADRI_BIND=127.0.0.1:8080
    export CADRI_DB="$2"
    exec cargo run --bin cadri-engine
  ' bash "$ROOT/engine" "$db" >"$log" 2>&1 </dev/null &
  ENGINE_PID=$!
  printf '%s\n' "$ENGINE_PID" >"$pidfile"
}

start_webui() {
  local log="$CADRI_HOME/webui.log" pidfile="$CADRI_HOME/webui.pid" old
  mkdir -p "$CADRI_HOME"
  if curl -fsS --max-time 2 http://127.0.0.1:5173/ >/dev/null 2>&1; then
    say "Web UI already running at http://127.0.0.1:5173"
    return
  fi
  if [[ -f "$pidfile" ]]; then
    old="$(tr -d '[:space:]' <"$pidfile")"
    if alive "$old"; then
      say "Web UI is already starting (pid $old). Log: $log"
      WEBUI_PID="$old"
      return
    fi
  fi
  if listening 5173; then
    die "Port 5173 is in use. Stop that process, then run this again."
  fi
  if [[ ! -d "$ROOT/webui/node_modules" ]]; then
    say "Installing web UI dependencies."
    (cd "$ROOT/webui" && npm ci) </dev/null
  fi
  say "Starting the web UI. Log: $log"
  nohup bash -c '
    export PATH="$HOME/.local/node/bin:$HOME/.local/bin:$PATH"
    cd "$1" || exit 1
    exec npm run dev -- --strictPort
  ' bash "$ROOT/webui" >"$log" 2>&1 </dev/null &
  WEBUI_PID=$!
  printf '%s\n' "$WEBUI_PID" >"$pidfile"
}

main() {
  [[ "$(uname -s)" == "Darwin" || "$(uname -s)" == "Linux" ]] || die "Cadri supports macOS and Linux"
  export GIT_TERMINAL_PROMPT=0
  resolve_root
  ensure_node
  ensure_rust
  ensure_linker
  export PATH="$HOME/.local/node/bin:$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
  start_engine
  say "Installing the Cadri CLI from this checkout."
  bash "$ROOT/plugin/install.sh" </dev/null
  start_webui
  wait_http "http://127.0.0.1:8080/healthz" "$ENGINE_PID" "the engine" "$CADRI_HOME/engine.log"
  wait_http "http://127.0.0.1:5173/" "$WEBUI_PID" "the web UI" "$CADRI_HOME/webui.log"
  say "Pointing hooks at the local engine."
  "$HOME/.local/bin/cadri" init --local </dev/null
  say ""
  say "Cadri is running locally. There is no sign-in."
  say "Web UI: http://127.0.0.1:5173"
  say "Engine: http://127.0.0.1:8080"
  say "Hooks on this machine upload to the engine. Run \`cadri init\` without --local to point them back at hosted Cadri."
  say "Logs: $CADRI_HOME/engine.log and $CADRI_HOME/webui.log"
  say "Stop: kill the pids in $CADRI_HOME/engine.pid and $CADRI_HOME/webui.pid"
}

main "$@"
