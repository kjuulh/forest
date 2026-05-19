use anyhow::bail;

use crate::native_credentials::PasswordRequirement;

// === Length (the most important one) ===

pub struct MinLength(pub usize);

impl PasswordRequirement for MinLength {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        if input.chars().count() >= self.0 {
            Ok(())
        } else {
            bail!("password must be at least {} characters", self.0)
        }
    }
}

pub struct MaxLength(usize);

impl PasswordRequirement for MaxLength {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        if input.chars().count() <= self.0 {
            Ok(())
        } else {
            bail!("password must be at most {} characters", self.0)
        }
    }
}

// === Character class requirements ===

pub struct LowerCaseLetter;

impl PasswordRequirement for LowerCaseLetter {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        if input.chars().any(|c| c.is_lowercase()) {
            Ok(())
        } else {
            bail!("password must contain at least one lowercase letter")
        }
    }
}

pub struct UpperCaseLetter;

impl PasswordRequirement for UpperCaseLetter {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        if input.chars().any(|c| c.is_uppercase()) {
            Ok(())
        } else {
            bail!("password must contain at least one uppercase letter")
        }
    }
}

pub struct Digit;

impl PasswordRequirement for Digit {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        if input.chars().any(|c| c.is_ascii_digit()) {
            Ok(())
        } else {
            bail!("password must contain at least one digit")
        }
    }
}

pub struct SpecialCharacter;

impl PasswordRequirement for SpecialCharacter {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        const SPECIAL: &str = "!@#$%^&*()_+-=[]{}|;':\",./<>?`~";
        if input.chars().any(|c| SPECIAL.contains(c)) {
            Ok(())
        } else {
            bail!("password must contain at least one special character")
        }
    }
}

// === Security requirements ===

pub struct NoWhitespace;

impl PasswordRequirement for NoWhitespace {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        if input.chars().any(|c| c.is_whitespace()) {
            bail!("password must not contain whitespace")
        } else {
            Ok(())
        }
    }
}

pub struct NoRepeatingChars(usize);

impl PasswordRequirement for NoRepeatingChars {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        let chars: Vec<_> = input.chars().collect();
        for window in chars.windows(self.0) {
            if window.iter().all(|&c| c == window[0]) {
                bail!(
                    "password must not contain {} or more repeating characters",
                    self.0
                );
            }
        }
        Ok(())
    }
}

pub struct NotCommonPassword;

impl PasswordRequirement for NotCommonPassword {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        // In production, use a proper list (e.g., Have I Been Pwned's top 100k)
        const COMMON: &[&str] = &[
            "password", "123456", "12345678", "qwerty", "abc123", "monkey", "1234567", "letmein",
            "trustno1", "dragon", "baseball", "iloveyou", "master", "sunshine", "ashley",
            "passw0rd", "shadow", "123123", "654321", "superman",
        ];
        let lower = input.to_lowercase();
        if COMMON.contains(&lower.as_str()) {
            bail!("password is too common")
        } else {
            Ok(())
        }
    }
}

pub struct NotContainsUsername<'a>(&'a str);

impl PasswordRequirement for NotContainsUsername<'_> {
    fn fulfill_requirements(&self, input: &str) -> anyhow::Result<()> {
        if self.0.len() >= 3 && input.to_lowercase().contains(&self.0.to_lowercase()) {
            bail!("password must not contain your username")
        } else {
            Ok(())
        }
    }
}
