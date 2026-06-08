use axum::http::{header::AUTHORIZATION, HeaderMap};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

pub fn authorized(headers: &HeaderMap, expected_hash: &[u8; 32]) -> bool {
    let Some(token) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    bool::from(hash_token(token).ct_eq(expected_hash))
}
