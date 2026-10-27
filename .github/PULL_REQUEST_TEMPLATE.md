## What changed

<!-- one paragraph, imperative mood -->

## Why

<!-- the problem this solves; link the issue -->

## Verification

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] If `log.data` writes changed: round-trip test covers it and
      `docs/interop.md` still holds
- [ ] No new dependency added without justification below

<!-- dep justification (if any): -->
