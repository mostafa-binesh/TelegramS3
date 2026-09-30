# ADR-0023: Server-side admin moves and configurable verifier startup

- Status: accepted for 0.7.6-rc.18
- Date: 2026-09-30

## Context

The admin browser previously implemented Move by downloading every selected
object into the browser and uploading it again. That duplicated client
bandwidth, could exhaust browser memory for large files, and did not support
folder rows. The recovery verifier also always started a remote scan as soon
as its worker started, which made startup cost impossible to control from the
operator UI. Automatic physical replication also re-downloaded and re-uploaded
payloads even when both connected accounts used the same Telegram storage chat.

## Decision

Add an authenticated `POST /_admin/api/objects/move` operation. The
server expands selected folder prefixes from the active local index, creates a
durable destination manifest that reuses the source encrypted chunk
references, publishes only the manifest document, and tombstones its source
only after the destination commit succeeds. Folder moves preserve the
selected folder basename and relative descendants, including directory-marker
objects. Destination conflicts are rejected before the first move. A partial
process failure may leave a committed destination and an untouched source;
this is safe and recoverable, but the conflict must be resolved before
retrying that item.

Automatic physical replication reuses the existing Telegram message when the
source and target connections use the same storage chat. Replication between
different storage chats retains the verified encrypted download/upload
fallback, while access mode remains a shared-chat pointer.

Add the persisted `telegram_recovery_verify_startup` setting,
defaulting to `true` for existing and new databases. It is independent
from the verifier enabled switch: when enabled, a worker performs one
asynchronous remote scan immediately; when disabled, the first scan waits for
the configured interval. The environment variable
`TELEGRAM_RECOVERY_VERIFY_STARTUP` only seeds a missing database value.

## Consequences

- Moves transfer only small manifest documents; the browser and Telegram
  transport do not download or re-upload object payloads.
- Source data remains visible until each replacement destination is committed,
  preserving the delete/tombstone and cleanup invariants.
- Folder moves use the normal durable transfer queue one manifest at a time;
  a process failure leaves committed destinations and untouched sources for
  retry review.
- Operators can defer the expensive first remote verifier scan without
  disabling the recurring verifier policy. The scheduled first-run timestamp
  is exposed immediately, so the Overview does not remain at an indefinite
  “waiting for first scan” state.
