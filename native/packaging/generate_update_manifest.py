#!/usr/bin/env python3
"""Generate the exact ListenOS native updater manifest from packaged artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from urllib.parse import quote


SCHEMA_VERSION = 1


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_one(root: Path, pattern: str) -> Path:
    matches = sorted(path for path in root.glob(pattern) if path.is_file())
    if len(matches) != 1:
        raise SystemExit(
            f"expected exactly one artifact matching {pattern!r} under {root}, found {len(matches)}"
        )
    return matches[0]


def artifact_record(
    artifact: Path,
    *,
    kind: str,
    arch: str,
    public_base_url: str,
    release_prefix: str,
) -> dict[str, str]:
    relative_path = artifact.name
    encoded_name = quote(relative_path)
    url = f"{public_base_url.rstrip('/')}/{release_prefix.strip('/')}/{encoded_name}"
    return {
        "kind": kind,
        "arch": arch,
        "path": relative_path,
        "url": url,
        "sha256": sha256(artifact),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--public-base-url", required=True)
    parser.add_argument("--release-prefix", required=True)
    parser.add_argument("--artifact-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    version = args.version.strip()
    if not version:
        raise SystemExit("version must not be empty")

    root = args.artifact_root
    windows = require_one(root, f"ListenOS-{version}-Setup-x86_64.exe")
    macos = require_one(root, f"ListenOS-{version}-macos-universal.dmg")

    manifest = {
        "schema_version": SCHEMA_VERSION,
        "version": version,
        "artifacts": {
            "windows": artifact_record(
                windows,
                kind="nsis-installer",
                arch="x86_64",
                public_base_url=args.public_base_url,
                release_prefix=args.release_prefix,
            ),
            "macos": artifact_record(
                macos,
                kind="dmg-installer",
                arch="universal",
                public_base_url=args.public_base_url,
                release_prefix=args.release_prefix,
            ),
        },
    }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
