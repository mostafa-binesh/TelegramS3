# ADR 0001 - RustFS Integration Strategy

## Status

Accepted

## Context

RustFS upstream already exposes a stable internal contract surface, but the
actual S3 server is wired to an ECStore backend that assumes local disks,
directory renames, and filesystem durability semantics. Telegram Drive provides
useful MTProto, session, retry, and envelope ideas, but not a drop-in RustFS
backend.

## Decision

Use a maintained RustFS fork with Telegram-specific storage integration rather
than trying to bolt Telegram onto ECStore as an unrelated plugin crate.

## Why This Decision

- RustFS has the right S3 server behavior and request processing.
- The concrete backend is still disk-centric, so a narrow adapter is not enough.
- A maintained fork keeps the integration coherent with the existing server.

## Rejected Alternatives

### Independent adapter crate

Rejected because the seam is not yet a clean external plugin boundary.

### Generic storage-provider trait upstream patch

Rejected as the best long-term architecture, but too large for the initial
maintainable implementation path.

### Gateway only

Rejected because it would diverge from RustFS request processing and not satisfy
the RustFS-centered requirement.

