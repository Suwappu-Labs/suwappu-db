//! **B6** — opt-in bearer-token auth for the JSON-RPC surface.
//!
//! Default off: with no `SUWAPPUDB_BEARER_TOKEN` env var set, the
//! middleware lets every request through unchanged. Set the env to a
//! shared secret to require `Authorization: Bearer <token>` on
//! authenticated routes.
//!
//! Deliberately small. The B6 audit verdict is that the production
//! deployment posture is firewall-or-ALB — the bearer token is a
//! second layer, not a primary access control. Operators who pin
//! `SUWAPPUDB_BEARER_TOKEN` get a hard reject for any request without a
//! matching token; everyone else preserves the pre-B6 behaviour.
//!
//! See `docs/architecture/deployment-topology.md` for the firewall /
//! Cloudflare-Access deployment patterns this complements.

use axum::{
    body::Body,
    extract::State as AxumState,
    http::{header::AUTHORIZATION, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::env;
use std::sync::Arc;

/// Per-process bearer-auth configuration. `None` = auth disabled.
#[derive(Clone, Debug, Default)]
pub struct BearerAuthConfig {
    expected_token: Option<Arc<String>>,
}

impl BearerAuthConfig {
    /// Read `SUWAPPUDB_BEARER_TOKEN` from the environment. If unset (or
    /// empty), returns a `disabled` config that lets every request
    /// through.
    #[must_use]
    pub fn from_env() -> Self {
        match env::var("SUWAPPUDB_BEARER_TOKEN") {
            Ok(t) if !t.is_empty() => Self::with_token(t),
            _ => Self::default(),
        }
    }

    /// Explicit-token constructor — used by integration tests that
    /// don't want to mutate the process-wide environment, and by any
    /// future caller that loads the token from a non-env source.
    #[must_use]
    pub fn with_token(token: impl Into<String>) -> Self {
        Self {
            expected_token: Some(Arc::new(token.into())),
        }
    }

    /// `true` iff a token was configured. Operators can call this at
    /// startup to log the security posture.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.expected_token.is_some()
    }
}

/// Axum middleware: gates every request behind a matching `Authorization:
/// Bearer <token>` header when `BearerAuthConfig::is_enabled()`.
///
/// - Disabled config → request passes through.
/// - Missing header → `401 Unauthorized`.
/// - Wrong scheme or token → `401 Unauthorized`.
///
/// **Constant-time comparison.** Token equality uses `subtle::ConstantTimeEq`
/// to deny timing side-channels. (`subtle` is already pulled in
/// transitively by the curve crates; if it ever drops we have a
/// fallback locally.)
pub async fn bearer_auth(
    AxumState(config): AxumState<BearerAuthConfig>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = config.expected_token.as_deref() else {
        // Auth disabled — pre-B6 behaviour.
        return Ok(next.run(req).await);
    };

    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if constant_time_eq(token.as_bytes(), expected.as_bytes()) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Constant-time byte comparison. Length difference short-circuits
/// (length is not secret), then byte-wise OR-accumulation.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: env-var tests for `from_env` would require `unsafe` blocks
    // around `std::env::set_var` (Rust 1.86+ marks them unsafe), which
    // the workspace lint `unsafe_code = "forbid"` rejects. The
    // `with_token` constructor is the unit-testable surface;
    // `from_env` is exercised via the integration test that drives
    // the binary with the env var set.

    #[test]
    fn default_config_is_disabled() {
        assert!(!BearerAuthConfig::default().is_enabled());
    }

    #[test]
    fn with_token_enables_auth() {
        assert!(BearerAuthConfig::with_token("test-token").is_enabled());
    }

    #[test]
    fn constant_time_eq_matches() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn constant_time_eq_rejects_mismatch() {
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
    }
}
