# TelegramS3 server debug runbook

This file is a read-only investigation and operations reference. It must not
contain passwords, API hashes, Telegram session material, admin cookies, S3
secrets, encryption keys, or proxy credentials. Use a password manager or the
existing private server credential store when SSH asks for a password.

## Server connection

| Property | Value |
|----------|-------|
| Host/IP | `191.101.113.64` |
| SSH port | `22` |
| SSH user | `root` |
| OS | Ubuntu 24.04.4 LTS |
| Kernel reported by user | `6.8.0-31-generic` |
| Public application | `https://telegram3.s3.nlp.artapanel.xyz` |
| Admin route | `https://telegram3.s3.nlp.artapanel.xyz/_admin/telegram` |
| SSH password | **Not stored in this repository.** Retrieve it from the approved private credential store before connecting. |

The supplied password was used for this investigation but is deliberately not
copied into repository documentation. If a reusable local source is needed,
keep it outside this repository and restrict its permissions; never paste it
into commands, logs, commits, or issue reports.

## Deployment identity observed on 2026-09-10

- Docker Compose project: `s3-telegrams3-o7gobi`
- Container: `s3-telegrams3-o7gobi-telegram-s3-1`
- Image: `ghcr.io/mostafa-binesh/telegrams3:v0.7.1-rc.5`
- Image revision label: `fef80596aac5e565cc8ba60a78f05e0d336a441fe`
- S3 listener inside the container: `0.0.0.0:9000`
- Admin API is loopback-only inside the container: `127.0.0.1:9001`
- Persistent volumes:
  - `s3-telegrams3-o7gobi_telegram-s3-session`
  - `s3-telegrams3-o7gobi_telegram-s3-metadata`
  - `s3-telegrams3-o7gobi_telegram-s3-data`

## Safe read-only connection and inspection

Use the private credential store for the password. Do not put the password in
these commands or in shell history. A pinned host key should be preferred when
one is available from the private server record.

```powershell
ssh root@191.101.113.64
```

Basic host/container checks:

```bash
docker ps --format 'table {{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}'
df -h /
free -h
docker logs --since 72h --timestamps s3-telegrams3-o7gobi-telegram-s3-1
```

Read-only application checks:

```bash
docker exec s3-telegrams3-o7gobi-telegram-s3-1 \
  /usr/local/bin/telegram-s3 db status
docker exec s3-telegrams3-o7gobi-telegram-s3-1 \
  /usr/local/bin/telegram-s3 auth status
docker exec s3-telegrams3-o7gobi-telegram-s3-1 \
  /usr/local/bin/telegram-s3 doctor
```

The admin API is served on port 9000 under `/_admin/api`; port 9001 is an
internal listener and does not expose those paths directly. Unauthenticated
session probing is safe, but the overview requires an authenticated admin
cookie:

```bash
docker exec s3-telegrams3-o7gobi-telegram-s3-1 \
  curl -sS http://127.0.0.1:9000/_admin/api/session
```

Read the SQLite database without opening it for writes. The database is live,
so use SQLite read-only mode and avoid `VACUUM`, migrations, repair, GC, or any
CLI command that changes state:

```bash
python3 - <<'PY'
import sqlite3

path = '/var/lib/docker/volumes/s3-telegrams3-o7gobi_telegram-s3-metadata/_data/metadata.sqlite'
db = sqlite3.connect('file:' + path + '?mode=ro', uri=True, timeout=2)
print([row[0] for row in db.execute(
    "select name from sqlite_master where type='table' order by name")])
print(list(db.execute(
    "select id, delete_uploaded_files, state, object_count, error "
    "from connection_removal_jobs order by requested_at")))
print(list(db.execute(
    "select target_kind, state, completed, count(*) "
    "from cleanup_targets group by target_kind, state, completed")))
db.close()
PY
```

## Investigation snapshot: 2026-09-10

Observed live host capacity:

- root filesystem: `40G` total, `36G` used, `1.3G` available, `97%` used
- memory: `7.8Gi` total, about `3.7Gi` available
- TelegramS3 container: healthy, running image `v0.7.1-rc.5`

Observed application logs and state:

- Startup logged `telegram bootstrap: Missing`, zero committed/active/staged
  objects, and one recovery marker.
- `db status` reported schema version `7`, zero committed/active/staged
  objects, and one recovery marker.
- `auth status` reported session state `Reused`.
- The live UI health error is consistent with the transport health code:
  `storage peer lookup failed` caused by Telegram RPC `401 AUTH_KEY_UNREGISTERED`
  while iterating dialogs through `messages.getDialogsChecked`.
- The metadata database contains three tombstoned manifests and two deleted
  buckets; four transfer jobs are `cleaned`.
- Removal job `957adc2a-4b93-4eb9-bf68-5cae77007823` completed at
  `2026-09-09 14:25:54 UTC`.
- Removal job `6f5ee2e3-4c63-4861-a150-2ff3abd6cd51` remains `pending`, has
  `delete_uploaded_files=1`, and covers one object.
- Its one blocking evidence target is object
  `784f7ade-1825-4dfa-ac79-4af9075f737c`, state `recovery_required`,
  `completed=0`, with one attempt and error:
  `Deletion-evidence acknowledgement is unknown; recovery material retained`.
- The cleanup table contains one incomplete evidence target, two completed
  evidence targets, and 154 completed message targets.
- The `telegram_bootstrap` setting still exists in metadata because the
  pending removal has not reached finalization. Its value was not copied here.
- The running container has an environment value
  `TELEGRAM_SESSION_PATH=/var/lib/telegram-s3/session/telegram.session`.
  The implementation in this worktree now honors that value consistently;
  the deployed `v0.7.1-rc.5` binary was observed using the metadata-derived
  path instead. The latter existed with mode `600` and size `1355`; the
  mounted session directory was empty. The deployment must be upgraded and
  the session volume verified before relying on the corrected path.
- Root storage pressure is a separate operational risk: the host is at 97%
  disk use and should be investigated before any rebuild or backup operation.

## Root cause and intended fix

There were two interacting problems. The local implementation now addresses
both; the live container remains unchanged until an explicitly reviewed
deployment:

1. The Telegram login wizard reported an `authorized` stage from the login
   driver/session check, but the success finalizer ignores the result of the
   subsequent transport health refresh. The UI then shows “Telegram account
   authorized” while the overview correctly reports `disconnected` after the
   storage-peer probe fails. Authorization must only be reported as complete
   after an end-to-end health probe succeeds, including storage-peer lookup.
   A refresh error must remain visible and keep the wizard in a retryable
   state. The wizard now exposes `connection_ready`, the health state, and the
   health detail, and the UI only closes with a success toast when readiness is
   `connected`. `AUTH_KEY_UNREGISTERED` is mapped to `needs_reauth`.
2. Connection removal was durable and safety-first, but it blocked
   re-login while any removal job is `pending` or `running`. Finalization waits
   for all cleanup targets, including an evidence target that is
   `recovery_required` when deletion acknowledgement is unknown. That leaves
   the old Telegram bootstrap settings/session in place and makes the next
   removal attempt return “a connection removal is already in progress”.

The process should be revised so that local connection detachment and the
   ability to start a new login are now independent of remote Telegram cleanup:

- On removal, atomically hide/tombstone the local namespace and mark the local
  connection detached immediately. The worker no longer clears new-session
  state only after remote cleanup completes.
- Stop using the old transport/session for new requests and allow a new login
  flow after local detachment, while retaining the old cleanup job as an
  isolated generation.
- Keep remote message deletion and evidence acknowledgement in a durable
  outbox. Never claim that remote deletion completed when Telegram returned an
  unknown/401 result; retain recovery evidence and show the job as blocked.
- Make cleanup generation/account identity explicit so a newly authorized
  account cannot process or delete the previous account's objects. Schema v8
  now stores that identifier on removal jobs and cleanup targets; mismatches
  are quarantined for recovery.
- Make the wizard success response depend on the same health snapshot used by
  the top-bar badge. Surface `needs_reauth`/`disconnected` and the exact cause
  instead of the unconditional authorized toast.
- Resolve the session path from one deployment setting only. The new code
  honors `TELEGRAM_SESSION_PATH`, with the metadata-derived path as fallback.
- Before deployment, free disk space safely and verify the metadata/session
  volume backups. Do not delete Telegram or SQLite data as a workaround for a
  stuck job.

## Source pointers for the diagnosis

- `src/admin.rs`: wizard state/success handling and
  `telegram_disconnect` (`finalize_wizard_success` currently ignores refresh
  failure; removal returns conflict for an active job).
- `src/telegram/transport.rs`: `evaluate_health` first resolves the storage
  peer and marks the connection disconnected on RPC failure; session status is
  otherwise allowed to remain `Reused`.
- `src/metadata/connection_removal.rs`: active pending/running jobs reject a
  second removal and removal tombstones the local namespace before cleanup.
- `src/object_format/workflow.rs`: final removal waits for cleanup targets,
  clears bootstrap settings, disconnects the transport, and only then marks
  the job completed.
- `src/durable.rs`: evidence-first cleanup ordering and the
  `recovery_required` state preserve ambiguous deletion evidence.

## Local implementation and deployment boundary

The worktree contains the schema-v8 lifecycle implementation and local Rust/UI
tests pass. No image was built, deployed, restarted, or mutated on the server
in this task. Before rollout, back up metadata/session volumes, verify free
disk space, migrate the database, and perform a controlled login/removal drill
through the SOCKS5 route documented in `AGENTS.md`.

## Do not run during investigation

Do not run `telegram-s3 repair`, `telegram-s3 gc` without `--dry-run`, database
migrations, `docker compose down`, container restarts, volume removal, session
deletion, direct SQLite updates, or Telegram delete/logout calls. Those are
state-changing actions and require an explicit repair/release plan and backup
verification.
