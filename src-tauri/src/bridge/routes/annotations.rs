//! AI 批注路由（REST 写入 → 界面实时可见，与书签的"人标给 AI"互为对称）：
//! `/sessions/:id/annotations` GET / POST / DELETE。

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use super::{not_found, BridgeCtx};
use bytetide_core::serial::manager::BridgeAnnotation;

/// 每会话批注容量上限（超出丢最旧）。
const ANNOTATION_CAP: usize = 200;
/// 批注携带的行文本摘录长度。
const ANNOTATION_CLIP: usize = 200;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn clip_str(s: &str) -> String {
    s.chars().take(ANNOTATION_CLIP).collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnnotationInput {
    no: u64,
    note: String,
    #[serde(default)]
    ts: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnnotationsBody {
    notes: Vec<AnnotationInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnnotationsDeleteParams {
    /// 缺省时清空全部。
    id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnnotationsPage {
    added: usize,
    annotations: Vec<BridgeAnnotation>,
}

/// 合并 AI 批注：按 (no, note) 去重（重复提交幂等），超出容量丢最旧。
/// 返回 (合并后列表, 实际新增条数)。
fn merge_annotations(
    mut existing: Vec<BridgeAnnotation>,
    incoming: Vec<BridgeAnnotation>,
    cap: usize,
) -> (Vec<BridgeAnnotation>, usize) {
    let mut added = 0usize;
    for c in incoming {
        if existing.iter().any(|e| e.no == c.no && e.note == c.note) {
            continue;
        }
        existing.push(c);
        added += 1;
    }
    if existing.len() > cap {
        let overflow = existing.len() - cap;
        existing.drain(..overflow);
    }
    (existing, added)
}

/// 列出当前批注。
pub(crate) async fn annotations_get(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
) -> Response {
    match ctx.service.annotations(&id) {
        Ok(a) => Json(a).into_response(),
        Err(_) => not_found(),
    }
}

/// 新增批注：`no` 为桥行号；`ts`/`text` 缺省时后端从 ring 中按 `no` 回填。
/// 按 (no, note) 幂等，返回完整列表与新增数；任何变化实时推送到界面。
pub(crate) async fn annotations_post(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Json(body): Json<AnnotationsBody>,
) -> Response {
    if body.notes.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "notes must not be empty",
        )
            .into_response();
    }
    // 行仍在缓冲中时回填 ts/text，AI 只需给 no + note（按行号单行有界读取，
    // 评审 P1-1：不再全量 snapshot；会话缺失 404，单行缺失保留调用方提供的值）
    if ctx.service.line_by_no(&id, 1).is_err() {
        return not_found();
    }
    let now = now_ms();
    let mut candidates: Vec<BridgeAnnotation> = Vec::with_capacity(body.notes.len());
    for (i, n) in body.notes.into_iter().enumerate() {
        let note = n.note.trim().to_string();
        if note.is_empty() {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "note must not be empty",
            )
                .into_response();
        }
        if n.no == 0 {
            return (axum::http::StatusCode::BAD_REQUEST, "no must be >= 1").into_response();
        }
        let (ts, text) = match ctx.service.line_by_no(&id, n.no).ok().flatten() {
            Some(l) => (l.ts.clone(), clip_str(&l.text)),
            None => (n.ts, clip_str(&n.text)),
        };
        candidates.push(BridgeAnnotation {
            id: format!("an{now:x}-{i}"),
            no: n.no,
            ts,
            text,
            note,
            at: now,
        });
    }
    let existing = ctx.service.annotations(&id).unwrap_or_default();
    let (all, added) = merge_annotations(existing, candidates, ANNOTATION_CAP);
    if added > 0 {
        let _ = ctx.service.set_annotations(&id, all.clone());
        ctx.service.notify_annotations_changed(&id, &all);
    }
    Json(AnnotationsPage {
        added,
        annotations: all,
    })
    .into_response()
}

/// 删除批注：带 `?id=` 删单条，不带则清空全部；返回剩余列表。
pub(crate) async fn annotations_delete(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<AnnotationsDeleteParams>,
) -> Response {
    let existing = match ctx.service.annotations(&id) {
        Ok(a) => a,
        Err(_) => return not_found(),
    };
    let remaining: Vec<BridgeAnnotation> = match &p.id {
        Some(rid) => existing.into_iter().filter(|a| &a.id != rid).collect(),
        None => vec![],
    };
    let _ = ctx.service.set_annotations(&id, remaining.clone());
    ctx.service.notify_annotations_changed(&id, &remaining);
    Json(remaining).into_response()
}

#[cfg(test)]
mod tests {
    //! merge_annotations 的去重/容量语义。
    use super::*;

    fn mk_note(id: &str, no: u64, note: &str) -> BridgeAnnotation {
        BridgeAnnotation {
            id: id.into(),
            no,
            ts: String::new(),
            text: String::new(),
            note: note.into(),
            at: 0,
        }
    }

    #[test]
    fn merge_annotations_dedupes_by_no_and_note() {
        let existing = vec![mk_note("a", 5, "first")];
        // 同 (no, note) 重复提交幂等；同 no 不同 note 保留
        let (all, added) = merge_annotations(
            existing,
            vec![mk_note("b", 5, "first"), mk_note("c", 5, "second")],
            200,
        );
        assert_eq!(added, 1);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "a"); // 旧的在前、不被替换
        assert_eq!(all[1].note, "second");
    }

    #[test]
    fn merge_annotations_caps_and_drops_oldest() {
        let existing: Vec<BridgeAnnotation> =
            (0..3).map(|i| mk_note(&format!("o{i}"), i, "n")).collect();
        let incoming: Vec<BridgeAnnotation> = (10..14)
            .map(|i| mk_note(&format!("n{i}"), i, "n"))
            .collect();
        let (all, added) = merge_annotations(existing, incoming, 5);
        assert_eq!(added, 4);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].id, "o2"); // 7 条裁到 5：最旧的 o0、o1 被丢弃
        assert_eq!(all.last().unwrap().id, "n13");
    }
}
