# ADR-0009: Commit stale send normalization before reconciliation

## Status

Accepted

## Context

An upload worker can lose its lease after a Telegram document may have been
sent but before the send attempt is checkpointed. The durable job is then
`recovery_required`, while an attempt can remain `sending`. A recovery poll may
have no newly claimable transfer, so rolling back its normalization leaves the
same `sending` row forever and blocks both automatic reconciliation and Retry.

## Decision

Recovery polling must normalize `sending` attempts for lease-less
`recovery_required` jobs to `unknown` and commit that transaction even when no
new transfer is claimed. Reconciliation then matches the durable token and
exact encrypted bytes before checkpointing or safely requeueing the attempt.

The release suite must include this persisted state as a regression test and
must keep exact-match, absent-after-scan, byte-collision, missing-staging, and
Telegram-unavailable outcomes distinct.

## Consequences

- A restarted worker can make progress without an operator database repair.
- Ambiguous remote data remains conservative and cannot be replayed blindly.
- Recovery tests must assert transaction durability, not only in-memory state.
