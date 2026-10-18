//! Passwords (argon2id), tokens (random, stored as SHA-256), and the
//! `AuthUser` extractor for bearer-authenticated requests.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use http::request::Parts;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::db::{self, Db, User};
use crate::error::ApiError;
use crate::state::AppState;

pub const PAT_PREFIX: &str = "kwr_";

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let hash: PasswordHash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

/// A fresh random token (hex) with an optional prefix.
pub fn new_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub token_id: String,
    pub token_kind: String,
}

/// Extracts the bearer token from `Authorization` (or a `token` query param).
pub fn bearer(parts: &Parts) -> Option<String> {
    if let Some(v) = parts.headers.get(http::header::AUTHORIZATION)
        && let Ok(s) = v.to_str()
        && let Some(t) = s
            .strip_prefix("Bearer ")
            .or_else(|| s.strip_prefix("bearer "))
    {
        return Some(t.trim().to_string());
    }
    parts.uri.query().and_then(|q| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == "token")
            .map(|(_, v)| v.to_string())
    })
}

pub fn authenticate(db: &Db, token: &str) -> Result<AuthUser, ApiError> {
    let conn = db.lock();
    let row = db::token_by_hash(&conn, &hash_token(token))?.ok_or_else(ApiError::unauthorized)?;
    Ok(AuthUser {
        user: row.user,
        token_id: row.id,
        token_kind: row.kind,
    })
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(user) = parts.extensions.get::<AuthUser>() {
            return Ok(user.clone());
        }
        let token = bearer(parts).ok_or_else(ApiError::unauthorized)?;
        authenticate(&state.db, &token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn tokens_are_unique_and_hashed() {
        let a = new_token(PAT_PREFIX);
        assert!(a.starts_with(PAT_PREFIX));
        assert_ne!(a, new_token(PAT_PREFIX));
        assert_eq!(hash_token(&a).len(), 64);
    }
}
