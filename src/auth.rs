//! Operator-account authentication helpers for the `/_admin` surface.
//!
//! Password rows live in the shared `metadata.sqlite` (see `metadata::users`),
//! argon2id-hashed as PHC strings. This module is deliberately free of HTTP /
//! hyper concerns so both the admin HTTP layer and the `users` CLI share it.
//! Cookie enrolment, CSRF and transport stay in `admin.rs`; session/account
//! storage queries stay in `metadata.rs`.

use crate::metadata::{DbUser, MetadataStore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use http::StatusCode;
use rand_core::OsRng;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_SUPERADMIN: &str = "superadmin";

const ARGON2_M_COST: u32 = 65_536;
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 4;

const MIN_PASSWORD_LENGTH: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthErrorKind {
    InvalidUsername,
    WeakPassword,
    UnknownUser,
    InvalidPassword,
    Disabled,
    LockedOut,
    RateLimited,
    UsernameTaken,
    NotAllowed,
    Internal,
}

#[derive(Debug)]
pub struct AuthError {
    pub kind: AuthErrorKind,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl AuthError {
    fn new(kind: AuthErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    pub fn http_status(&self) -> StatusCode {
        match self.kind {
            AuthErrorKind::RateLimited | AuthErrorKind::LockedOut => StatusCode::TOO_MANY_REQUESTS,
            AuthErrorKind::NotAllowed => StatusCode::FORBIDDEN,
            AuthErrorKind::UsernameTaken => StatusCode::CONFLICT,
            AuthErrorKind::InvalidUsername | AuthErrorKind::WeakPassword => StatusCode::BAD_REQUEST,
            AuthErrorKind::UnknownUser
            | AuthErrorKind::InvalidPassword
            | AuthErrorKind::Disabled => StatusCode::UNAUTHORIZED,
            AuthErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<crate::metadata::MetadataError> for AuthError {
    fn from(error: crate::metadata::MetadataError) -> Self {
        internal(format!("store error: {error}"))
    }
}

fn internal(message: impl Into<String>) -> AuthError {
    AuthError::new(AuthErrorKind::Internal, message)
}

fn argon2_params() -> Params {
    // OWASP-recommended argon2id baseline for an interactive-login control plane.
    Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, None).expect("argon2 params are valid")
}

/// Normalize a username for storage/comparison: trim and lowercase.
pub fn normalize_username(raw: &str) -> Result<String, AuthError> {
    let value = raw.trim().to_lowercase();
    if value.is_empty()
        || value.len() > 64
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(AuthError::new(
            AuthErrorKind::InvalidUsername,
            "username must be 1-64 non-control characters",
        ));
    }
    Ok(value)
}

/// Enforce a password policy server-side. We keep it deliberately simple and
/// strict on length; composition rules provide little value over length and
/// length minimums are the best-supported signal.
pub fn validate_password(password: &str) -> Result<(), AuthError> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(AuthError::new(
            AuthErrorKind::WeakPassword,
            "password must be at least 12 characters",
        ));
    }
    Ok(())
}

pub fn is_superadmin(user: &DbUser) -> bool {
    user.role == ROLE_SUPERADMIN
}

pub fn is_active(user: &DbUser) -> bool {
    !user.disabled
}

fn validate_role(role: &str) -> bool {
    matches!(role, ROLE_ADMIN | ROLE_SUPERADMIN)
}

/// Always-run a real argon2 verification so unknown usernames cost the same
/// work as a true password check (mitigates username enumeration via timing).
/// The dummy hash is computed once and reused.
fn burn_password_check() {
    let _ = verify_password("timing-equalizer-pw", burn_dummy_phc());
}

static BURN_DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn burn_dummy_phc() -> &'static str {
    BURN_DUMMY.get_or_init(|| {
        hash_password("dummy-equalizer-secret").unwrap_or_else(|_| FAILBACK_PHC.to_string())
    })
}

const FAILBACK_PHC: &str =
    "$argon2id$v=19$m=65536,t=3,p=4$c2FsdHNhbHRzYWx0cw$DpGpWfHlQrJONlJvgl9y2B4pdBpGm7nBQ8QGkMciN6c";

/// Hash a password into a PHC argon2id string. CPU-heavy: call on a blocking
/// thread when used from the async HTTP path.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params());
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| internal(format!("password hashing failed: {error}")))
}

/// Constant-time-ish verify; the mismatch result is trivial to return.
pub fn verify_password(password: &str, phc: &str) -> Result<(), AuthError> {
    let parsed = PasswordHash::new(phc)
        .map_err(|_| AuthError::new(AuthErrorKind::UnknownUser, "invalid stored credential"))?;
    let argon2 = Argon2::default();
    match argon2.verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(()),
        Err(argon2::password_hash::Error::Password) => Err(AuthError::new(
            AuthErrorKind::InvalidPassword,
            "invalid username or password",
        )),
        Err(error) => Err(internal(format!("password verification failed: {error}"))),
    }
}

/// Create an operator account. The first account ever created is forced to be
/// the superadmin (the CLI is the privileged path that seeds it). Subsequent
/// accounts may be any valid role and are normally created by an admin via the
/// API, which passes the authenticated caller's allowance explicitly via
/// `role` + this function's caller-supplied intent.
pub fn create_account(
    store: &MetadataStore,
    raw_username: &str,
    password: &str,
    requested_role: &str,
    display_name: &str,
) -> Result<DbUser, AuthError> {
    let username = normalize_username(raw_username)?;
    validate_password(password)?;
    if !validate_role(requested_role) {
        return Err(AuthError::new(
            AuthErrorKind::NotAllowed,
            "unknown role requested",
        ));
    }
    if store.get_user(&username)?.is_some() {
        return Err(AuthError::new(
            AuthErrorKind::UsernameTaken,
            "username taken",
        ));
    }
    let first = store.user_count()? == 0;
    // The very first operator must carry superadmin privileges so the system
    // can never be stranded without a privileged account.
    let role = if first {
        ROLE_SUPERADMIN.to_string()
    } else {
        requested_role.to_string()
    };
    let id = uuid::Uuid::new_v4().to_string();
    let hash = hash_password(password)?;
    store
        .create_user(&id, &username, &hash, &role, display_name)
        .map_err(|error| internal(format!("failed to store user: {error}")))?;
    store
        .get_user(&username)?
        .ok_or_else(|| internal("created user is unreadable"))
}

/// Authenticate an account by username + password. On success the returned row
/// is the enabled account (enrolment of the HTTP session is the caller's step).
pub fn authenticate(
    store: &MetadataStore,
    raw_username: &str,
    password: &str,
) -> Result<DbUser, AuthError> {
    let username = match normalize_username(raw_username) {
        Ok(username) => username,
        Err(error) => {
            burn_password_check();
            return Err(error);
        }
    };
    let Some(user) = store.get_user(&username)? else {
        burn_password_check();
        return Err(AuthError::new(
            AuthErrorKind::UnknownUser,
            "invalid username or password",
        ));
    };
    verify_password(password, &user.password_hash)?;
    if user.disabled {
        return Err(AuthError::new(
            AuthErrorKind::Disabled,
            "account is disabled",
        ));
    }
    Ok(user)
}

/// Change an account's password, bumping its token_version so every session it
/// issued before is invalid regardless of principal-side logout bookkeeping.
pub fn change_password(
    store: &MetadataStore,
    user_id: &str,
    new_password: &str,
) -> Result<(), AuthError> {
    validate_password(new_password)?;
    let hash = hash_password(new_password)?;
    store
        .set_user_password(user_id, &hash)
        .map_err(|error| internal(format!("failed to update password: {error}")))?;
    store
        .revoke_user_sessions(user_id)
        .map_err(|error| internal(format!("failed to revoke sessions: {error}")))
}

// ---- In-process login rate limiting / lockout -------------------------------
//
// Chosen over `governor`: this control plane is a single process behind one
// writer, the surface is just the login endpoint, and we avoid a heavyweight
// dependency. Buckets are fixed-window + escalating lockout. In-memory state is
// acceptable for the login rate-limit posture of a single-instance admin app,
// and it cannot be bypassed by restarting the process at a meaningful level
// (lockouts have short durations).

pub struct LoginLimiter {
    buckets: Arc<RwLock<HashMap<String, Bucket>>>,
}

struct Bucket {
    window_started: Instant,
    fails: u32,
    lock_step: u32,
    locked_until: Option<Instant>,
}

const WINDOW: Duration = Duration::from_secs(300);
const MAX_FAILS_IN_WINDOW: u32 = 8;
const LOCK_STEP_CEILING_SECS: u64 = 900;

impl Clone for LoginLimiter {
    fn clone(&self) -> Self {
        Self {
            buckets: Arc::clone(&self.buckets),
        }
    }
}

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn lock_for_step(step: u32) -> Duration {
        let secs = 30_u64.saturating_mul(1_u64 << step.min(10));
        Duration::from_secs(secs.min(LOCK_STEP_CEILING_SECS))
    }

    /// Record a successful login for the given keys (reset their windows).
    pub fn reset(&self, keys: &[String]) {
        if let Ok(mut buckets) = self.buckets.write() {
            for key in keys {
                buckets.remove(key);
            }
        }
    }

    /// Record a failed login for the given keys, escalating toward lockout.
    pub fn record_failure(&self, keys: &[String]) {
        if let Ok(mut buckets) = self.buckets.write() {
            for key in keys {
                let now = Instant::now();
                let bucket = buckets.entry(key.clone()).or_insert_with(|| Bucket {
                    window_started: now,
                    fails: 0,
                    lock_step: 0,
                    locked_until: None,
                });
                if now.duration_since(bucket.window_started) >= WINDOW {
                    bucket.window_started = now;
                    bucket.fails = 0;
                    bucket.lock_step = 0;
                    bucket.locked_until = None;
                }
                bucket.fails += 1;
                if bucket.fails >= MAX_FAILS_IN_WINDOW
                    && bucket.locked_until.is_none_or(|when| when <= now)
                {
                    bucket.locked_until = Some(now + LoginLimiter::lock_for_step(bucket.lock_step));
                    bucket.lock_step += 1;
                }
            }
        }
    }

    /// Return None when the request is allowed, or Some(kind) when blocked.
    pub fn check(&self, keys: &[String]) -> Option<AuthError> {
        let buckets = self.buckets.read().ok()?;
        let now = Instant::now();
        let mut blocked_until = None;
        for key in keys {
            if let Some(bucket) = buckets.get(key) {
                if let Some(locked_until) = bucket.locked_until {
                    if locked_until > now {
                        let remaining = locked_until.duration_since(now).as_secs().max(1);
                        blocked_until =
                            Some(blocked_until.map_or(remaining, |b: u64| b.max(remaining)));
                    }
                } else if now.duration_since(bucket.window_started) < WINDOW
                    && bucket.fails >= MAX_FAILS_IN_WINDOW
                {
                    blocked_until = Some(
                        blocked_until.map_or(WINDOW.as_secs(), |b: u64| b.max(WINDOW.as_secs())),
                    );
                }
            }
        }
        blocked_until.map(|retry_after_secs| AuthError {
            kind: AuthErrorKind::RateLimited,
            message: "too many login attempts, try again shortly".to_string(),
            retry_after_secs: Some(retry_after_secs),
        })
    }

    /// Convenience: produce the keys to bucket for a given client address +
    /// attempted username.
    pub fn keys_for(ip: Option<IpAddr>, username: &str) -> Vec<String> {
        let mut keys = Vec::with_capacity(2);
        if let Some(ip) = ip {
            keys.push(format!("ip:{ip}"));
        }
        if !username.is_empty() {
            keys.push(format!("user:{}", username.trim().to_lowercase()));
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowercases_and_trims() {
        assert_eq!(normalize_username("  Alice  ").unwrap(), "alice");
        assert!(normalize_username("").is_err());
        assert!(normalize_username("a b").is_err());
        assert!(normalize_username(&"x".repeat(65)).is_err());
    }

    #[test]
    fn password_policy_rejects_short() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("a-long-enough-pw").is_ok());
    }

    #[test]
    fn argon2_hash_and_verify_roundtrip() {
        let hash = hash_password("correct horse battery staple").expect("hash");
        assert!(hash.starts_with("$argon2id$"));
        verify_password("correct horse battery staple", &hash).expect("correct pw ok");
        assert!(verify_password("wrong password value", &hash).is_err());
    }

    #[test]
    fn login_limiter_locks_after_too_many_then_resets() {
        let limiter = LoginLimiter::new();
        let keys = LoginLimiter::keys_for(None, "alice");
        for _ in 0..10 {
            limiter.record_failure(&keys);
        }
        assert!(limiter.check(&keys).is_some(), "should be rate-limited");

        // A successful login resets the account bucket.
        limiter.reset(&keys);
        assert!(limiter.check(&keys).is_none());
    }

    #[test]
    fn first_account_is_forced_superadmin() {
        let store = crate::metadata::MetadataStore::open_in_memory().expect("store");
        let first = create_account(
            &store,
            "root",
            "correct-horse-battery-staple",
            ROLE_ADMIN,
            "Boot",
        )
        .expect("create");
        assert_eq!(first.role, ROLE_SUPERADMIN);

        let second = create_account(&store, "peer", "another-correct-passphrase", ROLE_ADMIN, "")
            .expect("peer");
        assert_eq!(second.role, ROLE_ADMIN);
    }
}
