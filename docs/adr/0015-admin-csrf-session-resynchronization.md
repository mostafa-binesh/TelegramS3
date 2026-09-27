# ADR 0015: Admin CSRF session resynchronization

## Status

Accepted

## Context

The authenticated admin SPA keeps the CSRF token returned at sign-in in
memory, while the server binds the matching token to an HTTP-only signed
session cookie. A session rotation in another tab, or another normal session
lifecycle event, can therefore leave an open page holding an older token. The
next mutating request is correctly rejected by the server as `invalid csrf
token`, but forcing the operator to reload the page is unnecessary and loses
the current UI context.

## Decision

- Keep the server-side cookie/header comparison and all existing CSRF checks
  unchanged.
- When the SPA receives exactly `403 invalid csrf token`, issue one guest-safe
  `GET /_admin/api/session` request without the stale CSRF header.
- Update the live Svelte session from that response and retry the original
  request once with the returned token. Share an in-flight synchronization
  request among concurrent failures to avoid a refresh stampede.
- Apply the same bounded recovery to raw object writes and XHR-based regular
  and resumable uploads, not only JSON API calls.
- Do not retry unrelated 403 responses, retry more than once, or recover when
  the session is no longer authenticated. In those cases the existing error
  remains visible and the operator must sign in again.

## Consequences

- A stale page can continue an action after another tab rotates the session,
  without weakening authentication or CSRF protection.
- A genuine expiry, revocation, or missing session remains an explicit failure.
- The recovery is control-plane behavior only and requires no metadata schema
  migration or object/Telegram format change.
