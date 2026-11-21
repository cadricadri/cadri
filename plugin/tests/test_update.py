"""Release checks, the install symlink, and harness refresh. No network."""

from __future__ import annotations

import io
import os
import subprocess
import tarfile
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from support import PLUGIN

import cadri.update as update
from cadri.install import bundles
from cadri.state import cadri_home


def release_tarball(version: str, extra: dict[str, bytes] | None = None) -> bytes:
    files = {
        "VERSION": (version + "\n").encode(),
        "pyproject.toml": b'[project]\nname = "cadri"\nversion = "0.0.0"\nrequires-python = ">=3.10"\n',
        "bin/cadri_cli.py": b"raise SystemExit(0)\n",
        "cadri/__init__.py": b"",
    }
    if extra:
        files.update(extra)
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, data in files.items():
            info = tarfile.TarInfo("cadri-plugin/" + name)
            info.size = len(data)
            info.mode = 0o644
            archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


class VersionTests(unittest.TestCase):
    def test_only_a_newer_release_moves_forward(self):
        self.assertTrue(update.should_upgrade("0.1.0", "0.2.0"))
        self.assertTrue(update.should_upgrade("0.1.9", "0.2.0"))
        self.assertFalse(update.should_upgrade("0.2.0", "0.2.0"))
        self.assertFalse(update.should_upgrade("0.3.0", "0.2.0"))
        self.assertFalse(update.should_upgrade("source", "0.2.0"))
        self.assertFalse(update.should_upgrade("0.2.0", "latest"))
        self.assertIsNone(update.parsed("1.2"))
        self.assertEqual(update.parsed("1.2.3"), (1, 2, 3))

    def test_a_managed_tree_is_the_installed_copy(self):
        home = Path("/tmp/cadri-home")
        with mock.patch("cadri.install.bundles.cadri_home", return_value=home):
            self.assertTrue(bundles.under_managed_tree(home / "versions" / "0.2.0" / "cadri" / "cli.py"))
            self.assertFalse(bundles.under_managed_tree(PLUGIN / "cadri" / "cli.py"))


class UpdateTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        self.cadri = self.home / ".cadri"
        env = {key: value for key, value in os.environ.items() if key != "CADRI_NO_UPDATE"}
        env.update({"HOME": str(self.home), "CADRI_HOME": str(self.cadri)})
        patcher = mock.patch.dict(os.environ, env, clear=True)
        patcher.start()
        self.addCleanup(patcher.stop)

    def shim(self, script: str | None = None) -> Path:
        path = self.cadri / "bin" / "cadri"
        path.parent.mkdir(parents=True)
        path.write_text(script or bundles.stable_shim_script(), encoding="utf-8")
        path.chmod(0o755)
        return path

    def tree(self, version: str) -> Path:
        dest = self.cadri / "versions" / version
        dest.mkdir(parents=True)
        (dest / "VERSION").write_text(version + "\n", encoding="utf-8")
        return dest

    def test_extract_drops_paths_outside_the_plugin_tree(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            for name, data in {
                "cadri-plugin/bin/cadri_cli.py": b"ok",
                "cadri-plugin/../../outside.txt": b"nope",
                "cadri-plugin/cadri/../../outside.txt": b"nope",
            }.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        self.cadri.mkdir()
        tarball = self.cadri / "plugin.tar.gz"
        tarball.write_bytes(buffer.getvalue())
        dest = self.cadri / "out"
        update.extract(tarball, dest)
        self.assertEqual((dest / "bin" / "cadri_cli.py").read_bytes(), b"ok")
        self.assertFalse((self.cadri / "outside.txt").exists())
        self.assertFalse((dest / "outside.txt").exists())

    def test_a_checkout_shim_is_not_a_release_install(self):
        self.shim("#!/bin/sh\nexec /usr/bin/python3 /src/plugin/bin/cadri_cli.py \"$@\"\n")
        self.assertFalse(update.tracks_releases())
        with self.assertRaises(update.UpdateError):
            update.manual()

    def test_the_daily_check_waits_and_can_be_switched_off(self):
        self.shim()
        self.tree("0.1.0")
        (self.cadri / "install").symlink_to(self.cadri / "versions" / "0.1.0")
        update.mark_checked()
        with mock.patch.object(update, "latest_version", side_effect=AssertionError("should not check")):
            self.assertEqual(update.automatic(), "")
            os.environ["CADRI_NO_UPDATE"] = "1"
            checked = update.checked_path()
            checked.write_text("0\n", encoding="utf-8")
            self.assertEqual(update.automatic(), "")
            os.environ.pop("CADRI_NO_UPDATE")
            # A checkout copied into ~/.cadri has no release version, so a due check stays local.
            self.assertEqual(update.automatic(), "")

    def test_upgrade_flips_the_symlink_keeps_one_previous_and_refreshes(self):
        self.shim()
        self.tree("0.1.0")
        self.tree("0.0.9")
        install = self.cadri / "install"
        install.symlink_to(self.cadri / "versions" / "0.1.0")
        tarball = release_tarball("0.2.0")

        def fake_download(url, dest, timeout):
            self.assertTrue(url.startswith("https://"))
            dest.write_bytes(tarball)

        def fake_sync(dest):
            python = dest / ".venv" / "bin" / "python"
            python.parent.mkdir(parents=True)
            python.write_text("#!/bin/sh\n", encoding="utf-8")

        spawned = {}

        def fake_run(argv, **kwargs):
            spawned["argv"] = argv
            spawned["env"] = kwargs.get("env")
            return subprocess.CompletedProcess(argv, 0, stdout="Cursor: refreshed\n", stderr="")

        with mock.patch.object(update, "local_version", return_value="0.1.0"), mock.patch.object(
            update, "latest_version", return_value="0.2.0"
        ), mock.patch.object(update, "download", fake_download), mock.patch.object(
            update, "sync_project", fake_sync
        ), mock.patch("cadri.update.subprocess.run", fake_run):
            message = update.manual()
        self.assertIn("updated 0.1.0 → 0.2.0", message)
        self.assertIn("Cursor: refreshed", message)
        self.assertTrue(install.is_symlink())
        self.assertEqual(install.resolve(), (self.cadri / "versions" / "0.2.0").resolve())
        self.assertEqual((self.cadri / "versions" / "0.2.0" / "VERSION").read_text(encoding="utf-8").strip(), "0.2.0")
        self.assertTrue((self.cadri / "versions" / "0.1.0").is_dir())
        self.assertFalse((self.cadri / "versions" / "0.0.9").exists())
        self.assertEqual(spawned["argv"][1:], ["update", "--apply"])
        self.assertEqual(spawned["env"][update.HOLDING], "1")
        shim = (self.cadri / "bin" / "cadri").read_text(encoding="utf-8")
        self.assertIn(str(install / ".venv" / "bin" / "python"), shim)
        self.assertNotIn("versions/0.2.0", shim)

    def test_a_failed_sync_leaves_the_current_install(self):
        self.shim()
        self.tree("0.1.0")
        install = self.cadri / "install"
        install.symlink_to(self.cadri / "versions" / "0.1.0")

        def fake_download(url, dest, timeout):
            dest.write_bytes(release_tarball("0.2.0"))

        with mock.patch.object(update, "local_version", return_value="0.1.0"), mock.patch.object(
            update, "latest_version", return_value="0.2.0"
        ), mock.patch.object(update, "download", fake_download), mock.patch.object(
            update, "sync_project", side_effect=update.UpdateError("uv sync failed")
        ):
            with self.assertRaises(update.UpdateError):
                update.manual()
        self.assertEqual(install.resolve(), (self.cadri / "versions" / "0.1.0").resolve())
        self.assertFalse((self.cadri / "versions" / "0.2.0").exists())

    def test_an_old_directory_install_is_moved_aside(self):
        self.shim()
        install = self.cadri / "install"
        install.mkdir(parents=True)
        (install / "VERSION").write_text("0.1.0\n", encoding="utf-8")
        (install / "marker").write_text("old", encoding="utf-8")

        def fake_download(url, dest, timeout):
            dest.write_bytes(release_tarball("0.2.0"))

        with mock.patch.object(update, "local_version", return_value="0.1.0"), mock.patch.object(
            update, "latest_version", return_value="0.2.0"
        ), mock.patch.object(update, "download", fake_download), mock.patch.object(update, "sync_project"), mock.patch(
            "cadri.update.subprocess.run",
            return_value=subprocess.CompletedProcess(["cadri"], 0, stdout="", stderr=""),
        ):
            update.manual()
        self.assertTrue(install.is_symlink())
        self.assertEqual((self.cadri / "versions" / "0.1.0" / "marker").read_text(encoding="utf-8"), "old")
        self.assertTrue((self.cadri / "versions" / "0.2.0" / "bin" / "cadri_cli.py").is_file())

    def test_apply_asks_for_a_reload_only_when_the_bundle_version_changed(self):
        plugin = self.home / ".cursor" / "plugins" / "local" / "cadri"
        plugin.mkdir(parents=True)
        update.stamp_path().parent.mkdir(parents=True, exist_ok=True)
        update.stamp_path().write_text("0.1.0\n", encoding="utf-8")
        with mock.patch.object(update, "local_version", return_value="0.2.0"):
            lines = update.apply()
        self.assertTrue(any("Cursor" in line for line in lines))
        self.assertIn("Reload Cursor", update.notice_path().read_text(encoding="utf-8"))
        self.assertEqual(update.stamp_path().read_text(encoding="utf-8").strip(), "0.2.0")
        update.notice_path().unlink()
        with mock.patch.object(update, "local_version", return_value="0.2.0"):
            update.apply()
        self.assertFalse(update.notice_path().exists())

    def test_a_due_check_upgrades_in_the_background(self):
        self.shim()
        self.tree("0.1.0")
        (self.cadri / "install").symlink_to(self.cadri / "versions" / "0.1.0")
        update.checked_path().parent.mkdir(parents=True, exist_ok=True)
        update.checked_path().write_text(str(time.time() - update.CHECK_INTERVAL - 10), encoding="utf-8")

        def fake_download(url, dest, timeout):
            dest.write_bytes(release_tarball("0.1.1"))

        with mock.patch.object(update, "local_version", return_value="0.1.0"), mock.patch.object(
            update, "latest_version", return_value="0.1.1"
        ), mock.patch.object(update, "download", fake_download), mock.patch.object(update, "sync_project"), mock.patch(
            "cadri.update.subprocess.run",
            return_value=subprocess.CompletedProcess(["cadri"], 0, stdout="", stderr=""),
        ):
            message = update.automatic()
        self.assertIn("updated 0.1.0 → 0.1.1", message)
        self.assertFalse(update.check_due())
        self.assertEqual(cadri_home(), self.cadri)
