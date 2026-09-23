#[derive(Clone, Debug)]
pub enum JwtKey {
    Hmac(Vec<u8>),
    RsaPublicPem(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct JwtConfig {
    pub key: JwtKey,
    pub issuer: Option<String>,
    pub leeway_seconds: u64,
}

impl JwtConfig {
    pub fn hmac(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            key: JwtKey::Hmac(secret.into()),
            issuer: None,
            leeway_seconds: 0,
        }
    }

    pub fn rsa_public_pem(pem: impl Into<Vec<u8>>) -> Self {
        Self {
            key: JwtKey::RsaPublicPem(pem.into()),
            issuer: None,
            leeway_seconds: 0,
        }
    }
}
