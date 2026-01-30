use std::{
    collections::BTreeMap,
    ops::Add,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use hmac::{Hmac, Mac};
use jwt::{Header, SignWithKey, Token, VerifyWithKey};
use sha2::Sha384;

pub struct TokenService {
    secret: TokenSecret,
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

#[derive(Debug)]
pub struct AppClaims {
    pub user_id: String,
    pub session: String,
}

pub trait TokenServiceState {
    fn tokens(&self) -> TokenService;
}

impl TokenServiceState for TokenService {
    fn tokens(&self) -> TokenService {
        TokenService {
            secret: TokenSecret::SymmetricKey(vec![0u8]),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::tokens::TokenService;

    #[test]
    fn test_can_issue_token() -> anyhow::Result<()> {
        let id = uuid::Uuid::now_v7().to_string();

        let svc = TokenService {
            secret: super::TokenSecret::SymmetricKey(vec![0u8]),
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
}
