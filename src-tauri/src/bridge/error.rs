//! 桥错误类型：REST 错误响应（`ApiError`）与服务层错误（`ServiceError`）。
//!
//! - `ApiError`：稳定错误码 + JSON 信封 `{"error":{"code","message"}}`，供 skill/AI 侧
//!   程序化判别。/exchange 的非法 matcher 一律 400，绝不静默降级为 match-all。
//! - `ServiceError`：`BridgeService` 各方法的统一错误面（缺会话/非法输入/后端故障），
//!   路由侧按端点语义显式转换为响应（404 文本 / 400 文本 / ApiError 信封）。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// 服务层错误：`BridgeService` 实现统一的错误面（缺会话语义一律 `NotFound`）。
#[derive(Debug)]
pub enum ServiceError {
    /// 会话不存在（或已关闭）。
    NotFound,
    /// 调用方输入非法（携带人话描述）。
    Invalid(String),
    /// 后端故障（底层 manager/IO 错误文本原样透出，与抽取前行为一致）。
    Backend(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "session not found"),
            Self::Invalid(m) => write!(f, "{m}"),
            Self::Backend(m) => write!(f, "{m}"),
        }
    }
}

/// REST 错误：稳定错误码 + JSON 体 `{"error":{"code","message"}}`。
/// `status`/`code`/`message` 对同 crate 的路由测试可见（断言稳定错误码用）。
#[derive(Debug)]
pub struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl ApiError {
    pub(crate) fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    /// message 缺省取 code（404 场景错误码本身即人话）。
    pub(crate) fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            message: code.to_string(),
        }
    }
}

impl From<ServiceError> for ApiError {
    fn from(e: ServiceError) -> Self {
        match e {
            ServiceError::NotFound => ApiError::not_found("session_not_found"),
            ServiceError::Invalid(m) => ApiError::bad_request("invalid_request", m),
            ServiceError::Backend(m) => ApiError::bad_request("backend_error", m),
        }
    }
}

#[derive(Serialize)]
struct ApiErrorEnvelope<'a> {
    error: ApiErrorDetail<'a>,
}

#[derive(Serialize)]
struct ApiErrorDetail<'a> {
    code: &'a str,
    message: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorEnvelope {
                error: ApiErrorDetail {
                    code: self.code,
                    message: &self.message,
                },
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ServiceError → ApiError 的缺省映射：NotFound → 404 信封 session_not_found。
    #[test]
    fn service_error_converts_to_api_error_envelope() {
        let err: ApiError = ServiceError::NotFound.into();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.code, "session_not_found");

        let err: ApiError = ServiceError::Backend("port gone".into()).into();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, "backend_error");
        assert_eq!(err.message, "port gone");
    }

    /// Display 面向 HTTP 文本响应（/send 400 文本体即 e.to_string()）。
    #[test]
    fn service_error_display_matches_variant_text() {
        assert_eq!(ServiceError::NotFound.to_string(), "session not found");
        assert_eq!(ServiceError::Invalid("bad".into()).to_string(), "bad");
        assert_eq!(ServiceError::Backend("boom".into()).to_string(), "boom");
    }
}
