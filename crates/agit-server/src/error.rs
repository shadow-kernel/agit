use agit_core::Error as CoreError;
use agit_protocol::types::{ApiError, ApiErrorBody};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug)]
pub enum ApiErr {
    Core(CoreError),
    NotFound(String),
    Forbidden(String),
    BadRequest(String),
}

pub type ApiResult<T> = Result<T, ApiErr>;

impl From<CoreError> for ApiErr {
    fn from(e: CoreError) -> Self {
        ApiErr::Core(e)
    }
}

impl From<serde_json::Error> for ApiErr {
    fn from(e: serde_json::Error) -> Self {
        ApiErr::BadRequest(e.to_string())
    }
}

impl From<std::io::Error> for ApiErr {
    fn from(e: std::io::Error) -> Self {
        ApiErr::Core(CoreError::Io(e))
    }
}

impl ApiErr {
    fn parts(&self) -> (StatusCode, String, String, bool, Option<serde_json::Value>) {
        match self {
            ApiErr::NotFound(m) => (StatusCode::NOT_FOUND, "not_found".into(), m.clone(), false, None),
            ApiErr::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden".into(), m.clone(), false, None),
            ApiErr::BadRequest(m) => (StatusCode::BAD_REQUEST, "invalid".into(), m.clone(), false, None),
            ApiErr::Core(e) => {
                let code = e.code().to_string();
                let details = match e {
                    CoreError::RefConflict { name, expected, actual } => Some(serde_json::json!({
                        "name": name, "expected": expected, "actual": actual
                    })),
                    CoreError::LeaseConflict { path, holder, lease_id } => Some(serde_json::json!({
                        "path": path, "holder": holder, "lease_id": lease_id
                    })),
                    CoreError::MergeConflicts(n) => Some(serde_json::json!({ "count": n })),
                    _ => None,
                };
                let status = match e {
                    CoreError::NotARepository(_) | CoreError::RefNotFound(_) | CoreError::UnknownRevision(_)
                    | CoreError::LeaseNotFound(_) | CoreError::WorkspaceNotFound(_) | CoreError::PathNotTracked(_) => StatusCode::NOT_FOUND,
                    CoreError::ObjectNotFound(_) => StatusCode::PRECONDITION_FAILED,
                    CoreError::AlreadyExists(_) | CoreError::RefConflict { .. } | CoreError::LeaseConflict { .. }
                    | CoreError::MergeConflicts(_) | CoreError::WorkspaceExists(_) => StatusCode::CONFLICT,
                    CoreError::Invalid(_) | CoreError::InvalidRefName(_) | CoreError::InvalidObjectId(_)
                    | CoreError::AmbiguousRevision(_) | CoreError::NothingToCommit => StatusCode::BAD_REQUEST,
                    CoreError::LockTimeout(_) => StatusCode::SERVICE_UNAVAILABLE,
                    CoreError::PolicyViolation(_) => StatusCode::FORBIDDEN,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                (status, code, e.to_string(), e.is_retryable(), details)
            }
        }
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        let (status, code, message, retryable, details) = self.parts();
        if status.is_server_error() {
            tracing::error!(%code, %message, "request failed");
        }
        let body = ApiErrorBody { ok: false, error: ApiError { code, message, retryable, details } };
        (status, Json(body)).into_response()
    }
}
