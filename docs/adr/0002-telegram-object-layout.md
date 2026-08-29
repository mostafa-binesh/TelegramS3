# ADR 0002 - Telegram Object Layout

## Status

Accepted

## Context

Telegram Drive shows that Telegram-backed storage needs a bounded, encrypted,
and recoverable envelope rather than a one-message-per-object model. Telegram S3
must support objects larger than a single Telegram file and must survive local
index loss.

## Decision

Store each object as:

- one manifest document
- one or more immutable chunk documents
- one local journal/index row

Use a conservative default chunk size of `1 MiB`.

## Consequences

- Range reads can be mapped to chunk spans.
- Large objects stay below Telegram's file limit.
- The manifest can rebuild the local index.
- Deletes and overwrites need tombstones and background cleanup.

## Rejected Alternatives

### Single Telegram document per object

Rejected because large S3 objects exceed Telegram's practical file limits.

### Caption-only metadata

Rejected because captions are not sufficient for durable object metadata.

