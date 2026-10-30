#!/usr/bin/env python3
"""Prepare a release: set the version and move the changelog's Unreleased notes under it.

    scripts/bump-version.py 0.4.0 ["Entry used if Unreleased is empty"]
    scripts/bump-version.py minor|patch|major [...]

Updates Cargo.toml, Cargo.lock, CHANGELOG.md (Unreleased -> new version, a fresh empty
Unreleased on top) and drops the README's "*(next release)*" markers. Prints the new
version. Commit and tag (v<version>) afterwards; the tag triggers the Release workflow.
"""

import datetime
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def current_version() -> str:
    return re.search(r'^version = "(.+?)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)


def next_version(arg: str) -> str:
    if re.fullmatch(r"\d+\.\d+\.\d+", arg):
        return arg
    major, minor, patch = map(int, current_version().split("."))
    return {
        "major": f"{major + 1}.0.0",
        "minor": f"{major}.{minor + 1}.0",
        "patch": f"{major}.{minor}.{patch + 1}",
    }[arg]


def set_version(version: str) -> None:
    toml = ROOT / "Cargo.toml"
    toml.write_text(re.sub(r'^version = ".+?"', f'version = "{version}"', toml.read_text(), count=1, flags=re.M))
    lock = ROOT / "Cargo.lock"
    lock.write_text(re.sub(r'(name = "haze"\nversion = )".+?"', rf'\1"{version}"', lock.read_text(), count=1))


def update_changelog(version: str, fallback_entry: str | None) -> None:
    path = ROOT / "CHANGELOG.md"
    text = path.read_text()
    head, rest = text.split("## Unreleased\n", 1)
    # Unreleased notes run until the next version heading.
    match = re.search(r"^## ", rest, re.M)
    notes, older = (rest[: match.start()], rest[match.start() :]) if match else (rest, "")
    notes = notes.strip()
    if not notes and fallback_entry:
        notes = f"- {fallback_entry}"
    today = datetime.date.today().isoformat()
    section = f"## {version} ({today})\n\n{notes}\n\n" if notes else f"## {version} ({today})\n\n"
    path.write_text(f"{head}## Unreleased\n\n{section}{older}")


def drop_readme_markers() -> None:
    path = ROOT / "README.md"
    path.write_text(path.read_text().replace(" *(next release)*", ""))


def main() -> None:
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    version = next_version(sys.argv[1])
    set_version(version)
    update_changelog(version, sys.argv[2] if len(sys.argv) > 2 else None)
    drop_readme_markers()
    print(version)


if __name__ == "__main__":
    main()
