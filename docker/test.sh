#!/usr/bin/env bash
# Runs the docker end-to-end test on a fresh, throwaway network; exits with its result.
set -euo pipefail
cd "$(dirname "$0")"
P="wharfedale-e2e-$$"
cleanup() { docker compose -p "$P" -f compose.yml --profile test down -v >/dev/null 2>&1 || true; }
trap cleanup EXIT
docker compose -p "$P" -f compose.yml --profile test build
docker compose -p "$P" -f compose.yml --profile test run --rm e2e
