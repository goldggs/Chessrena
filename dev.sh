#!/usr/bin/env bash
set -euo pipefail

# Install Rust outside the repository and keep build output out of the workspace.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMP_BASE="${CHESSRENA_TEMP_DIR:-/var/tmp}"
TMP_BASE="${CHESSRENA_TOOLCHAIN_DIR:-$TEMP_BASE/chessrena-toolchain}"
export RUSTUP_HOME="$TMP_BASE/rustup"
export CARGO_HOME="$TMP_BASE/cargo"
export CARGO_TARGET_DIR="${CHESSRENA_TARGET_DIR:-$TEMP_BASE/target}"
export PATH="$CARGO_HOME/bin:$PATH"

if [[ ! -x "$CARGO_HOME/bin/cargo" ]]; then
  mkdir -p "$TMP_BASE"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    RUSTUP_HOME="$RUSTUP_HOME" CARGO_HOME="$CARGO_HOME" \
    sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable
fi

if [[ "${1:-}" == "setup" ]]; then
  "$CARGO_HOME/bin/rustup" component add rustfmt clippy
  "$CARGO_HOME/bin/rustup" show
  exit 0
fi

if [[ $# -eq 0 ]]; then
  set -- test
fi
cd "$ROOT"
exec "$CARGO_HOME/bin/cargo" "$@"
