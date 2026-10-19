use crate::native_credentials::PasswordValidationError;
use crate::repositories::error::DbError;
use crate::services::users::UserServiceError;

pub fn to_status(err: anyhow::Error) -> tonic::Status {
    if let Some(db_err) = err.downcast_ref::<DbError>() {
        return match db_err {
            DbError::AlreadyExists(msg) => tonic::Status::already_exists(msg.as_str()),
            DbError::ReferenceNotFound(msg) => tonic::Status::not_found(msg.as_str()),
            DbError::ConstraintViolation(msg) => tonic::Status::invalid_argument(msg.as_str()),
            DbError::Other(_) => {
                tracing::warn!("database error: {err:#}");
                tonic::Status::internal("internal error")
            }
        };
    }

    if let Some(pwd_err) = err.downcast_ref::<PasswordValidationError>() {
        return tonic::Status::invalid_argument(pwd_err.to_string());
    }

    if let Some(svc_err) = err.downcast_ref::<UserServiceError>() {
        return match svc_err {
            UserServiceError::LastAuthMethod => {
                tonic::Status::failed_precondition(svc_err.to_string())
            }
        };
    }

    if let Some(oauth_err) = err.downcast_ref::<crate::services::oauth_apps::OAuthAppError>() {
        return tonic::Status::invalid_argument(oauth_err.to_string());
    }

    tracing::warn!("service error: {err:#}");
    tonic::Status::internal("internal error")
}
