use std::collections::HashMap;

use zega_parser::value::Value;

use crate::{jwt, JwtConfig, Result};

#[derive(Clone, Debug)]
pub enum ZegaContext {
    Anonymous,
    System,
    Claims(HashMap<String, Value>),
    Jwt(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedContext {
    pub claims: HashMap<String, Value>,
    pub is_system: bool,
    pub is_anonymous: bool,
}

impl ZegaContext {
    pub fn anonymous() -> Self {
        Self::Anonymous
    }

    pub fn system() -> Self {
        Self::System
    }

    pub fn claims(map: HashMap<String, Value>) -> Self {
        Self::Claims(map)
    }

    pub fn jwt(token: &str) -> Self {
        Self::Jwt(token.to_string())
    }

    pub fn resolve(&self, config: &JwtConfig) -> Result<ResolvedContext> {
        match self {
            Self::Anonymous => Ok(ResolvedContext {
                claims: HashMap::new(),
                is_system: false,
                is_anonymous: true,
            }),
            Self::System => Ok(ResolvedContext {
                claims: HashMap::new(),
                is_system: true,
                is_anonymous: false,
            }),
            Self::Claims(claims) => Ok(ResolvedContext {
                claims: claims.clone(),
                is_system: false,
                is_anonymous: false,
            }),
            Self::Jwt(token) => Ok(ResolvedContext {
                claims: jwt::verify(token, config)?,
                is_system: false,
                is_anonymous: false,
            }),
        }
    }
}
