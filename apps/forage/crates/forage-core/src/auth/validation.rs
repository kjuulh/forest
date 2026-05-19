#[derive(Debug, PartialEq)]
pub struct ValidationError(pub String);

pub fn validate_email(email: &str) -> Result<(), ValidationError> {
    if email.is_empty() {
        return Err(ValidationError("Email is required".into()));
    }
    if !email.contains('@') || !email.contains('.') {
        return Err(ValidationError("Invalid email format".into()));
    }
    if email.len() > 254 {
        return Err(ValidationError("Email too long".into()));
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), ValidationError> {
    if password.is_empty() {
        return Err(ValidationError("Password is required".into()));
    }
    if password.len() < 12 {
        return Err(ValidationError(
            "Password must be at least 12 characters".into(),
        ));
    }
    if password.len() > 1024 {
        return Err(ValidationError("Password too long".into()));
    }
    if !password.chars().any(|c| c.is_uppercase()) {
        return Err(ValidationError(
            "Password must contain at least one uppercase letter".into(),
        ));
    }
    if !password.chars().any(|c| c.is_lowercase()) {
        return Err(ValidationError(
            "Password must contain at least one lowercase letter".into(),
        ));
    }
    if !password.chars().any(|c| c.is_ascii_digit()) {
        return Err(ValidationError(
            "Password must contain at least one digit".into(),
        ));
    }
    Ok(())
}

pub fn validate_username(username: &str) -> Result<(), ValidationError> {
    if username.is_empty() {
        return Err(ValidationError("Username is required".into()));
    }
    if username.len() < 3 {
        return Err(ValidationError(
            "Username must be at least 3 characters".into(),
        ));
    }
    if username.len() > 64 {
        return Err(ValidationError("Username too long".into()));
    }
    if !username
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ValidationError(
            "Username can only contain letters, numbers, hyphens, and underscores".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_email() {
        assert!(validate_email("user@example.com").is_ok());
        assert!(validate_email("a@b.c").is_ok());
    }

    #[test]
    fn invalid_email() {
        assert!(validate_email("").is_err());
        assert!(validate_email("noat").is_err());
        assert!(validate_email("no@dot").is_err());
        assert!(validate_email(&format!("{}@b.c", "a".repeat(251))).is_err());
    }

    #[test]
    fn valid_password() {
        assert!(validate_password("SecurePass123").is_ok());
        assert!(validate_password("MyLongPassphrase1").is_ok());
    }

    #[test]
    fn invalid_password() {
        assert!(validate_password("").is_err());
        assert!(validate_password("short").is_err());
        assert!(validate_password("12345678901").is_err()); // 11 chars
        assert!(validate_password(&"a".repeat(1025)).is_err());
        assert!(validate_password("alllowercase1").is_err()); // no uppercase
        assert!(validate_password("ALLUPPERCASE1").is_err()); // no lowercase
        assert!(validate_password("NoDigitsHere!").is_err()); // no digit
    }

    #[test]
    fn valid_username() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("bob-123").is_ok());
        assert!(validate_username("foo_bar").is_ok());
    }

    #[test]
    fn invalid_username() {
        assert!(validate_username("").is_err());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("has spaces").is_err());
        assert!(validate_username("has@symbol").is_err());
        assert!(validate_username(&"a".repeat(65)).is_err());
    }
}
