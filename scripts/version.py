#!/usr/bin/env python3
"""Keep Origin Speak native/backend Cargo package versions in sync."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
BACKEND_MANIFEST = ROOT / "backend" / "Cargo.toml"
BACKEND_LOCK = ROOT / "backend" / "Cargo.lock"
NATIVE_MANIFEST = ROOT / "native" / "Cargo.toml"
NATIVE_LOCK = ROOT / "native" / "Cargo.lock"
SEMVER = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def package_version(content: str) -> str:
    in_package = False
    for raw_line in content.splitlines():
        line = raw_line.strip()
        if line == "[package]":
            in_package = True
            continue
        if line.startswith("[") and line.endswith("]"):
            in_package = False
        if in_package:
            match = re.fullmatch(r'version\s*=\s*"([^"]+)"', line)
            if match:
                return match.group(1)
    raise ValueError("could not find [package] version")


def update_manifest(content: str, version: str) -> str:
    lines = content.splitlines(keepends=True)
    in_package = False
    for index, raw_line in enumerate(lines):
        line = raw_line.strip()
        if line == "[package]":
            in_package = True
            continue
        if line.startswith("[") and line.endswith("]"):
            in_package = False
        if in_package and line.startswith("version = "):
            newline = "\r\n" if raw_line.endswith("\r\n") else "\n" if raw_line.endswith("\n") else ""
            lines[index] = f'version = "{version}"{newline}'
            return "".join(lines)
    raise ValueError("could not find [package] version")


def update_lock(content: str, package_names: list[str], version: str) -> str:
    updated = content
    for name in package_names:
        pattern = re.compile(
            rf'(\[\[package\]\]\r?\nname = "{re.escape(name)}"\r?\nversion = ")[^"]+(")'
        )
        updated, count = pattern.subn(rf"\g<1>{version}\g<2>", updated, count=1)
        if count != 1:
            raise ValueError(f"could not find {name} in Cargo.lock")
    return updated


def write_if_changed(path: Path, content: str, label: str) -> None:
    current = path.read_text(encoding="utf-8")
    if current == content:
        print(f"{label} already in sync")
        return
    path.write_text(content, encoding="utf-8", newline="")
    print(f"Updated {label}")


def apply_version(version: str) -> None:
    if not SEMVER.fullmatch(version):
        raise ValueError(f"invalid semver version: {version}")

    backend_manifest = BACKEND_MANIFEST.read_text(encoding="utf-8")
    native_manifest = NATIVE_MANIFEST.read_text(encoding="utf-8")
    backend_lock = BACKEND_LOCK.read_text(encoding="utf-8")
    native_lock = NATIVE_LOCK.read_text(encoding="utf-8")

    write_if_changed(
        BACKEND_MANIFEST,
        update_manifest(backend_manifest, version),
        f"backend/Cargo.toml -> {version}",
    )
    write_if_changed(
        NATIVE_MANIFEST,
        update_manifest(native_manifest, version),
        f"native/Cargo.toml -> {version}",
    )
    write_if_changed(
        BACKEND_LOCK,
        update_lock(backend_lock, ["origin-speak-backend"], version),
        f"backend/Cargo.lock -> {version}",
    )
    write_if_changed(
        NATIVE_LOCK,
        update_lock(native_lock, ["origin-speak-backend", "origin-speak-native"], version),
        f"native/Cargo.lock -> {version}",
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    bump = subparsers.add_parser("bump", help="set all package versions")
    bump.add_argument("version")
    subparsers.add_parser("sync", help="sync all versions from native/Cargo.toml")
    args = parser.parse_args()

    if args.command == "bump":
        apply_version(args.version.strip())
        return

    version = package_version(NATIVE_MANIFEST.read_text(encoding="utf-8"))
    apply_version(version)


if __name__ == "__main__":
    main()
