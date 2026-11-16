"""Writes a release's version into the build (the release workflow runs it before building - the tag is the version).

    python scripts/set-version.py 2.6.0          -> Cargo.toml, Cargo.lock and Packager.toml say 2.6.0
    python scripts/set-version.py 2.6.0-rc1      -> Cargo.toml / Cargo.lock 2.6.0-rc1, Packager.toml 2.6.0

Packager.toml gets only X.Y.Z: an MSI's version is numbers only. Cargo.lock's entries for this workspace's own crates
change with Cargo.toml, so `cargo build --locked` still holds. The files keep their line endings.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERSION = re.compile(r"^(\d+\.\d+\.\d+)(-[0-9A-Za-z][0-9A-Za-z.]*)?$")


def rewrite(path: pathlib.Path, change, expect: str) -> None:
    new = change(path.read_bytes().decode("utf-8"))
    if f'version = "{expect}"' not in new:
        sys.exit(f"{path.name}: no version line was set - is the layout what this script expects?")
    path.write_bytes(new.encode("utf-8"))


def main() -> None:
    if len(sys.argv) != 2 or not VERSION.match(sys.argv[1].lstrip("v")):
        sys.exit("usage: set-version.py X.Y.Z[-suffix]")
    full = sys.argv[1].lstrip("v")
    base = VERSION.match(full).group(1)

    def cargo_toml(text: str) -> str:
        # The first `version = "..."` is [workspace.package]'s, which every crate takes.
        return re.sub(r'(?m)^version = "[^"]*"', f'version = "{full}"', text, count=1)

    def packager_toml(text: str) -> str:
        return re.sub(r'(?m)^version = "[^"]*"', f'version = "{base}"', text, count=1)

    # This workspace's own crates (crates/*/Cargo.toml) - not other path crates such as vendor/xattr.
    members = set()
    for manifest in (ROOT / "crates").glob("*/Cargo.toml"):
        name = re.search(r'(?m)^name = "([^"]+)"', manifest.read_text(encoding="utf-8"))
        if name:
            members.add(name.group(1))

    def cargo_lock(text: str) -> str:
        blocks = re.split(r"(?=\r?\n\[\[package\]\])", text)
        out = []
        for block in blocks:
            name = re.search(r'(?m)^name = "([^"]+)"', block)
            if "[[package]]" in block and name and name.group(1) in members and "\nsource = " not in block:
                block = re.sub(r'(?m)^version = "[^"]*"', f'version = "{full}"', block, count=1)
            out.append(block)
        return "".join(out)

    rewrite(ROOT / "Cargo.toml", cargo_toml, full)
    rewrite(ROOT / "Packager.toml", packager_toml, base)
    rewrite(ROOT / "Cargo.lock", cargo_lock, full)
    print(f"Version {full} (Packager.toml {base})")


if __name__ == "__main__":
    main()
