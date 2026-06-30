# Archive Manifest

Files moved to `_archive/` are **not deleted** — they are preserved for reference
but excluded from the active build and CI. They must not be imported or re-activated
without an explicit ZEP.

---

## Entry: zbx-finality (L3 fix — 2026-06-27)

**Reason:** `zbx-finality` was superseded by the inline finality logic in
`zbx-consensus` and `node/src/finalizer.rs`. The three remaining source files
had zero callers and no open issues referencing them. Keeping them in-tree caused
`cargo check` dead-code warnings and confused onboarding readers.

**Files archived:**

| Original path | Archived to |
|---|---|
| `crates/zbx-finality/src/checkpoint.rs` | `crates/zbx-finality/src/_archive/checkpoint.rs` |
| `crates/zbx-finality/src/justification.rs` | `crates/zbx-finality/src/_archive/justification.rs` |
| `crates/zbx-finality/src/tracker.rs` | `crates/zbx-finality/src/_archive/tracker.rs` |

**Replacement:** Finality is now tracked in `node/src/finalizer.rs` and
`crates/zbx-consensus/src/finality.rs`. See those files for the canonical
implementation.

**Restoration:** To reactivate, move the files back to `crates/zbx-finality/src/`
(from `crates/zbx-finality/src/_archive/`) and declare them in `zbx-finality/src/lib.rs`.
No schema migrations needed.

---
