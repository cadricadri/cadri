"""Install a newer Cadri release and refresh the copies each harness already has.

`~/.cadri/install` is a symlink to `~/.cadri/versions/<version>`. The shim at
`~/.cadri/bin/cadri` points at that symlink, so flipping it moves every hook onto
the new code. Skills, hook manifests, and keeper agents are copies made by
`cadri init`; `--apply` rewrites those and nothing else.

Clients check GitHub's latest plugin release once a day and only move forward.
A source checkout is left alone.
"""

from __future__ import annotations

import contextlib
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
from pathlib import Path

from .install import bundles
from .state import LockTimeout, cadri_home, locked, write_text

VERSION_URL = "https://github.com/cadricadri/cadri/releases/latest/download/VERSION"
TARBALL_URL = "https://github.com/cadricadri/cadri/releases/latest/download/cadri-plugin.tar.gz"
CHECK_INTERVAL = 24 * 60 * 60
HOLDING = "CADRI_UPDATE_HOLDING_LOCK"
PYTHON_VERSION = "3.12"


class UpdateError(Exception):
    pass


def plugin_root() -> Path:
    return Path(__file__).resolve().parents[1]


def local_version() -> str:
    try:
        text = (plugin_root() / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        return "source"
    return text or "source"


def parsed(value: str) -> tuple[int, int, int] | None:
    parts = value.strip().split(".")
    if len(parts) != 3:
        return None
    numbers = []
    for part in parts:
        if not part.isdigit():
            return None
        numbers.append(int(part))
    return numbers[0], numbers[1], numbers[2]


def should_upgrade(local: str, remote: str) -> bool:
    """A newer semver release. Source installs and older releases stay put."""
    remote_version = parsed(remote)
    local_version = parsed(local)
    if remote_version is None or local_version is None:
        return False
    return remote_version > local_version


def tracks_releases() -> bool:
    """True when the shim runs ~/.cadri/install. A checkout shim is not updated."""
    path = bundles.shim_path()
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return False
    install = str(cadri_home() / "install")
    return f"{install}/.venv/bin/python" in text and f"{install}/bin/cadri_cli.py" in text


def checked_path() -> Path:
    return cadri_home() / "state" / "update-checked"


def stamp_path() -> Path:
    return cadri_home() / "state" / "bundle-version"


def notice_path() -> Path:
    return cadri_home() / "state" / "update-notice"


def check_due() -> bool:
    try:
        checked = float(checked_path().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return True
    return time.time() - checked >= CHECK_INTERVAL


def mark_checked() -> None:
    write_text(checked_path(), f"{time.time()}\n")


def record_bundle_version() -> None:
    write_text(stamp_path(), local_version() + "\n")


def bundles_stale() -> bool:
    current = local_version()
    if parsed(current) is None or not bundles.cadri_installed():
        return False
    try:
        stamped = stamp_path().read_text(encoding="utf-8").strip()
    except OSError:
        return True
    return stamped != current


def take_notice() -> str:
    path = notice_path()
    claimed = path.with_name(f".update-notice-{os.getpid()}")
    try:
        os.replace(path, claimed)
    except OSError:
        return ""
    try:
        return claimed.read_text(encoding="utf-8").strip()
    except OSError:
        return ""
    finally:
        claimed.unlink(missing_ok=True)


@contextlib.contextmanager
def hold():
    """One update at a time. A child `cadri update --apply` inherits the lock."""
    if os.environ.get(HOLDING) == "1":
        yield
        return
    with locked(cadri_home() / "update.lock", timeout=0):
        os.environ[HOLDING] = "1"
        try:
            yield
        finally:
            os.environ.pop(HOLDING, None)


def fetch_text(url: str, timeout: float) -> str:
    if not url.startswith("https://"):
        raise UpdateError(f"refusing {url}")
    request = urllib.request.Request(url)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.read().decode().strip()
    except (OSError, ValueError) as exc:
        raise UpdateError(f"could not reach {url}: {exc}") from exc


def download(url: str, dest: Path, timeout: float) -> None:
    if not url.startswith("https://"):
        raise UpdateError(f"refusing {url}")
    request = urllib.request.Request(url)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response, open(dest, "wb") as handle:
            shutil.copyfileobj(response, handle)
    except (OSError, ValueError) as exc:
        raise UpdateError(f"could not download {url}: {exc}") from exc


def latest_version() -> str:
    text = fetch_text(VERSION_URL, 15)
    if parsed(text) is None:
        raise UpdateError(f"latest release version is {text!r}")
    return text


def uv_env() -> dict[str, str]:
    env = os.environ.copy()
    env["UV_PYTHON_PREFERENCE"] = "only-managed"
    local = str(Path.home() / ".local" / "bin")
    env["PATH"] = local + os.pathsep + env.get("PATH", "")
    if sys.platform == "darwin":
        try:
            subprocess.run(["xcode-select", "-p"], capture_output=True, check=True)
        except (OSError, subprocess.CalledProcessError):
            env["PATH"] = os.pathsep.join(part for part in env["PATH"].split(os.pathsep) if part != "/usr/bin")
    return env


def find_uv() -> str:
    for directory in uv_env()["PATH"].split(os.pathsep):
        candidate = Path(directory) / "uv"
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    raise UpdateError("uv is not installed; re-run the Cadri installer")


def sync_project(dest: Path) -> None:
    result = subprocess.run(
        [find_uv(), "sync", "--quiet", "--project", str(dest), "--python", PYTHON_VERSION],
        env=uv_env(),
        capture_output=True,
        text=True,
        timeout=600,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise UpdateError(detail or "uv sync failed")


def extract(tarball: Path, dest: Path) -> None:
    """Unpack the plugin tree from a release tarball. Skip anything outside that tree."""
    with tarfile.open(tarball, "r:gz") as tar:
        clis = [member.name for member in tar.getmembers() if member.name.endswith("bin/cadri_cli.py")]
        if not clis:
            raise UpdateError("release tarball has no Cadri CLI")
        prefix = min(clis, key=len)[: -len("bin/cadri_cli.py")]
        dest.mkdir(parents=True, exist_ok=True)
        root = dest.resolve()
        for member in tar.getmembers():
            if not member.name.startswith(prefix) or not (member.isfile() or member.isdir()):
                continue
            relative = member.name[len(prefix) :]
            if not relative or relative == ".venv" or relative.startswith(".venv/"):
                continue
            if Path(relative).is_absolute() or ".." in Path(relative).parts:
                continue
            target = (dest / relative).resolve()
            if os.path.commonpath([root, target]) != str(root):
                continue
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            source = tar.extractfile(member)
            if source is None:
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with source, open(target, "wb") as handle:
                shutil.copyfileobj(source, handle)
            mode = member.mode & 0o777
            if mode:
                target.chmod(mode)


def stage(version: str) -> Path:
    if parsed(version) is None:
        raise UpdateError(f"refusing version {version!r}")
    root = cadri_home() / "versions"
    root.mkdir(parents=True, exist_ok=True)
    dest = root / version
    if dest.exists():
        shutil.rmtree(dest)
    work = Path(tempfile.mkdtemp(prefix=".update-", dir=root))
    try:
        tarball = work / "cadri-plugin.tar.gz"
        download(TARBALL_URL, tarball, 120)
        extract(tarball, dest)
        if not (dest / "bin" / "cadri_cli.py").is_file():
            raise UpdateError("release tarball has no Cadri CLI")
        try:
            stamped = (dest / "VERSION").read_text(encoding="utf-8").strip()
        except OSError:
            stamped = ""
        if stamped != version:
            raise UpdateError(f"release tarball is {stamped or 'unversioned'}, not {version}")
        sync_project(dest)
    except Exception:
        shutil.rmtree(dest, ignore_errors=True)
        raise
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return dest


def version_in(tree: Path) -> str:
    try:
        text = (tree / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        return "previous"
    return text or "previous"


def current_target() -> Path | None:
    link = cadri_home() / "install"
    if link.is_symlink():
        return Path(os.path.realpath(link))
    return None


def flip(dest: Path) -> Path | None:
    """Point ~/.cadri/install at `dest`. A pre-symlink install is moved aside and returned."""
    home = cadri_home()
    link = home / "install"
    parked = None
    if link.is_dir() and not link.is_symlink():
        old = version_in(link)
        name = old if parsed(old) else "previous"
        parked = home / "versions" / name
        if parked.exists():
            parked = home / "versions" / f"{name}-previous"
        if parked.resolve() == dest.resolve():
            raise UpdateError("refusing to replace the tree being installed")
        link.rename(parked)
    temporary = link.with_name(f".install-{os.getpid()}")
    if temporary.is_symlink() or temporary.exists():
        temporary.unlink()
    temporary.symlink_to(dest)
    os.replace(temporary, link)
    return parked


def prune(keep: Path, previous: Path | None) -> None:
    root = cadri_home() / "versions"
    keep_ids = {keep.resolve()}
    if previous is not None and previous.exists():
        keep_ids.add(previous.resolve())
    try:
        children = list(root.iterdir())
    except OSError:
        return
    for child in children:
        if not child.is_dir() or child.name.startswith("."):
            continue
        try:
            resolved = child.resolve()
        except OSError:
            continue
        if resolved not in keep_ids:
            shutil.rmtree(child, ignore_errors=True)


def reload_notice(harnesses: list[str]) -> str:
    steps = []
    if "cursor" in harnesses:
        steps.append("reload Cursor")
    if "claude_code" in harnesses:
        steps.append("restart open Claude Code sessions")
    if "codex" in harnesses:
        steps.append("open a new Codex session")
    if "opencode" in harnesses:
        steps.append("restart OpenCode")
    if not steps:
        return ""
    if len(steps) == 1:
        action = steps[0]
    elif len(steps) == 2:
        action = f"{steps[0]} and {steps[1]}"
    else:
        action = ", ".join(steps[:-1]) + f", and {steps[-1]}"
    return f"Cadri updated. {action[:1].upper()}{action[1:]} to load the new skills."


def apply() -> list[str]:
    """Rewrite harness copies from this CLI. No sign-in, upload, or book clone."""
    home = Path.home()
    harnesses = bundles.cadri_installed(home)
    if not harnesses:
        return []
    try:
        previous = stamp_path().read_text(encoding="utf-8").strip()
    except OSError:
        previous = ""
    lines = bundles.install(harnesses, home)
    record_bundle_version()
    if parsed(previous) and previous != local_version():
        notice = reload_notice(harnesses)
        if notice:
            write_text(notice_path(), notice + "\n")
    return lines


def apply_with_new_cli() -> str:
    bundles.write_stable_shim()
    shim = bundles.shim_path()
    result = subprocess.run(
        [str(shim), "update", "--apply"],
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
        env=os.environ.copy(),
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise UpdateError(detail or "could not refresh the agents")
    return result.stdout.strip()


def upgrade(manual: bool) -> str | None:
    """Install a newer release. None when this one is current or has no release version."""
    current = local_version()
    if parsed(current) is None:
        if manual:
            raise UpdateError("this Cadri is a source install, so it is not updated from GitHub")
        return None
    remote = latest_version()
    if not should_upgrade(current, remote):
        return None
    previous = current_target()
    dest = stage(remote)
    parked = flip(dest)
    prune(dest, parked or previous)
    applied = apply_with_new_cli()
    summary = f"updated {current} → {remote}"
    return summary if not applied else summary + "\n" + applied


def automatic() -> str:
    """The session-start check. Quiet when nothing is due, and never raises."""
    if os.environ.get("CADRI_NO_UPDATE") == "1" or not tracks_releases():
        return ""
    if not check_due() and not bundles_stale():
        return ""
    try:
        with hold():
            return _automatic()
    except LockTimeout:
        return ""
    except UpdateError as exc:
        print(f"cadri update: {exc}", file=sys.stderr)
        return ""


def _automatic() -> str:
    if check_due():
        mark_checked()
        upgraded = upgrade(manual=False)
        if upgraded:
            return upgraded
    if bundles_stale():
        lines = apply()
        if lines:
            return "\n".join(lines)
    return ""


def manual() -> str:
    if not tracks_releases() or parsed(local_version()) is None:
        raise UpdateError("this Cadri is a source install, so it is not updated from GitHub")
    try:
        with hold():
            mark_checked()
            upgraded = upgrade(manual=True)
            if upgraded:
                return upgraded
            if bundles_stale():
                lines = apply()
                if lines:
                    return "\n".join(lines)
            return f"Cadri {local_version()} is up to date."
    except LockTimeout as exc:
        raise UpdateError("another update is already running") from exc
