<!-- This crate holds key custody and signs. Treat every change as security-sensitive; see AGENTS.md. -->

## Summary

<!-- What changes, and why. -->

## Checks

- [ ] `bash tools/check.sh all` passes (or say which mode could not run, and why)
- [ ] Behaviour changes come with tests; parity changes with regenerated fixtures
- [ ] No secret can reach a log, error, `Debug` output or UniFFI error string
- [ ] The mobile build (`--no-default-features --features mobile`) gains no raw-key export

## Fixtures

<!-- If tests/fixtures changed: the Bramble commit they came from, and the updated sha256 rows. Otherwise "none". -->

none

## Dependencies

<!-- Any added or bumped dependency: pinned exactly, why it is needed. Otherwise "none". -->

none
