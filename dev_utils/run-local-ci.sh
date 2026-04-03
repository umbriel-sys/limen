#!/usr/bin/env bash
#
# run-local-ci.sh
#
# Usage:
#   ./run-local-ci.sh [OPTIONS]
#
# Description:
#   Runs the full local equivalent of the CI pipeline, including:
#     - rustfmt check (unless `--no-clippy-or-fmt` is set)
#     - build and test for:
#         * no-default-features
#         * alloc
#         * std
#         * spsc_raw
#     - clippy for the same feature sets (unless `--no-clippy-or-fmt` is set)
#
# Options:
#   --clean       Run `cargo clean` after all CI checks pass
#   --release     Run `cargo build --release` after all CI checks pass
#   --no-clippy-or-fmt
#                 Skip `cargo fmt` and all `cargo clippy` checks
#
# Examples:
#   ./run-local-ci.sh
#       Run all CI checks locally
#
#   ./run-local-ci.sh --clean
#       Run CI checks, then clean the workspace
#
#   ./run-local-ci.sh --release
#       Run CI checks, then build release
#
#   ./run-local-ci.sh --clean --release
#       Run CI checks, then clean and build release
#
#   ./run-local-ci.sh --no-clippy-or-fmt
#       Run CI checks locally, but skip rustfmt and clippy
#
#   ./run-local-ci.sh --clean --release --no-clippy-or-fmt
#       Run build/test checks only, then clean and build release

set -euo pipefail

SKIP_CLIPPY_AND_FMT=false
CLEAN_WORKSPACE=false
BUILD_RELEASE=false

printf "==> starting run-local-ci.sh\n\n"

# Parse args
for arg in "$@"; do
  case "$arg" in
    --no-clippy-or-fmt)
      SKIP_CLIPPY_AND_FMT=true
      ;;
    --clean)
      CLEAN_WORKSPACE=true
      ;;
    --release)
      BUILD_RELEASE=true
      ;;
    *)
      printf "Unknown argument: %s\n" "$arg"
      exit 1
      ;;
  esac
done

printf "==> starting local CI run\n\n"

printf "==> no-default-features\n"
printf "  -> cargo build --all --no-default-features --all-targets --verbose\n"
cargo build --all --no-default-features --all-targets --verbose
printf "  -> cargo test --all --no-default-features --all-targets --verbose\n"
cargo test --all --no-default-features --all-targets --verbose
if [ "$SKIP_CLIPPY_AND_FMT" = false ]; then
  printf "  -> cargo clippy --all --no-default-features -- -D warnings\n"
  cargo clippy --all --no-default-features -- -D warnings
else
  printf "  -> clippy skipped (--no-clippy-or-fmt)\n"
fi
printf "\n"

printf "==> alloc\n"
printf "  -> cargo build --all --features alloc --all-targets --verbose\n"
cargo build --all --features alloc --all-targets --verbose
printf "  -> cargo test --all --features alloc --all-targets --verbose\n"
cargo test --all --features alloc --all-targets --verbose
if [ "$SKIP_CLIPPY_AND_FMT" = false ]; then
  printf "  -> cargo clippy --all --features alloc -- -D warnings\n"
  cargo clippy --all --features alloc -- -D warnings
else
  printf "  -> clippy skipped (--no-clippy-or-fmt)\n"
fi
printf "\n"

printf "==> std\n"
printf "  -> cargo build --all --features std --all-targets --verbose\n"
cargo build --all --features std --all-targets --verbose
printf "  -> cargo test --all --features std --all-targets --verbose\n"
cargo test --all --features std --all-targets --verbose
if [ "$SKIP_CLIPPY_AND_FMT" = false ]; then
  printf "  -> cargo clippy --all --features std -- -D warnings\n"
  cargo clippy --all --features std -- -D warnings
else
  printf "  -> clippy skipped (--no-clippy-or-fmt)\n"
fi
printf "\n"

printf "==> spsc_raw\n"
printf "  -> cargo build --all --features spsc_raw --all-targets --verbose\n"
cargo build --all --features spsc_raw --all-targets --verbose
printf "  -> cargo test --all --features spsc_raw --all-targets --verbose\n"
cargo test --all --features spsc_raw --all-targets --verbose
if [ "$SKIP_CLIPPY_AND_FMT" = false ]; then
  printf "  -> cargo clippy --all --features spsc_raw -- -D warnings\n"
  cargo clippy --all --features spsc_raw -- -D warnings
else
  printf "  -> clippy skipped (--no-clippy-or-fmt)\n"
fi
printf "\n"

if [ "$SKIP_CLIPPY_AND_FMT" = false ]; then
  printf "==> rustfmt\n"
  printf "  -> cargo fmt --all -- --check\n"
  cargo fmt --all -- --check
  printf "\n"
else
  printf "==> rustfmt skipped (--no-clippy-or-fmt)\n\n"
fi

printf "==> local CI passed\n\n"

if [ "$CLEAN_WORKSPACE" = true ]; then
  printf "==> cleaning workspace\n"
  printf "  -> cargo clean\n"
  cargo clean
  printf "\n"
fi

if [ "$BUILD_RELEASE" = true ]; then
  printf "==> building release\n"
  printf "  -> cargo build --release\n"
  cargo build --release
  printf "\n"
fi

printf "==> finished run-local-ci.sh\n"
