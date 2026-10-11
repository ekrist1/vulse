use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use vulse_model::ApiErrorBody;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized,
    Forbidden(String),
    NotFound,
    PayloadTooLarge,
    TooManyRequests,
    Conflict {
        message: String,
        document_ids: Vec<String>,
    },
    Internal(anyhow::Error),
}

impl AppError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
            document_ids: Vec::new(),
        }
    }

    pub fn conflict_docs(message: impl Into<String>, document_ids: Vec<String>) -> Self {
        Self::Conflict {
            message: message.into(),
            document_ids,
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::BadRequest(message) => write!(f, "{message}"),
            AppError::Unauthorized => write!(f, "unauthorized"),
            AppError::Forbidden(message) => write!(f, "{message}"),
            AppError::NotFound => write!(f, "not found"),
            AppError::TooManyRequests => write!(f, "Authentication is busy. Try again shortly."),
            AppError::PayloadTooLarge => write!(f, "payload too large"),
            AppError::Conflict { message, .. } => write!(f, "{message}"),
            AppError::Internal(err) => write!(f, "{err}"),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            AppError::BadRequest(error) => (
                StatusCode::BAD_REQUEST,
                ApiErrorBody {
                    error,
                    document_ids: None,
                },
            ),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                ApiErrorBody {
                    error: "unauthorized".into(),
                    document_ids: None,
                },
            ),
            AppError::Forbidden(error) => (
                StatusCode::FORBIDDEN,
                ApiErrorBody {
                    error,
                    document_ids: None,
                },
            ),
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                ApiErrorBody {
                    error: "not found".into(),
                    document_ids: None,
                },
            ),
            AppError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                ApiErrorBody {
                    error: "Authentication is busy. Try again shortly.".into(),
                    document_ids: None,
                },
            ),
            AppError::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                ApiErrorBody {
                    error: "payload too large".into(),
                    document_ids: None,
                },
            ),
            AppError::Conflict {
                message,
                document_ids,
            } => (
                StatusCode::CONFLICT,
                ApiErrorBody {
                    error: message,
                    document_ids: if document_ids.is_empty() {
                        None
                    } else {
                        Some(document_ids)
                    },
                },
            ),
            AppError::Internal(err) => {
                tracing::error!(error = %err, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ApiErrorBody {
                        error: "internal error".into(),
                        document_ids: None,
                    },
                )
            }
        };
        (status, Json(body)).into_response()
    }
}

pub fn unique_message(err: anyhow::Error, fallback: &str) -> AppError {
    let text = format!("{err:#}").to_ascii_lowercase();
    if text.contains("users.email") {
        AppError::conflict("email already registered")
    } else if text.contains("users.handle") {
        AppError::conflict("handle already registered")
    } else if text.contains("collections.owner_id") || text.contains("owner_id, name") {
        AppError::conflict("a collection with that name already exists")
    } else if text.contains("unique") {
        AppError::conflict(fallback)
    } else {
        AppError::Internal(err)
    }
}

impl std::error::Error for AppError {}
