use crate::native_credentials::PasswordValidationError;
use crate::repositories::error::DbError;
use crate::services::users::UserServiceError;

/// Converts an `anyhow::Error` from the service layer into the appropriate
/// `tonic::Status`. Database constraint errors (carried as `DbError` inside
/// the anyhow chain) are mapped to specific gRPC status codes with safe
/// user-facing messages. Unknown errors are logged and returned as a generic
/// "internal error" to avoid leaking implementation details.
pub fn to_status(err: anyhow::Error) -> tonic::Status {
    // Check for typed database errors first.
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

    // Password validation failures are user-actionable; surface the rule list
    // so the CLI can show "password must contain at least one uppercase letter"
    // instead of "internal error".
    if let Some(pwd_err) = err.downcast_ref::<PasswordValidationError>() {
        return tonic::Status::invalid_argument(pwd_err.to_string());
    }

    // Typed user-service preconditions. The error's `to_string()` is the
    // stable wire code (e.g. "last_auth_method") that callers — including
    // Forage's account-page error banner — branch on.
    if let Some(svc_err) = err.downcast_ref::<UserServiceError>() {
        return match svc_err {
            UserServiceError::LastAuthMethod => {
                tonic::Status::failed_precondition(svc_err.to_string())
            }
        };
    }

    // Log the full error chain for debugging, return a safe message.
    tracing::warn!("service error: {err:#}");
    tonic::Status::internal("internal error")
}
