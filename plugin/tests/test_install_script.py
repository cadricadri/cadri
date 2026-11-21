"""install.sh from a checkout copies the tree and writes the shim, without downloading uv or a release."""

from __future__ import annotations

import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

PLUGIN = Path(__file__).resolve().parent.parent
INSTALL = PLUGIN / "install.sh"


class InstallScriptTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        bindir = self.home / "fakebin"
        bindir.mkdir()
        uv = bindir / "uv"
        uv.write_text('#!/bin/sh\nprintf \'%s\\n\' "$*" >> "$CADRI_UV_LOG"\nexit 0\n', encoding="utf-8")
        uv.chmod(0o755)
        self.uv_log = self.home / "uv.log"
        self.env = os.environ.copy()
        self.env.update(
            {
                "HOME": str(self.home),
                "PATH": f"{bindir}:/bin:/usr/bin",
                "CADRI_UV_LOG": str(self.uv_log),
                "SHELL": "/bin/zsh",
            }
        )
        self.env.pop("CADRI_REF", None)
        self.env.pop("CADRI_PLUGIN_TGZ", None)
        self.env.pop("CADRI_HOME", None)

    def install(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(INSTALL)],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )

    def test_a_checkout_installs_into_the_home_directory(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn(f"Installing from this checkout: {PLUGIN}", result.stderr)
        self.assertNotIn("Downloading", result.stderr)
        installed = self.home / ".cadri" / "install"
        self.assertTrue(installed.is_symlink())
        self.assertEqual(installed.resolve(), (self.home / ".cadri" / "versions" / "source").resolve())
        self.assertTrue((installed / "cadri" / "cli.py").is_file())
        self.assertTrue((installed / "pyproject.toml").is_file())
        shim = (self.home / ".cadri" / "bin" / "cadri").read_text(encoding="utf-8")
        self.assertIn(str(installed / ".venv" / "bin" / "python"), shim)
        self.assertIn(str(installed / "bin" / "cadri_cli.py"), shim)
        link = self.home / ".local" / "bin" / "cadri"
        self.assertTrue(link.is_symlink())
        self.assertEqual(link.resolve(), (self.home / ".cadri" / "bin" / "cadri").resolve())
        self.assertIn("sync --quiet --project", self.uv_log.read_text(encoding="utf-8"))
        self.assertIn(str(installed), self.uv_log.read_text(encoding="utf-8"))
        self.assertIn(".local/bin", (self.home / ".zshrc").read_text(encoding="utf-8"))
        self.assertIn("cadri init", result.stdout)

    def test_a_second_install_keeps_the_virtualenv_and_drops_stale_files(self):
        self.assertEqual(self.install().returncode, 0)
        install_dir = self.home / ".cadri" / "install"
        keep = install_dir / ".venv" / "keep"
        keep.parent.mkdir()
        keep.write_text("venv", encoding="utf-8")
        stale = install_dir / "stale.txt"
        stale.write_text("old", encoding="utf-8")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(keep.read_text(encoding="utf-8"), "venv")
        self.assertFalse(stale.exists())
        self.assertTrue(stat.S_IXUSR & (self.home / ".cadri" / "bin" / "cadri").stat().st_mode)
