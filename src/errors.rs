use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub enum AppError {
    Db(sqlx::Error),
    Template(minijinja::Error),
    NotFound(String),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Db(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Database error: {e}")),
            AppError::Template(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Template error: {e}")),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Internal error: {e}")),
        };
        (status, message).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Db(e)
    }
}

impl From<minijinja::Error> for AppError {
    fn from(e: minijinja::Error) -> Self {
        AppError::Template(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Internal(anyhow::anyhow!(e))
    }
}
