#!/bin/sh
# Shared builder: one process at a time; dependencies and outputs survive containers.
set -eu
cd "$(dirname "$0")/.."
exec docker run --rm -v "$PWD:/workspace" -v shopcore-rustup:/usr/local/rustup -v shopcore-cargo:/usr/local/cargo -v shopcore-target:/workspace/target -w /workspace rust:1.91-bookworm cargo "$@"
