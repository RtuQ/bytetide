//! 元信息路由：`/health` `/ports` `/sessions` `/sessions/:id` `/sessions/:id/stats`。

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use super::{not_found, BridgeCtx};
use bytetide_core::serial::manager::{BridgeStats, RING_CAP};
use bytetide_core::serial::port::list_ports;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    ok: bool,
    version: &'static str,
    ring_cap: usize,
    token_set: bool,
    allow_send: bool,
}

pub(crate) async fn health(State(ctx): State<BridgeCtx>) -> impl IntoResponse {
    let c = ctx.cfg.read();
    Json(Health {
        ok: true,
        version: VERSION,
        ring_cap: RING_CAP,
        token_set: !c.token.is_empty(),
        allow_send: c.allow_send,
    })
}

pub(crate) async fn ports() -> impl IntoResponse {
    Json(list_ports())
}

pub(crate) async fn sessions(State(ctx): State<BridgeCtx>) -> impl IntoResponse {
    Json(ctx.service.list_sessions())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDetail {
    id: String,
    config: serde_json::Value,
    status: String,
    stats: Option<BridgeStats>,
    /// 会话落盘日志完整路径（`/export` 即流式返回该文件；离线会话为来源文件）。
    log_path: Option<String>,
}

pub(crate) async fn session_detail(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
) -> Response {
    match ctx.service.session(&id) {
        Ok(s) => {
            let stats = ctx.service.stats(&id).ok();
            let log_path = ctx
                .service
                .log_path(&id)
                .ok()
                .map(|p| p.to_string_lossy().into_owned());
            Json(SessionDetail {
                id: s.id,
                config: serde_json::to_value(&s.config).unwrap_or_default(),
                status: s.status,
                stats,
                log_path,
            })
            .into_response()
        }
        Err(_) => not_found(),
    }
}

pub(crate) async fn stats(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.service.stats(&id) {
        Ok(s) => Json(s).into_response(),
        Err(_) => not_found(),
    }
}
