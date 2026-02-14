/// Typed database errors for constraint violations.
///
/// Implements `std::error::Error` (via `thiserror`), making it anyhow-compatible.
/// Repository write methods return `Result<T, DbError>` so that constraint
/// violations are classified at the boundary. The gRPC layer can then downcast
/// `anyhow::Error` to `DbError` and map to the correct `tonic::Status`.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Duplicate key / unique constraint violation (PG 23505).
    #[error("{0}")]
    AlreadyExists(String),

    /// Foreign key violation — referenced row doesn't exist (PG 23503).
    #[error("{0}")]
    ReferenceNotFound(String),

    /// Not-null or check constraint violation (PG 23502, 23514).
    #[error("{0}")]
    ConstraintViolation(String),

    /// Any other database error.
    #[error(transparent)]
    Other(sqlx::Error),
}

impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::Database(db_err) => {
                let code = db_err.code();
                let constraint = db_err.constraint();

                match code.as_deref() {
                    Some("23505") => {
                        DbError::AlreadyExists(constraint_message_unique(constraint))
                    }
                    Some("23503") => {
                        DbError::ReferenceNotFound(constraint_message_fk(constraint))
                    }
                    Some("23502") | Some("23514") => {
                        DbError::ConstraintViolation(constraint_message_generic(constraint))
                    }
                    _ => DbError::Other(err),
                }
            }
            _ => DbError::Other(err),
        }
    }
}

/// Friendly message for unique-constraint violations.
fn constraint_message_unique(constraint: Option<&str>) -> String {
    match constraint {
        Some("organisation_members_pkey") => {
            "member already exists in this organisation".to_string()
        }
        Some("organisations_name_key") => "organisation name already taken".to_string(),
        Some("users_username_key") => "username already taken".to_string(),
        Some("user_emails_email_key") | Some("user_emails_pkey") => {
            "email already in use".to_string()
        }
        Some(name) => format!("resource already exists ({name})"),
        None => "resource already exists".to_string(),
    }
}

/// Friendly message for foreign-key violations.
fn constraint_message_fk(constraint: Option<&str>) -> String {
    match constraint {
        Some("fk_projects_organisation") | Some("fk_destinations_organisation") => {
            "organisation does not exist".to_string()
        }
        Some(name) => format!("referenced resource not found ({name})"),
        None => "referenced resource not found".to_string(),
    }
}

/// Friendly message for other constraint violations.
fn constraint_message_generic(constraint: Option<&str>) -> String {
    match constraint {
        Some(name) => format!("constraint violation ({name})"),
        None => "constraint violation".to_string(),
    }
}
