from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent
GENERATOR = HERE / "generate_update_manifest.py"
VALIDATOR = HERE / "validate_update_manifest.py"


class BootstrapManifestTests(unittest.TestCase):
    def test_round_trip_contract_and_hash_validation(self) -> None:
        version = "9.8.7"
        names = [
            f"origin-speak-{version}-windows-x86_64.exe",
            f"origin-speak-runtime-{version}-windows-x86_64.exe",
            f"origin-speak-{version}-macos-universal",
            f"origin-speak-runtime-{version}-macos-universal.zip",
        ]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for index, name in enumerate(names):
                (root / name).write_bytes(f"payload-{index}".encode())
            manifest = root / "bootstrap-update.json"
            common = [
                "--version", version,
                "--public-base-url", "https://github.com/devrajmahar/origin-speak",
                "--release-prefix", f"releases/download/v{version}",
                "--artifact-root", str(root),
            ]
            subprocess.run([sys.executable, str(GENERATOR), *common, "--output", str(manifest)], check=True)
            subprocess.run([sys.executable, str(VALIDATOR), "--manifest", str(manifest), *common], check=True)
            parsed = json.loads(manifest.read_text())
            self.assertEqual(parsed["schema_version"], 2)
            self.assertEqual(set(parsed["platforms"]["windows-x86_64"]), {"manager", "runtime"})
            self.assertEqual(
                parsed["platforms"]["windows-x86_64"]["manager"]["url"],
                f"https://github.com/devrajmahar/origin-speak/releases/download/v{version}/{names[0]}",
            )

    def test_validator_rejects_tampered_payload(self) -> None:
        version = "1.2.3"
        names = [
            f"origin-speak-{version}-windows-x86_64.exe",
            f"origin-speak-runtime-{version}-windows-x86_64.exe",
            f"origin-speak-{version}-macos-universal",
            f"origin-speak-runtime-{version}-macos-universal.zip",
        ]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in names:
                (root / name).write_bytes(b"payload")
            manifest = root / "bootstrap-update.json"
            common = [
                "--version", version,
                "--public-base-url", "https://github.com/devrajmahar/origin-speak",
                "--release-prefix", f"releases/download/v{version}",
                "--artifact-root", str(root),
            ]
            subprocess.run([sys.executable, str(GENERATOR), *common, "--output", str(manifest)], check=True)
            (root / names[1]).write_bytes(b"tampered")
            result = subprocess.run(
                [sys.executable, str(VALIDATOR), "--manifest", str(manifest), *common],
                text=True,
                capture_output=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("sha256 does not match", result.stderr + result.stdout)


if __name__ == "__main__":
    unittest.main()
