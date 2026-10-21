use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sivryn_protocol::api::ErrorBody;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("authentication required")]
    Unauthorized,
    #[error("missing permission")]
    Forbidden,
    /// Registration on an invite-only instance without a code.
    #[error("an invite code is needed to register")]
    InviteRequired,
    /// An invite code that doesn't exist, expired or is used up.
    #[error("the invite code is invalid, expired or used up")]
    InvalidInvite,
    #[error("registration is closed on this instance")]
    RegistrationClosed,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    NotImplemented(&'static str),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ApiError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::InviteRequired => (StatusCode::FORBIDDEN, "invite_required"),
            Self::InvalidInvite => (StatusCode::FORBIDDEN, "invalid_invite"),
            Self::RegistrationClosed => (StatusCode::FORBIDDEN, "registration_closed"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            Self::Database(sqlx::Error::RowNotFound) => (StatusCode::NOT_FOUND, "not_found"),
            Self::Database(_) | Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = ?self, "internal error");
            "internal server error".to_owned()
        } else {
            self.to_string()
        };
        (status, Json(ErrorBody { error: code.to_owned(), message })).into_response()
    }
}
