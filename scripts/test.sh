#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
./scripts/cargo.sh test --locked
docker compose up -d --build --wait app
docker compose --profile test run --build --rm test
./scripts/test-cli.sh
