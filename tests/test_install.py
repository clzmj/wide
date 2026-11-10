#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import stat


PROJECT = Path(__file__).resolve().parents[1]
INSTALLER = PROJECT / "install.sh"
ASSET = "https://github.com/clzmj/wide/releases/download/v0.1.8/wide-aarch64-apple-darwin.tar.gz"


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.bin_dir = self.root / "mock-bin"
        self.bin_dir.mkdir()
        self.tmp_dir = self.root / "tmp"
        self.tmp_dir.mkdir()
        self.manifest_file = self.root / "manifest.json"
        self.archive_file = self.root / "wide.tar.gz"
        self.curl_log = self.root / "curl.log"
        self._make_archive()
        self._write_manifest()
        self._write_mocks()

    def tearDown(self):
        self.temp.cleanup()

    def _make_archive(self, extra=None, hardlink=False):
        payloads = {
            "wide": b"new-wide-binary",
            "LICENSE": b"license text\n",
            "completions/_wide": b"zsh completion\n",
            "completions/wide.bash": b"bash completion\n",
            "completions/wide.fish": b"fish completion\n",
        }
        if extra:
            payloads[extra] = b"unexpected"
        with tarfile.open(self.archive_file, "w:gz") as archive:
            for name, body in payloads.items():
                info = tarfile.TarInfo(name)
                if name == "wide" and hardlink:
                    info.type = tarfile.LNKTYPE
                    info.linkname = "../../victim"
                    archive.addfile(info)
                    continue
                info.size = len(body)
                info.mode = 0o755 if name == "wide" else 0o644
                archive.addfile(info, io.BytesIO(body))

    def _write_manifest(self, **overrides):
        manifest = {"version": "0.1.8", "url": ASSET,
                    "sha256": hashlib.sha256(self.archive_file.read_bytes()).hexdigest()}
        manifest.update(overrides)
        self.manifest_file.write_text(json.dumps(manifest))

    def _write_mocks(self):
        mocks = {
            "uname": '''#!/bin/sh
case "$1" in
  -s) printf '%s\\n' "${MOCK_UNAME_S:-Darwin}" ;;
  -m) printf '%s\\n' "${MOCK_UNAME_M:-arm64}" ;;
  *) exit 2 ;;
esac
''',
            "curl": '''#!/bin/sh
set -eu
out=
url=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) out=$2; shift 2 ;;
    http*) url=$1; shift ;;
    *) shift ;;
  esac
done
printf '%s\\n' "$url" >> "$CURL_LOG"
case "$url" in
  https://wide.clzmj.dev/latest.json) cp "$MANIFEST_FIXTURE" "$out" ;;
  "$ARCHIVE_URL") cp "$ARCHIVE_FIXTURE" "$out" ;;
  *) exit 9 ;;
esac
''',
            "plutil": f'''#!{sys.executable}
import json, sys
a = sys.argv[1:]
key = a[a.index("-extract") + 1]
with open(a[-1]) as f:
    print(json.load(f)[key])
''',
            "shasum": f'''#!{sys.executable}
import hashlib, sys
path = sys.argv[-1]
print(hashlib.sha256(open(path, "rb").read()).hexdigest() + "  " + path)
''',
        }
        for name, body in mocks.items():
            path = self.bin_dir / name
            path.write_text(body)
            path.chmod(0o755)

    def env(self, install_dir=None, **extra):
        env = os.environ.copy()
        env.update({"PATH": f"{self.bin_dir}:{env.get('PATH', '')}",
                    "TMPDIR": str(self.tmp_dir),
                    "MANIFEST_FIXTURE": str(self.manifest_file),
                    "ARCHIVE_FIXTURE": str(self.archive_file),
                    "ARCHIVE_URL": ASSET,
                    "CURL_LOG": str(self.curl_log),
                    "WIDE_INSTALL_DIR": str(install_dir or self.root / "install")})
        env.update(extra)
        return env

    def run_installer(self, *args, install_dir=None, **extra):
        return subprocess.run([str(INSTALLER), *args], cwd=PROJECT, env=self.env(install_dir, **extra),
                              capture_output=True, text=True)

    def test_installs_verified_binary_and_leaves_completions_uninstalled(self):
        destination = self.root / "user" / "bin"
        result = self.run_installer(install_dir=destination)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((destination / "wide").read_bytes(), b"new-wide-binary")
        self.assertIn(str(destination / "wide"), result.stdout)
        self.assertFalse((destination / "completions").exists())
        self.assertEqual(len(self.curl_log.read_text().splitlines()), 2)

    def test_dry_run_verifies_without_creating_install_destination(self):
        destination = self.root / "not-created"
        result = self.run_installer("--dry-run", install_dir=destination)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Dry run", result.stdout)
        self.assertFalse(destination.exists())

    def test_reports_existing_wide_earlier_in_path(self):
        existing = self.bin_dir / "wide"
        existing.write_text("#!/bin/sh\nexit 0\n")
        existing.chmod(0o755)
        destination = self.root / "user" / "bin"
        result = self.run_installer("--dry-run", install_dir=destination)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"PATH currently resolves wide to {existing}", result.stdout)

    def test_bad_checksum_preserves_existing_binary(self):
        destination = self.root / "install"
        destination.mkdir()
        binary = destination / "wide"
        binary.write_bytes(b"old-working-binary")
        self._write_manifest(sha256="0" * 64)
        result = self.run_installer(install_dir=destination)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256", result.stderr)
        self.assertEqual(binary.read_bytes(), b"old-working-binary")
        self.assertEqual(list(destination.iterdir()), [binary])

    def test_unsafe_archive_entry_preserves_existing_binary(self):
        destination = self.root / "install"
        destination.mkdir()
        binary = destination / "wide"
        binary.write_bytes(b"old-working-binary")
        self._make_archive("../outside")
        self._write_manifest()
        result = self.run_installer(install_dir=destination)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected archive entry", result.stderr)
        self.assertEqual(binary.read_bytes(), b"old-working-binary")
        self.assertFalse((self.root / "outside").exists())

    def test_hardlink_executable_is_rejected_before_chmod_or_replace(self):
        destination = self.root / "install"
        destination.mkdir()
        binary = destination / "wide"
        binary.write_bytes(b"old-working-binary")
        victim = self.tmp_dir / "victim"
        victim.write_bytes(b"must-not-be-chmodded")
        victim.chmod(0o644)
        self._make_archive(hardlink=True)
        self._write_manifest()
        result = self.run_installer(install_dir=destination)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("non-regular", result.stderr)
        self.assertEqual(binary.read_bytes(), b"old-working-binary")
        self.assertEqual(victim.read_bytes(), b"must-not-be-chmodded")
        self.assertEqual(stat.S_IMODE(victim.stat().st_mode), 0o644)

    def test_manifest_url_must_match_exact_versioned_asset(self):
        destination = self.root / "install"
        destination.mkdir()
        binary = destination / "wide"
        binary.write_bytes(b"old-working-binary")
        self._write_manifest(url="https://example.invalid/wide.tar.gz")
        result = self.run_installer(install_dir=destination)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected GitHub release asset", result.stderr)
        self.assertEqual(binary.read_bytes(), b"old-working-binary")
        self.assertEqual(len(self.curl_log.read_text().splitlines()), 1)

    def test_unsupported_platform_fails_before_network_access(self):
        result = self.run_installer(MOCK_UNAME_M="x86_64")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Apple Silicon", result.stderr)
        self.assertFalse(self.curl_log.exists())


if __name__ == "__main__":
    unittest.main()
