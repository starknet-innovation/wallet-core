#!/usr/bin/env bash
# Canonical check entrypoint. CI (.github/workflows/ci.yml), the git hooks and agents all run these
# modes; keep command sequences here rather than copying them into workflows or docs.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

usage() {
  cat <<'EOF'
Usage: bash tools/check.sh <mode>...

  quick       fmt + guardrails (the pre-commit hook)
  rust        fmt + lint + test
  all         quick + lint + test + deny + package + wasm (run before handing off)

  fmt         cargo fmt --check
  guardrails  pins in sync, actions SHA-pinned, no print macros in src, 500-line cap
  lint        clippy: default features, mobile profile (--no-default-features), wasm32
  test        cargo test
  deny        cargo deny check
  package     cargo package: both crates build from what crates.io would receive
  wasm        build the npm package through ubrn, then type-check and test it
EOF
}

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

fail() {
  echo "error: $*" >&2
  failed=1
}

# Versions that must agree everywhere they are written.
toolchain_version() { sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml; }
locked_wasm_bindgen() { awk '/^name = "wasm-bindgen"$/ { f = 1; next } f && /^version = / { gsub(/^version = "|"$/, ""); print; exit }' Cargo.lock; }

check_fmt() { run cargo fmt --all --check; }

check_guardrails() {
  failed=0
  echo
  echo "==> guardrails"

  local rust bindgen file line
  rust="$(toolchain_version)"
  bindgen="$(locked_wasm_bindgen)"

  # 1. The Rust toolchain is pinned in rust-toolchain.toml and repeated in every workflow.
  while IFS= read -r line; do
    [[ "$line" == *"RUST_TOOLCHAIN: $rust" ]] || fail "$line: expected RUST_TOOLCHAIN: $rust (rust-toolchain.toml)"
  done < <(grep -Hn 'RUST_TOOLCHAIN:' .github/workflows/*.yml)

  # 2. wasm-bindgen-cli must be the exact version Cargo.lock resolves, or ubrn's output breaks.
  while IFS= read -r line; do
    [[ "$line" == *"wasm-bindgen-cli --version $bindgen "* ]] || fail "$line: expected wasm-bindgen-cli --version $bindgen (Cargo.lock)"
  done < <(grep -Hn 'wasm-bindgen-cli --version' .github/workflows/*.yml README.md AGENTS.md)

  # 3. Every action is pinned to a commit SHA (tags can be moved under us).
  while IFS= read -r line; do
    fail "$line: pin the action to a full commit SHA"
  done < <(grep -HnE '^\s*-?\s*uses:' .github/workflows/*.yml | grep -vE '@[0-9a-f]{40}( |$)' || true)

  # 4. Library code never prints: a stray debug print is how key material reaches a log.
  while IFS= read -r line; do
    fail "$line: no print/dbg macros in library code"
  done < <(git ls-files 'crates/*/src/*.rs' | grep -v '/src/bin/' \
    | xargs grep -HnE '\b(e?println|e?print|dbg)!' || true)

  # 5. Source files stay under 500 lines; split by behaviour before then.
  while IFS= read -r file; do
    [[ -f "$file" ]] || continue
    line="$(wc -l <"$file" | tr -d ' ')"
    ((line <= 500)) || fail "$file: $line lines (cap 500)"
  done < <(git ls-files '*.rs' '*.ts' '*.mjs' '*.sh')

  ((failed == 0)) || exit 1
  echo "guardrails ok"
}

check_lint() {
  run cargo clippy --all-targets --locked -- -D warnings
  run cargo clippy --locked --no-default-features -- -D warnings
  run cargo clippy --locked --target wasm32-unknown-unknown -- -D warnings
}

check_test() { run cargo test --locked --no-fail-fast; }

check_deny() { run cargo deny check; }

# Builds each crate from its .crate file, as crates.io would. bramble-core resolves wallet-core
# through a local registry, so this works before wallet-core is published. Needs a clean tree.
# Its own target directory: the packaged bramble-core builds against the published
# starknet-wallet-core, and its library has no hash in its file name (cdylib, staticlib), so in
# the shared target it would overwrite the workspace's and break the next `cargo test`.
check_package() { run cargo package --locked --workspace --target-dir target/package-verify; }

check_wasm() {
  local want have=""
  want="$(locked_wasm_bindgen)"
  command -v wasm-bindgen >/dev/null && have="$(wasm-bindgen --version | awk '{ print $2 }')"
  if [[ "$have" != "$want" ]]; then
    echo "error: wasm-bindgen $want required (found '${have:-none}'):" >&2
    echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
    exit 1
  fi
  cd js
  run npm ci
  run npm run build
  run npm run check
  run npm test
  cd "$root"
}

(($# > 0)) || { usage; exit 0; }
for mode in "$@"; do
  case "$mode" in
    quick) check_fmt; check_guardrails ;;
    rust) check_fmt; check_lint; check_test ;;
    all) check_fmt; check_guardrails; check_lint; check_test; check_deny; check_package; check_wasm ;;
    fmt) check_fmt ;;
    guardrails) check_guardrails ;;
    lint) check_lint ;;
    test) check_test ;;
    deny) check_deny ;;
    package) check_package ;;
    wasm) check_wasm ;;
    help | -h | --help) usage ;;
    *) echo "unknown mode: $mode" >&2; usage >&2; exit 2 ;;
  esac
done
