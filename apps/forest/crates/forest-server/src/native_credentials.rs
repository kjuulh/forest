use std::sync::{Arc, LazyLock};

use argon2::{
    Argon2, Params, PasswordHash, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};

use crate::{
    State,
    native_credentials::requirements::{LowerCaseLetter, MinLength, UpperCaseLetter},
};

pub struct NativeCredentials {
    secret_key: Vec<u8>,
}

trait PasswordRequirement {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()>;
}

mod requirements;

#[derive(Debug, thiserror::Error)]
#[error("password did not meet requirements: {}", .0.join("; "))]
pub struct PasswordValidationError(pub Vec<String>);

impl NativeCredentials {
    pub fn password_fulfills_requirements(
        &self,
        password: &str,
    ) -> Result<(), PasswordValidationError> {
        let mut errors = Vec::new();

        for requirement in self.get_requirements() {
            if let Err(e) = requirement.fulfill_requirements(password) {
                errors.push(format!("{e}"));
            }
        }

        if errors.is_empty() {
            return Ok(());
        }

        Err(PasswordValidationError(errors))
    }

    pub fn hash(&self, password: impl Into<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
        let bytes = password.into();

        let salt = SaltString::generate(&mut OsRng);
        let a = Argon2::new_with_secret(
            &self.secret_key,
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            Params::DEFAULT,
        )
        .map_err(|e| anyhow::anyhow!("failed to build password hashing facility: {e:#}"))?;

        let output = PasswordHash::generate(a, bytes, &salt)
            .map_err(|e| anyhow::anyhow!("could not hash password: {e:#}"))?;

        Ok(output.serialize().to_string().into_bytes())
    }

    pub fn verify(&self, password: impl Into<Vec<u8>>, hash: &[u8]) -> anyhow::Result<()> {
        let bytes = password.into();

        let argon2 = Argon2::new_with_secret(
            &self.secret_key,
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            Params::DEFAULT,
        )
        .map_err(|e| anyhow::anyhow!("failed to build password hashing facility: {e:#}"))?;

        let password_hash = PasswordHash::new(std::str::from_utf8(hash)?)
            .map_err(|e| anyhow::anyhow!("failed to form password hash: {e:#}"))?;

        argon2
            .verify_password(&bytes, &password_hash)
            .map_err(|e| anyhow::anyhow!("password doesn't match: {e:#}"))?;

        Ok(())
    }

    fn get_requirements(&self) -> &[Arc<dyn PasswordRequirement + Send + Sync + 'static>] {
        static ONCE: LazyLock<Vec<Arc<dyn PasswordRequirement + Send + Sync + 'static>>> =
            LazyLock::new(|| {
                vec![
                    Arc::new(LowerCaseLetter),
                    Arc::new(UpperCaseLetter),
                    Arc::new(MinLength(12)),
                ]
            });

        &ONCE
    }
}

pub trait NativeCredentialsState {
    fn native_credentials(&self) -> NativeCredentials;
}

impl NativeCredentialsState for State {
    fn native_credentials(&self) -> NativeCredentials {
        NativeCredentials {
            secret_key: self.config.password_secret_key.clone().into_bytes(),
        }
    }
}
