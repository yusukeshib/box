#!/usr/bin/env python3
"""Update the box Homebrew formula from release artifacts."""

import argparse
import re
from pathlib import Path

ASSETS = (
    "box-aarch64-darwin",
    "box-x86_64-darwin",
    "box-aarch64-linux",
    "box-x86_64-linux",
)


def checksum_for(artifacts: Path, asset: str) -> str:
    matches = list(artifacts.rglob(f"{asset}.sha256"))
    if len(matches) != 1:
        raise ValueError(f"expected one checksum for {asset}, found {len(matches)}")

    checksum = matches[0].read_text().split()[0]
    if not re.fullmatch(r"[0-9a-f]{64}", checksum):
        raise ValueError(f"invalid SHA-256 checksum for {asset}: {checksum}")
    return checksum


def update_formula(formula: Path, version: str, artifacts: Path) -> None:
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"invalid stable version: {version}")

    content = formula.read_text()
    content, count = re.subn(
        r'(?m)^  version "[^"]+"$', f'  version "{version}"', content
    )
    if count != 1:
        raise ValueError(f"expected one version declaration, found {count}")

    for asset in ASSETS:
        checksum = checksum_for(artifacts, asset)
        pattern = (
            rf'(?m)(url "[^"]+/{re.escape(asset)}"\n'
            rf'\s+sha256 ")[0-9a-f]{{64}}("$)'
        )
        content, count = re.subn(pattern, rf"\g<1>{checksum}\2", content)
        if count != 1:
            raise ValueError(f"expected one formula entry for {asset}, found {count}")

    formula.write_text(content)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--formula", required=True, type=Path)
    args = parser.parse_args()

    update_formula(args.formula, args.version, args.artifacts)


if __name__ == "__main__":
    main()
