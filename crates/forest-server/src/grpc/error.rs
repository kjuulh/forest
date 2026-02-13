use crate::repositories::error::DbError;

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

    // Log the full error chain for debugging, return a safe message.
    tracing::warn!("service error: {err:#}");
    tonic::Status::internal("internal error")
}
