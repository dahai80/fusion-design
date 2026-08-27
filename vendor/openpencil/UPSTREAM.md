# Vendored OpenPencil — Upstream Tracking

## Snapshot

- **Crate**: `op-ai` (OpenPencil AI chat layer)
- **Upstream version**: `0.8.1`
- **Source**: vendored subset of OpenPencil (transport-free `ChatProvider` trait + model catalog + agent-settings state)
- **Synced into fusion-design**: initial commit `aa3d8b9` (V0.1 MVP)
- **Local patches**: none — byte-for-byte snapshot of upstream `op-ai 0.8.1` `crates/op-ai/` subset

## Scope

Only `op-ai` is vendored. The other OpenPencil crates (editor-core, codegen, mcp, design-lint) are NOT vendored — they are replaced by self-built Rust counterparts:

- `fd-canvas-core` replaces `op-editor-core`
- `fd-codegen` replaces `op-codegen`
- `fd-design-lint` replaces `op-design-lint`
- `op-mcp` has no self-built replacement (the non-compliant self-built MCP layer was removed in H-A3/P2-2)

## Maintenance

- OpenPencil has no public release cadence to track mechanically. This is a **manual** sync.
- TODO (recurring, human): periodically check the upstream OpenPencil repository for new `op-ai` tags. If a newer version fixes a defect or adds a capability fusion-design needs, re-vendor the subset here and bump the version above.
- When re-vendoring, diff the new subset against `crates/op-ai/` and record any local patches (currently none) in this file.

## Why vendored (not crates.io dependency)

OpenPencil is not published to crates.io as a consumable package in the form fusion-design needs. Vendoring the `op-ai` subset pins a known-good, offline-buildable version with no registry/network dependency at build time — consistent with the fusion-design 100%-offline hard constraint.
