#!/usr/bin/env python3
"""Validate a generated native updater manifest against the shipping contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from urllib.parse import quote, urlsplit


SCHEMA_VERSION = 1
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
    platform: str,
    version: str,
    expected_kind: str,
    expected_arches: set[str],
    expected_name_re: re.Pattern[str],
    public_base_url: str,
    release_prefix: str,
    artifact_root: Path,
) -> None:
    if not isinstance(record, dict):
        fail(f"{platform} artifact must be an object")
    expected_keys = {"kind", "arch", "path", "url", "sha256"}
    if set(record) != expected_keys:
        fail(
            f"{platform} artifact keys must be exactly {sorted(expected_keys)}, "
            f"got {sorted(record)}"
        )
    if record["kind"] != expected_kind:
        fail(f"{platform} kind must be {expected_kind!r}")
    if record["arch"] not in expected_arches:
        fail(f"{platform} arch must be one of {sorted(expected_arches)}")

    path = record["path"]
    if not isinstance(path, str) or not expected_name_re.fullmatch(path):
        fail(f"{platform} path does not match the shipping filename contract: {path!r}")
    if platform == "macos":
        encoded_arch = path.removeprefix(f"ListenOS-{version}-macos-").removesuffix(".dmg")
        if encoded_arch != record["arch"]:
            fail("macos path architecture must match the artifact arch field")

    expected_url = (
        f"{public_base_url.rstrip('/')}/{release_prefix.strip('/')}/{quote(path)}"
    )
    if record["url"] != expected_url:
        fail(f"{platform} URL must be {expected_url!r}, got {record['url']!r}")

    digest = record["sha256"]
    if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
        fail(f"{platform} sha256 must be 64 lowercase hexadecimal characters")
    artifact = artifact_root / path
    if not artifact.is_file():
        fail(f"{platform} artifact is missing: {artifact}")
    actual = sha256(artifact)
    if digest != actual:
        fail(f"{platform} sha256 does not match {artifact.name}")


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
        fail(
            "public base URL must be a credential-free HTTPS origin/path without query or fragment"
        )
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        fail("manifest must be an object")
    expected_top_keys = {"schema_version", "version", "artifacts"}
    if set(manifest) != expected_top_keys:
        fail(
            f"manifest keys must be exactly {sorted(expected_top_keys)}, "
            f"got {sorted(manifest)}"
        )
    if manifest["schema_version"] != SCHEMA_VERSION:
        fail(f"schema_version must be {SCHEMA_VERSION}")
    if manifest["version"] != args.version:
        fail(f"manifest version must be {args.version!r}")

    artifacts = manifest["artifacts"]
    if not isinstance(artifacts, dict) or set(artifacts) != {"windows", "macos"}:
        fail("artifacts must contain exactly windows and macos")

    escaped_version = re.escape(args.version)
    validate_artifact(
        artifacts["windows"],
        platform="windows",
        version=args.version,
        expected_kind="nsis-installer",
        expected_arches={"x86_64"},
        expected_name_re=re.compile(
            rf"ListenOS-{escaped_version}-Setup-x86_64\.exe"
        ),
        public_base_url=args.public_base_url,
        release_prefix=args.release_prefix,
        artifact_root=args.artifact_root,
    )
    validate_artifact(
        artifacts["macos"],
        platform="macos",
        version=args.version,
        expected_kind="dmg-installer",
        expected_arches={"universal"},
        expected_name_re=re.compile(
            rf"ListenOS-{escaped_version}-macos-universal\.dmg"
        ),
        public_base_url=args.public_base_url,
        release_prefix=args.release_prefix,
        artifact_root=args.artifact_root,
    )


if __name__ == "__main__":
    main()
