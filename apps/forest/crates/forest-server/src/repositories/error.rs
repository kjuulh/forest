#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("{0}")]
    AlreadyExists(String),

    #[error("{0}")]
    ReferenceNotFound(String),

    #[error("{0}")]
    ConstraintViolation(String),

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
                    Some("23505") => DbError::AlreadyExists(constraint_message_unique(constraint)),
                    Some("23503") => DbError::ReferenceNotFound(constraint_message_fk(constraint)),
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
        Some("identities_provider_external_id_key") => {
            "this external account is already linked to another user".to_string()
        }
        Some("identities_user_provider_key") => {
            "user already has an account linked for this provider".to_string()
        }
        Some(name) => format!("resource already exists ({name})"),
        None => "resource already exists".to_string(),
    }
}

fn constraint_message_fk(constraint: Option<&str>) -> String {
    match constraint {
        Some("fk_projects_organisation") | Some("fk_destinations_organisation") => {
            "organisation does not exist".to_string()
        }
        Some(name) => format!("referenced resource not found ({name})"),
        None => "referenced resource not found".to_string(),
    }
}

fn constraint_message_generic(constraint: Option<&str>) -> String {
    match constraint {
        Some(name) => format!("constraint violation ({name})"),
        None => "constraint violation".to_string(),
    }
}
