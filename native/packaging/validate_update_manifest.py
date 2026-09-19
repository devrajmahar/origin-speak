#!/usr/bin/env python3
"""Validate the Origin Speak CLI/bootstrap update manifest and artifact hashes."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from urllib.parse import quote, urlsplit


SCHEMA_VERSION = 2
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fail(message: str) -> None:
    raise SystemExit(message)


def validate_artifact(
    record: object,
    *,
    label: str,
    expected_kind: str,
    expected_arch: str,
    expected_name: str,
    public_base_url: str,
    release_prefix: str,
    artifact_root: Path,
) -> None:
    if not isinstance(record, dict):
        fail(f"{label} artifact must be an object")
    expected_keys = {"kind", "arch", "path", "url", "sha256"}
    if set(record) != expected_keys:
        fail(f"{label} artifact keys must be exactly {sorted(expected_keys)}")
    if record["kind"] != expected_kind:
        fail(f"{label} kind must be {expected_kind!r}")
    if record["arch"] != expected_arch:
        fail(f"{label} arch must be {expected_arch!r}")
    if record["path"] != expected_name:
        fail(f"{label} path must be {expected_name!r}")

    expected_url = (
        f"{public_base_url.rstrip('/')}/{release_prefix.strip('/')}/{quote(expected_name)}"
    )
    if record["url"] != expected_url:
        fail(f"{label} URL must be {expected_url!r}, got {record['url']!r}")

    digest = record["sha256"]
    if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
        fail(f"{label} sha256 must be 64 lowercase hexadecimal characters")
    artifact = artifact_root / expected_name
    if not artifact.is_file():
        fail(f"{label} artifact is missing: {artifact}")
    if digest != sha256(artifact):
        fail(f"{label} sha256 does not match {artifact.name}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--public-base-url", required=True)
    parser.add_argument("--release-prefix", required=True)
    parser.add_argument("--artifact-root", required=True, type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    public_base = urlsplit(args.public_base_url.rstrip("/"))
    if (
        public_base.scheme != "https"
        or not public_base.netloc
        or public_base.username is not None
        or public_base.password is not None
        or public_base.query
        or public_base.fragment
    ):
        fail("public base URL must be a credential-free HTTPS origin/path without query or fragment")

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        fail("manifest must be an object")
    if set(manifest) != {"schema_version", "version", "platforms"}:
        fail("manifest keys must be exactly schema_version, version, platforms")
    if manifest["schema_version"] != SCHEMA_VERSION:
        fail(f"schema_version must be {SCHEMA_VERSION}")
    if manifest["version"] != args.version:
        fail(f"manifest version must be {args.version!r}")

    platforms = manifest["platforms"]
    if not isinstance(platforms, dict) or set(platforms) != {"windows-x86_64", "macos-universal"}:
        fail("platforms must contain exactly windows-x86_64 and macos-universal")
    for platform, record in platforms.items():
        if not isinstance(record, dict) or set(record) != {"manager", "runtime"}:
            fail(f"{platform} must contain exactly manager and runtime payloads")

    version = args.version
    validate_artifact(
        platforms["windows-x86_64"]["manager"],
        label="windows manager",
        expected_kind="cli-manager",
        expected_arch="x86_64",
        expected_name=f"origin-speak-{version}-windows-x86_64.exe",
        public_base_url=args.public_base_url,
        release_prefix=args.release_prefix,
        artifact_root=args.artifact_root,
    )
    validate_artifact(
        platforms["windows-x86_64"]["runtime"],
        label="windows runtime",
        expected_kind="silent-runtime",
        expected_arch="x86_64",
        expected_name=f"origin-speak-runtime-{version}-windows-x86_64.exe",
        public_base_url=args.public_base_url,
        release_prefix=args.release_prefix,
        artifact_root=args.artifact_root,
    )
    validate_artifact(
        platforms["macos-universal"]["manager"],
        label="macos manager",
        expected_kind="cli-manager",
        expected_arch="universal",
        expected_name=f"origin-speak-{version}-macos-universal",
        public_base_url=args.public_base_url,
        release_prefix=args.release_prefix,
        artifact_root=args.artifact_root,
    )
    validate_artifact(
        platforms["macos-universal"]["runtime"],
        label="macos runtime",
        expected_kind="app-bundle-zip",
        expected_arch="universal",
        expected_name=f"origin-speak-runtime-{version}-macos-universal.zip",
        public_base_url=args.public_base_url,
        release_prefix=args.release_prefix,
        artifact_root=args.artifact_root,
    )


if __name__ == "__main__":
    main()
