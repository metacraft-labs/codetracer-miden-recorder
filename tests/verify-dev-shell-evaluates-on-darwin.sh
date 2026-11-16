#!/usr/bin/env bash
# The dev shell evaluates for both macOS systems, whatever host this runs on.
#
# The shell builds on the `miden` shell of nix-blockchain-development, which is
# exported only on Linux, where its SDK exists. Taking it through `inputsFrom`
# unconditionally made evaluation fail on a Mac with an attribute-missing
# error, so the macOS CI leg could not start. Evaluating the real flake (no
# mocks, no builds) catches that, and a lock that moves back to a revision
# exporting the shell on Darwin.
#
#   bash tests/verify-dev-shell-evaluates-on-darwin.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT
for system in aarch64-darwin x86_64-darwin; do
  nix eval --no-write-lock-file --option builders '' --raw ".#devShells.$system.default.drvPath" >/dev/null 2>"$LOG" || {
    echo "FAIL: devShells.$system.default does not evaluate: $(grep 'error: ' "$LOG" | tail -1 | sed 's/^ *//')" >&2
    exit 1
  }
  echo "ok: devShells.$system.default evaluates"
done
