use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use rsa::pkcs1v15::{Signature as RsaSignature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::Verifier;
use rsa::RsaPublicKey;
use serde_json::Value as JsonValue;
use sha2::Sha256;
use zega_parser::value::Value;

use crate::{JwtConfig, JwtKey, Result, ZegaError};

type HmacSha256 = Hmac<Sha256>;

pub fn verify(token: &str, config: &JwtConfig) -> Result<HashMap<String, Value>> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(jwt_error("token must have exactly three parts"));
    }

    let header_bytes = decode_part(parts[0], "header")?;
    let payload_bytes = decode_part(parts[1], "payload")?;
    let signature = decode_part(parts[2], "signature")?;

    let header: JsonValue = serde_json::from_slice(&header_bytes)
        .map_err(|err| jwt_error(format!("invalid header json: {err}")))?;
    let payload: JsonValue = serde_json::from_slice(&payload_bytes)
        .map_err(|err| jwt_error(format!("invalid payload json: {err}")))?;

    let alg = header
        .get("alg")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| jwt_error("missing alg"))?;

    verify_registered_claims(&payload, config)?;
    verify_signature(alg, parts[0], parts[1], &signature, config)?;

    let JsonValue::Object(claims) = payload else {
        return Err(jwt_error("payload must be a json object"));
    };

    claims
        .into_iter()
        .map(|(key, value)| json_to_zega_value(value).map(|value| (key, value)))
        .collect()
}

fn decode_part(part: &str, name: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|err| jwt_error(format!("invalid {name} base64url: {err}")))
}

fn verify_registered_claims(payload: &JsonValue, config: &JwtConfig) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| jwt_error(format!("system time before unix epoch: {err}")))?
        .as_secs();

    if let Some(exp) = payload.get("exp") {
        let exp = exp
            .as_u64()
            .ok_or_else(|| jwt_error("exp must be an unsigned integer"))?;
        if now > exp.saturating_add(config.leeway_seconds) {
            return Err(jwt_error("token expired"));
        }
    }

    if let Some(expected_issuer) = &config.issuer {
        let Some(issuer) = payload.get("iss").and_then(JsonValue::as_str) else {
            return Err(jwt_error("missing issuer"));
        };
        if issuer != expected_issuer {
            return Err(jwt_error("issuer mismatch"));
        }
    }

    Ok(())
}

fn verify_signature(
    alg: &str,
    encoded_header: &str,
    encoded_payload: &str,
    signature: &[u8],
    config: &JwtConfig,
) -> Result<()> {
    let signing_input = format!("{encoded_header}.{encoded_payload}");

    match (alg, &config.key) {
        ("HS256", JwtKey::Hmac(secret)) => {
            let mut mac = HmacSha256::new_from_slice(secret)
                .map_err(|err| jwt_error(format!("invalid hmac key: {err}")))?;
            mac.update(signing_input.as_bytes());
            mac.verify_slice(signature)
                .map_err(|_| jwt_error("invalid signature"))
        }
        ("RS256", JwtKey::RsaPublicPem(pem)) => {
            let pem = std::str::from_utf8(pem)
                .map_err(|err| jwt_error(format!("invalid rsa public pem utf8: {err}")))?;
            let public_key = RsaPublicKey::from_public_key_pem(pem)
                .map_err(|err| jwt_error(format!("invalid rsa public pem: {err}")))?;
            let verifying_key = VerifyingKey::<Sha256>::new(public_key);
            let signature = RsaSignature::try_from(signature)
                .map_err(|err| jwt_error(format!("invalid rsa signature: {err}")))?;
            verifying_key
                .verify(signing_input.as_bytes(), &signature)
                .map_err(|_| jwt_error("invalid signature"))
        }
        ("HS256", JwtKey::RsaPublicPem(_)) => Err(jwt_error("HS256 requires an HMAC key")),
        ("RS256", JwtKey::Hmac(_)) => Err(jwt_error("RS256 requires an RSA public key")),
        _ => Err(jwt_error(format!("unsupported jwt alg: {alg}"))),
    }
}

fn json_to_zega_value(value: JsonValue) -> Result<Value> {
    match value {
        JsonValue::Null => Ok(Value::Null),
        JsonValue::Bool(value) => Ok(Value::Bool(value)),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::Int(value))
            } else if let Some(value) = value.as_u64() {
                i64::try_from(value)
                    .map(Value::Int)
                    .map_err(|_| jwt_error("unsigned claim value is too large"))
            } else if let Some(value) = value.as_f64() {
                Ok(Value::from_f64(value))
            } else {
                Err(jwt_error("unsupported numeric claim value"))
            }
        }
        JsonValue::String(value) => Ok(Value::String(value)),
        JsonValue::Array(values) => values
            .into_iter()
            .map(json_to_zega_value)
            .collect::<Result<Vec<_>>>()
            .map(Value::List),
        JsonValue::Object(values) => values
            .into_iter()
            .map(|(key, value)| json_to_zega_value(value).map(|value| (key, value)))
            .collect::<Result<HashMap<_, _>>>()
            .map(Value::Map),
    }
}

fn jwt_error(message: impl Into<String>) -> ZegaError {
    ZegaError::Jwt(message.into())
}
