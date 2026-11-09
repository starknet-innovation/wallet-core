#!/usr/bin/env bash
# Publishes one workspace crate to crates.io and waits until the index serves it, so the next crate
# in dependency order can resolve it. Idempotent: a version already on crates.io is skipped, so a
# failed release can be re-run. Called by .github/workflows/publish-crates.yml only; never locally.
set -euo pipefail

crate="$1"
user_agent="wallet-core-ci (https://github.com/starknet-innovation/wallet-core)"
version="$(cargo metadata --no-deps --format-version 1 --locked \
  | jq -r ".packages[] | select(.name == \"${crate}\") | .version")"

if [[ -z "${version}" || "${version}" == "null" ]]; then
  echo "::error::crate ${crate} was not found in cargo metadata"
  exit 1
fi

is_published() {
  curl -fsS -A "${user_agent}" "https://crates.io/api/v1/crates/${crate}/${version}" >/dev/null
}

if is_published; then
  echo "${crate}@${version} already published, skipping"
  exit 0
fi

publish_log="$(mktemp)"
# --locked: publish exactly the dependency graph CI verified.
if ! cargo publish --locked -p "${crate}" 2>&1 | tee "${publish_log}"; then
  if grep -qi "already exists on crates.io index" "${publish_log}"; then
    echo "${crate}@${version} already published, skipping"
    exit 0
  fi
  exit 1
fi

for attempt in {1..20}; do
  if is_published; then
    echo "${crate}@${version} is visible on crates.io"
    exit 0
  fi
  echo "waiting for ${crate}@${version} to appear on crates.io (${attempt}/20)"
  sleep 15
done

echo "::error::${crate}@${version} was published but did not become visible in time"
exit 1
