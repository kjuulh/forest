use std::{
    collections::BTreeMap,
    ops::Add,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    AeadCore, Aes256Gcm, Nonce,
    aead::{Aead, OsRng},
};
use anyhow::Context;
use base64::{Engine, prelude::BASE64_STANDARD};
use hmac::{Hmac, Mac};
use jwt::{Header, SignWithKey, Token, VerifyWithKey};
use sha2::{Digest, Sha384};

use crate::State;

pub struct TokenService {
    secret: TokenSecret,

    refresh_token_secret: TokenSecret,
}

pub enum TokenSecret {
    SymmetricKey(Vec<u8>),
}

impl TokenSecret {
    pub fn get_private_key(&self) -> anyhow::Result<Hmac<Sha384>> {
        match self {
            TokenSecret::SymmetricKey(items) => {
                Hmac::new_from_slice(items).context("failed to create hmac from slice")
            }
        }
    }

    pub fn get_raw(&self) -> &Vec<u8> {
        match self {
            TokenSecret::SymmetricKey(items) => items,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessToken {
    content: Vec<u8>,
}

impl AccessToken {
    pub fn as_string(&self) -> String {
        String::from_utf8(self.content.clone()).expect("string needs to be valid utf-8")
    }
    pub fn new_from(input: &str) -> anyhow::Result<AccessToken> {
        Ok(AccessToken {
            content: input.as_bytes().to_vec(),
        })
    }
}

impl TokenService {
    pub fn generate_refresh_token(&self) -> anyhow::Result<(String, Vec<u8>)> {
        // Generate hash
        let mut buf = [0u8; 64]; // 64 bytes of randomness
        rand::fill(&mut buf[..]);
        let hash = sha2::Sha256::digest(buf).to_vec();

        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

        let cipher =
            <Aes256Gcm as aes_gcm::KeyInit>::new_from_slice(self.refresh_token_secret.get_raw())
                .context("key must be 32 bytes")?;

        let cipher_text = cipher
            .encrypt(&nonce, &buf[..])
            .map_err(|e| anyhow::anyhow!("encrypt refresh token: {e:#}"))?;

        // Concat output = protocol || nonce || cipher
        let mut output = vec![0, 0, 1]; // 1 is protocol = aesgcm256, 0, 0 for future usage
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&cipher_text);

        let token = BASE64_STANDARD.encode(output);

        Ok((token, hash))
    }

    pub fn get_token_hash(&self, refresh_token: &str) -> anyhow::Result<Vec<u8>> {
        let refresh_token = BASE64_STANDARD.decode(refresh_token)?;

        let (protocol, rest) = refresh_token
            .split_at_checked(3)
            .context("refresh token doesn't contain a protocol")?; // protocol (first 3 bytes)

        if protocol[2] != 1 {
            // 0 0 1 == aesgcm256
            anyhow::bail!("protocol version not supported")
        }

        // aesgcm258
        let (nonce, ciphertext) = rest.split_at_checked(12).context("invalid ciphertext")?; // nonce is 12 bytes long for gcm
        let cipher =
            <Aes256Gcm as aes_gcm::KeyInit>::new_from_slice(self.refresh_token_secret.get_raw())?;

        let nonce = Nonce::from_slice(nonce);

        let raw = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("failed to descrypt refresh token: {e:#}"))?;

        Ok(sha2::Sha256::digest(raw).to_vec())
    }

    pub fn issue_access_token(
        &self,
        user_id: &str,
        session_id: &str,
        scopes: Vec<String>,
    ) -> anyhow::Result<AccessToken> {
        let hmac = self.secret.get_private_key()?;

        let header = Header {
            algorithm: jwt::AlgorithmType::Hs384,
            ..Default::default()
        };

        let now = &SystemTime::now().duration_since(UNIX_EPOCH)?;
        let expire = &SystemTime::now()
            .add(Duration::from_hours(24 * 30 * 3))
            .duration_since(UNIX_EPOCH)?;
        let now_str = now.as_secs().to_string();
        let expire_str = expire.as_secs().to_string();
        let not_before_str = now.as_secs().to_string();
        let id = uuid::Uuid::now_v7().to_string();

        let mut claims = BTreeMap::new();
        claims.insert("sub", user_id);
        claims.insert("iat", &now_str);
        claims.insert("exp", &expire_str);
        claims.insert("nbf", &not_before_str);
        claims.insert("jti", &id);
        claims.insert("sid", session_id);

        let scope = scopes.join(" ");
        claims.insert("scope", &scope);

        let access_token = Token::new(header, claims)
            .sign_with_key(&hmac)
            .context("sign with key")?;

        AccessToken::new_from(access_token.as_str())
    }

    pub fn verify_access_token(&self, access_token: &AccessToken) -> anyhow::Result<AppClaims> {
        let hmac = self
            .secret
            .get_private_key()
            .context("could not get private key")?;

        let token: Token<Header, BTreeMap<String, String>, _> = access_token
            .as_string()
            .verify_with_key(&hmac)
            .context("could not verify token")?;

        let claims = token.claims().clone();

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;

        let nbf = claims.get("nbf").context("could not find not before")?;
        let nbf = Duration::from_secs(nbf.parse::<u64>()?);

        if now < nbf {
            anyhow::bail!("token was used before it was valid")
        }

        let exp = claims.get("exp").context("could not find expiry")?;
        let exp = Duration::from_secs(exp.parse::<u64>()?);

        if now > exp {
            anyhow::bail!("token has expired")
        }

        Ok(AppClaims {
            user_id: claims.get("sub").context("failed to find user id")?.clone(),
            session: claims
                .get("sid")
                .context("failed to get session id")?
                .clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct AppClaims {
    pub user_id: String,
    pub session: String,
}

pub trait TokenServiceState {
    fn tokens(&self) -> TokenService;
}

impl TokenServiceState for State {
    fn tokens(&self) -> TokenService {
        TokenService {
            secret: TokenSecret::SymmetricKey(self.config.access_token_secret_key.clone()),
            refresh_token_secret: TokenSecret::SymmetricKey(
                self.config.refresh_token_secret_key.clone(),
            ),
        }
    }
}

#[cfg(test)]
mod test {
    use base64::Engine;

    use crate::tokens::{TokenSecret, TokenService};

    #[test]
    fn test_can_issue_token() -> anyhow::Result<()> {
        let id = uuid::Uuid::now_v7().to_string();

        let svc = TokenService {
            secret: super::TokenSecret::SymmetricKey(vec![0u8]),
            refresh_token_secret: TokenSecret::SymmetricKey(vec![0u8; 64]),
        };

        let tkn = svc.issue_access_token(&uuid::Uuid::now_v7().to_string(), &id, vec![])?;

        dbg!(tkn);

        Ok(())
    }

    #[test]
    fn test_can_verify_token() -> anyhow::Result<()> {
        let id = uuid::Uuid::now_v7().to_string();

        let svc = TokenService {
            secret: super::TokenSecret::SymmetricKey(vec![0u8]),
            refresh_token_secret: TokenSecret::SymmetricKey(vec![0u8; 64]),
        };

        let tkn = svc.issue_access_token(
            &uuid::Uuid::now_v7().to_string(),
            &id,
            vec!["project:admin".into(), "component:read".into()],
        )?;
        dbg!(&tkn.as_string());

        let claims = svc
            .verify_access_token(&tkn)
            .inspect_err(|e| println!("{e:#}"))?;

        dbg!(&claims);

        Ok(())
    }

    #[test]
    fn test_encrypt_and_decrypt() -> anyhow::Result<()> {
        let svc = TokenService {
            secret: super::TokenSecret::SymmetricKey(vec![0u8]),
            refresh_token_secret: TokenSecret::SymmetricKey(vec![0u8; 32]),
        };

        let (token, hash) = svc.generate_refresh_token()?;

        let output_hash = svc.get_token_hash(&token)?;

        println!("token: {}", &token);
        println!("hash:  {}", base64::prelude::BASE64_STANDARD.encode(&hash));

        let token = base64::prelude::BASE64_STANDARD.decode(token)?;

        assert_eq!(hash, output_hash);
        assert_eq!([0, 0, 1], token[..3]);
        assert_eq!(32, output_hash.len());

        Ok(())
    }
}
