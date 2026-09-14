//! 回放 soak（Stage 3 Task 8 Step 4）——`#[ignore]` 长跑测试，显式触发：
//!
//! ```text
//! cargo test -p bytetide-core --test replay_soak -- --ignored
//! ```
//!
//! 或一键：`node scripts/soak-replay.mjs`（默认 quick 即本测试；`--real` 切真实
//! 墙钟 30 分钟）。参数经环境变量透传：`SOAK_REAL` / `SOAK_DURATION_MIN` /
//! `SOAK_SPEED` / `SOAK_TARGET_LINES`。
//!
//! # 两种模式
//! - **quick（默认 / CI）**：虚拟时钟等效——原始间隔 1000ms 的合成流 +
//!   `max_gap_ms=1` 钳制 + `speed=100` → 每行实际睡眠 = round(min(1000,1)/100)
//!   = 0ms，整流瞬放（回放调度只剩 ingest 成本）。两阶段：
//!   ①**bulk 相**（无规则，~1-3s）：每圈 `SOAK_LINES_PER_LOOP`（默认 110_000，
//!   大于 RING_CAP）→ ring 淘汰真实触顶（峰值=上限）+ loop 回卷清屏，产线
//!   `SOAK_TARGET_LINES`（默认 220_000 = 2 圈）；②**告警相**（Pause → 灌规则
//!   → Resume，1 万行）：告警规则评估含逐行正则编译（core 现状成本），全量
//!   挂规则会把 soak 拖慢一个量级，故只在有界尾相验证「事件计数与产线线性、
//!   无无界增长」。秒级跑完 30 分钟级虚拟时长——这就是 CI 可跑的 soak。
//! - **real（SOAK_REAL=1）**：真实墙钟 `SOAK_DURATION_MIN`（默认 30）分钟、
//!   `speed=100` + 默认 10s gap 钳制（≈18 万行），规则全程挂、采样 RSS 趋势
//!   ——只在本地显式跑，不进 CI。
//!
//! # 断言（两模式一致，对应 plan Step 4 验收口径）
//! - ring size 恒 ≤ `RING_CAP`（100_000），bulk 相真实触顶，记录峰值
//!   `maxRingLines`；
//! - 观测 `no` 全程严格递增、批内连续（无重复/回退）；唯一合法的游标缺口 =
//!   loop 回卷清屏（以 `ring_bounds.first_no == 新批首 no` 验证）——
//!   「no 单调无重复」；
//! - 告警事件总量有界且 ≈ 告警相产线/标记周期（无每行事件类无界增长）；
//! - 回放会话禁用操作（send/信号线/录制/分段）稳定报错；
//! - 结束时会话干净移除；摘要以 `SOAK_SUMMARY {json}` 打印供脚本采集
//!   （durationMs / producedLines / maxRingLines / completedLoops / errors[]）。
//!   前端视图上限（viewBufCap）是渲染进程策略，core soak 覆盖其共同数据源
//!   ring；真实 30 分钟人工复跑时对照 perf-frontend.log / perf-heartbeat.log。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytetide_core::replay::model::DEFAULT_MAX_GAP_MS;
use bytetide_core::replay::{ReplayCmd, ReplayConfig, ReplayState};
use bytetide_core::serial::manager::{Pin, SendMode, SendRequest};
use bytetide_core::serial::ring::RING_CAP;
use bytetide_core::serial::rules::{AlertCfg, AlertRuleCfg, AutoReplyCfg, CaptureCfg};
use bytetide_core::serial::PortManager;
use bytetide_core::sink::VecSink;

// ============ 参数 ============

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct SoakParams {
    real: bool,
    /// 停止预算：quick = 保险丝（正常按产线目标先到）；real = 墙钟时长。
    duration: Duration,
    speed: f64,
    max_gap_ms: u64,
    /// bulk 相产线目标（ingest 的 no 计）；real = u64::MAX（按墙钟停）。
    target_lines: u64,
    /// 告警相行数（quick 在 bulk 后追灌 N 行验证事件线性；real = 0 = 全程挂规则）。
    alert_phase_lines: u64,
    lines_per_loop: u64,
    marker_every: u64,
}

fn params() -> SoakParams {
    let speed = env_u64("SOAK_SPEED", 100) as f64;
    let lines_per_loop = env_u64("SOAK_LINES_PER_LOOP", 110_000);
    let marker_every = env_u64("SOAK_MARKER_EVERY", 100);
    if std::env::var("SOAK_REAL").is_ok_and(|v| v == "1") {
        SoakParams {
            real: true,
            duration: Duration::from_secs(env_u64("SOAK_DURATION_MIN", 30) * 60),
            speed,
            max_gap_ms: DEFAULT_MAX_GAP_MS,
            target_lines: u64::MAX,
            alert_phase_lines: 0,
            lines_per_loop,
            marker_every,
        }
    } else {
        SoakParams {
            real: false,
            // 保险丝：正常 <10s 完成；触发即产线吞吐塌陷（后续断言会红）
            duration: Duration::from_secs(env_u64("SOAK_FUSE_SEC", 180)),
            speed,
            max_gap_ms: 1,
            target_lines: env_u64("SOAK_TARGET_LINES", 220_000),
            alert_phase_lines: env_u64("SOAK_ALERT_PHASE_LINES", 10_000),
            lines_per_loop,
            marker_every,
        }
    }
}

// ============ 合成流 ============

/// epoch 当日毫秒 → 合法 ts（可被 offline 解析回原值）。
fn ts_of_ms(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1_000 % 60,
        ms % 1_000
    )
}

/// 写合成日志：N 行、相邻原始间隔 1000ms；每 marker_every 行一条 OVERHEAT
/// 告警标记（告警事件上界断言的产线）。返回（目录守卫、文件路径）。
fn write_synthetic(tag: &str, lines_per_loop: u64, marker_every: u64) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "bytetide-soak-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("soak.log");
    let mut body = String::with_capacity(lines_per_loop as usize * 24);
    for i in 1..=lines_per_loop {
        let text = if i % marker_every == 0 {
            format!("tick-{i} OVERHEAT")
        } else {
            format!("tick-{i}")
        };
        body.push_str(&format!("{}\tRX\t{}\n", ts_of_ms(i * 1_000), text));
    }
    std::fs::write(&path, body).expect("write synthetic log");
    (dir, path)
}

/// RSS 采样（仅 Linux /proc；其余平台 None——真实模式的人工复跑在桌面端对照
/// perf-*.log，CI 不依赖 RSS）。
#[cfg(target_os = "linux")]
fn rss_kb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages * 4096 / 1024)
}

#[cfg(not(target_os = "linux"))]
fn rss_kb() -> Option<u64> {
    None
}

// ============ soak 本体 ============

/// 排空 ring 到头：断言游标纪律（批内/跨批 no 连续、缺口只允许回卷清屏），
/// 返回推进后的 scan_no；顺带采样 ring 峰值。
fn drain_and_check(
    m: &PortManager,
    id: &str,
    mut scan_no: u64,
    max_ring: &mut usize,
    wrap_clears: &mut u64,
) -> u64 {
    loop {
        let batch = m.ring_lines_after_no(id, scan_no, 8192).expect("pull");
        if batch.is_empty() {
            break;
        }
        let first = batch[0].no;
        if first != scan_no + 1 {
            let b = m.ring_bounds(id).expect("bounds");
            assert_eq!(
                b.first_no, first,
                "非回卷清屏的游标缺口: scan_no={scan_no} first={first}（重复/回退?）"
            );
            *wrap_clears += 1;
        }
        for (i, l) in batch.iter().enumerate() {
            assert_eq!(l.no, first + i as u64, "批内 no 不连续（重复/回退?）");
        }
        scan_no = batch[batch.len() - 1].no;
    }
    let b = m.ring_bounds(id).expect("bounds");
    *max_ring = (*max_ring).max(b.size);
    assert!(
        (b.size as u64) <= RING_CAP as u64,
        "ring 超上限: {} > {RING_CAP}",
        b.size
    );
    scan_no
}

#[test]
#[ignore]
fn replay_soak_rings_and_events_stay_bounded() {
    let p = params();
    let tag = if p.real { "real" } else { "quick" };
    let (_dir, path) = write_synthetic(tag, p.lines_per_loop, p.marker_every);

    let m = PortManager::new();
    let sink = Arc::new(VecSink::default());
    let (id, tx, index) = m
        .start_replay_indexed(
            &path,
            ReplayConfig {
                speed: p.speed,
                looped: true,
                max_gap_ms: p.max_gap_ms,
            },
            sink.clone(),
            PathBuf::new(),
        )
        .expect("start replay");
    assert_eq!(index.line_count, p.lines_per_loop);

    let t0 = Instant::now();
    let mut scan_no: u64 = 0;
    let mut max_ring: usize = 0;
    let mut wrap_clears: u64 = 0;
    let mut rss_max: Option<u64> = None;
    let mut rss_last: Option<u64> = None;

    // 规则就位（real=开跑前、quick=bulk 后的告警相前）：统一用 Pause 冻结 ingest
    // 再灌规则——期望命中以**冻结时刻的 ingest 水位**为锚（ingest 恒跑在 scan
    // 前面，scan 游标不能作锚）
    let pause_and_freeze = |tx: &std::sync::mpsc::Sender<ReplayCmd>| {
        tx.send(ReplayCmd::Pause).unwrap();
        while m
            .replay_view(&id)
            .is_none_or(|(s, _)| s != ReplayState::Paused)
        {
            assert!(t0.elapsed() < Duration::from_secs(5), "pause 未生效");
            std::thread::sleep(Duration::from_millis(1));
        }
        m.bridge_last_no(&id).unwrap_or(0)
    };
    let ingest_at_rules = if p.real {
        let frozen = pause_and_freeze(&tx);
        m.set_live_rules(
            &id,
            AutoReplyCfg::default(),
            overheat_rule(),
            CaptureCfg::default(),
        )
        .expect("set rules");
        tx.send(ReplayCmd::Resume).unwrap();
        frozen
    } else {
        0
    };

    // bulk 相：无规则全速瞬放（quick）/ 定速（real），直至产线目标或墙钟预算
    let bulk_target = if p.real { u64::MAX } else { p.target_lines };
    while scan_no < bulk_target && t0.elapsed() < p.duration {
        scan_no = drain_and_check(&m, &id, scan_no, &mut max_ring, &mut wrap_clears);
        if let Some(rss) = rss_kb() {
            rss_max = rss_max.map_or(Some(rss), |prev| Some(prev.max(rss)));
            rss_last = Some(rss);
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // 告警相（quick）：Pause → 灌规则 → Resume 追灌有界行数，验证事件计数与
    // 产线线性（规则评估逐行编译正则，全量挂会拖慢一个量级——见模块头注）
    let ingest_at_rules = if !p.real {
        let frozen = pause_and_freeze(&tx);
        m.set_live_rules(
            &id,
            AutoReplyCfg::default(),
            overheat_rule(),
            CaptureCfg::default(),
        )
        .expect("set rules");
        tx.send(ReplayCmd::Resume).unwrap();
        frozen
    } else {
        ingest_at_rules
    };
    let alert_phase_target = scan_no + p.alert_phase_lines;
    while scan_no < alert_phase_target && t0.elapsed() < p.duration {
        scan_no = drain_and_check(&m, &id, scan_no, &mut max_ring, &mut wrap_clears);
    }
    // 收尾：冻结 ingest 再排空，水位/命中数都不再变动（断言无竞态）
    pause_and_freeze(&tx);
    scan_no = drain_and_check(&m, &id, scan_no, &mut max_ring, &mut wrap_clears);
    let last_no = m.bridge_last_no(&id).unwrap_or(0);
    let wall_ms = t0.elapsed().as_millis() as u64;

    // 产线目标达成（quick 模式墙钟保险丝未先触发）
    if !p.real {
        assert!(
            scan_no >= alert_phase_target,
            "产线未达目标（吞吐塌陷?）: {scan_no} < {alert_phase_target}，wall={wall_ms}ms"
        );
        let loops = scan_no / p.lines_per_loop;
        assert!(loops >= 2, "循环回放未实际发生: loops={loops}");
    }

    // 告警事件有界且与告警相产线线性（每 marker_every 行 1 次；±2 容忍边界）
    let hits: u64 = sink
        .0
        .lock()
        .iter()
        .filter(|e| e.starts_with(&format!("alert-hit {id} ")))
        .filter_map(|e| e.rsplit("n=").next()?.parse::<u64>().ok())
        .sum();
    let event_count = sink
        .0
        .lock()
        .iter()
        .filter(|e| e.starts_with(&format!("alert-hit {id} ")))
        .count() as u64;
    // 期望命中锚在「规则生效时刻的 ingest 水位 → 冻结时刻水位」窗口（每
    // marker_every 行 1 标记；±2 容忍窗口边界）
    let expected = last_no.saturating_sub(ingest_at_rules) / p.marker_every;
    assert!(
        hits.abs_diff(expected) <= 2,
        "告警命中与产线不线性: hits={hits} expected≈{expected} (ingest {ingest_at_rules}→{last_no})"
    );
    assert_eq!(event_count, hits, "每次告警恰一事件（n=1）");

    // 禁用操作稳定报错（长跑全程守卫面不松动）
    let err = |e: anyhow::Error| e.to_string();
    assert_eq!(
        err(m
            .send(
                &id,
                SendRequest {
                    mode: SendMode::Ascii,
                    text: "x".into()
                }
            )
            .unwrap_err()),
        "回放会话不支持发送"
    );
    assert_eq!(
        err(m.set_signal(&id, Pin::Dtr, true).unwrap_err()),
        "回放会话不支持信号线"
    );
    assert_eq!(
        err(m.set_recording(&id, true).unwrap_err()),
        "回放会话不支持落盘"
    );
    assert_eq!(err(m.rotate_log(&id).unwrap_err()), "回放会话不支持落盘");

    // 摘要（脚本采集：SOAK_SUMMARY {json}）
    let summary = serde_json::json!({
        "mode": if p.real { "real" } else { "quick" },
        "durationMs": wall_ms,
        "producedLines": scan_no,
        "maxRingLines": max_ring,
        "ringCap": RING_CAP,
        "completedLoops": scan_no / p.lines_per_loop,
        "observedWrapClears": wrap_clears,
        "alertHitEvents": event_count,
        "alertHits": hits,
        "speed": p.speed,
        "rssKbMax": rss_max,
        "rssKbLast": rss_last,
        "errors": [],
    });
    println!("SOAK_SUMMARY {summary}");

    // 结束清理：断开 = Stop + join + 会话移除
    m.disconnect(&id).expect("disconnect");
    assert!(m.ring_lines_after_no(&id, 0, 1).is_err(), "会话已移除");
    assert_eq!(m.replay_view(&id), None);
}

/// OVERHEAT 告警规则（标记行由 `write_synthetic` 按同一 `marker_every` 生成；
/// min_count=1 无窗口无冷却 = 逐标记命中）。
fn overheat_rule() -> AlertCfg {
    AlertCfg {
        enabled: true,
        rules: vec![AlertRuleCfg {
            id: "soak-overheat".into(),
            pattern: "OVERHEAT".into(),
            use_regex: false,
            case_sensitive: false,
            whole_word: false,
            min_count: 1,
            window_sec: 0,
            cooldown_sec: 0,
            level: "warn".into(),
            enabled: true,
        }],
    }
}
