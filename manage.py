#!/usr/bin/env python3
"""Compatibility launcher; operational commands are implemented by Rust."""
import os
import shutil
import sys
from pathlib import Path

binary = os.environ.get("SHOPCORE_BIN") or shutil.which("shopcore")
root = Path(__file__).resolve().parent
if not binary:
    for profile in ("release", "debug"):
        candidate = root / "target" / profile / "shopcore"
        if candidate.is_file():
            binary = str(candidate)
            break
if binary:
    os.execv(binary, [binary, *sys.argv[1:]])
if shutil.which("cargo"):
    os.chdir(root)
    os.execvp("cargo", ["cargo", "run", "--quiet", "--", *sys.argv[1:]])
raise SystemExit("shopcore binary not found; run: docker compose exec app python manage.py ...")
