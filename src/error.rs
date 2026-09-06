use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// RFC 7807 Problem Details representation.
#[derive(Debug, Clone, Serialize, serde::Deserialize, ToSchema, PartialEq, Eq)]
pub struct ProblemDetails {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

/// Portal API error variants conforming to RFC 7807.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, slug, title, detail) = match self {
            Self::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                "resource-not-found",
                "Resource Not Found",
                Some(msg),
            ),
            Self::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(msg),
            ),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Unauthorized",
                None,
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "Forbidden", None),
            Self::Conflict(msg) => (StatusCode::CONFLICT, "conflict", "Conflict", Some(msg)),
            Self::Unavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service-unavailable",
                "Service Unavailable",
                Some(msg),
            ),
            Self::Internal(err) => {
                tracing::error!(error = %err, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal-error",
                    "Internal Server Error",
                    Some("Unexpected internal error".to_string()),
                )
            }
        };

        let problem = ProblemDetails {
            r#type: format!("https://joinedcontext.com/errors/{slug}"),
            title: title.to_string(),
            status: status.as_u16(),
            detail,
            instance: None,
        };

        let body = serde_json::to_string(&problem).unwrap_or_else(|_| {
            r#"{"type":"https://joinedcontext.com/errors/internal-error","title":"Internal Server Error","status":500}"#.to_string()
        });

        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/problem+json")
            .body(Body::from(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn status_codes_and_content_type() {
        let cases = [
            (ApiError::NotFound("test".into()), StatusCode::NOT_FOUND),
            (ApiError::BadRequest("test".into()), StatusCode::BAD_REQUEST),
            (ApiError::Unauthorized, StatusCode::UNAUTHORIZED),
            (ApiError::Forbidden, StatusCode::FORBIDDEN),
            (ApiError::Conflict("test".into()), StatusCode::CONFLICT),
            (
                ApiError::Unavailable("test".into()),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ApiError::Internal("test".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (err, expected_status) in cases {
            let res = err.into_response();
            assert_eq!(res.status(), expected_status);
            assert_eq!(
                res.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/problem+json"
            );
        }
    }

    #[tokio::test]
    async fn internal_error_hides_message() {
        let secret_msg = "database password leaked";
        let res = ApiError::Internal(secret_msg.to_string()).into_response();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
        let problem: ProblemDetails = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            problem.detail,
            Some("Unexpected internal error".to_string())
        );
        assert!(!String::from_utf8_lossy(&body_bytes).contains(secret_msg));
        assert_eq!(
            problem.r#type,
            "https://joinedcontext.com/errors/internal-error"
        );
        assert_eq!(problem.title, "Internal Server Error");
    }
}
