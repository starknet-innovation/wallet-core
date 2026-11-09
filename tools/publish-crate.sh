#!/usr/bin/env bash
# Publishes one workspace crate to crates.io and waits until the sparse index serves it, so the
# next crate in dependency order can resolve it. Idempotent, so a failed release can be re-run.
# Called by .github/workflows/publish-crates.yml only; never locally.
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

# The API records a version before a background job writes it to the index, and the next crate's
# `cargo publish` resolves against the index. Path is the sparse layout for names of 4+ characters.
in_index() {
  curl -fsS -A "${user_agent}" "https://index.crates.io/${crate:0:2}/${crate:2:2}/${crate}" \
    | jq -e --arg v "${version}" 'select(.vers == $v)' >/dev/null
}

# A version already on crates.io (a re-run) is not published again, but still waited for: the
# crate after it needs it in the index either way.
publish_log="$(mktemp)"
if is_published; then
  echo "${crate}@${version} already published, skipping publish"
# --locked: publish exactly the dependency graph CI verified.
elif ! cargo publish --locked -p "${crate}" 2>&1 | tee "${publish_log}"; then
  grep -qi "already exists on crates.io index" "${publish_log}" || exit 1
  echo "${crate}@${version} already published, skipping publish"
fi

for attempt in {1..20}; do
  if in_index; then
    echo "${crate}@${version} is in the crates.io index"
    exit 0
  fi
  echo "waiting for ${crate}@${version} to appear in the index (${attempt}/20)"
  sleep 15
done

echo "::error::${crate}@${version} is on crates.io but did not reach the index in time"
exit 1
