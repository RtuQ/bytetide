//! 桥认证：Bearer 令牌恒定时间比较（防计时侧信道）+ axum 中间件。
//!
//! `/health` 公开（不回令牌）；令牌未设置时一律 401 并提示先在应用内启用桥。

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::routes::BridgeCtx;

/// 恒定时间比较，避免计时侧信道。
fn ct_eq(a: &str, b: &str) -> bool {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    if ab.len() != bb.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in ab.iter().zip(bb.iter()) {
        d |= x ^ y;
    }
    d == 0
}

pub(crate) async fn auth_mw(
    State(ctx): State<BridgeCtx>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    // /health 公开（不回令牌）
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }
    let token = ctx.cfg.read().token.clone();
    if token.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            "bridge token not set (enable the bridge in the app first)".to_string(),
        )
            .into_response();
    }
    let ok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| ct_eq(t, &token))
        .unwrap_or(false);
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            "invalid or missing bearer token".to_string(),
        )
            .into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_equal_and_differ() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd")); // 前缀同末位异
        assert!(!ct_eq("abc", "ab")); // 长度不等早退
        assert!(!ct_eq("", "a"));
        assert!(ct_eq("", ""));
    }
}
