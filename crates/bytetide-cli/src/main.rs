//! ByteTide CLI：无 UI 的串口/网络日志监控。
//! 输出分流：数据行走 stdout（便于重定向/管道），连接提示/错误/汇总走 stderr。
//! 行消费与桌面端同一拉模型：按 ring 游标每 50ms 拉取，无高频事件推送。

mod args;
mod clisink;
mod input;
mod ports;
mod render;

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use bytetide_core::serial::manager::{PortManager, SendMode, SendRequest};
use bytetide_core::serial::port::{list_ports, PortConfig};
use bytetide_core::{logfmt, sink::EventSink};
use clap::Parser;

use args::{Cli, Command, MonitorArgs};
use clisink::{CliSink, SinkEvent};
use input::{parse_hex_pairs, parse_input, InputCmd, Mode};

/// 主循环节奏：拉取周期 / 单批行数上限
const POLL_MS: u64 = 50;
const PULL_MAX: usize = 5000;

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::List => cmd_list(),
        Command::Monitor(a) => cmd_monitor(&a),
    };
    // 显式 exit：stdin 线程可能阻塞在读上，不等它自然退出
    std::process::exit(code);
}

/// `list`：枚举串口并对齐输出（Linux 无 udev 元数据时用 by-id 文件名补描述）
fn cmd_list() -> i32 {
    let all = list_ports();
    if all.is_empty() {
        eprintln!("未发现串口");
        return 0;
    }
    for row in ports::format_port_rows(&all, &ports::scan_by_id()) {
        println!("{row}");
    }
    0
}

fn cmd_monitor(args: &MonitorArgs) -> i32 {
    let color = color_enabled(args);
    let config = match args::to_port_config(args) {
        Ok(Some(c)) => c,
        Ok(None) => match pick_port(args, color) {
            Ok(c) => c,
            Err(code) => return code,
        },
        Err(msg) => {
            eerr(color, &format!("参数错误: {msg}"));
            return 2;
        }
    };
    let show_ts = args.ts || args.ts_format.is_some();
    run_session(args, &config, color, args.json, show_ts)
}

/// 颜色开关：--no-color / NO_COLOR / stdout 非终端时关闭（数据行走 stdout，按它判定）
fn color_enabled(args: &MonitorArgs) -> bool {
    if args.no_color {
        return false;
    }
    if std::env::var_os("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// 无数据源参数：终端里弹选择器；非终端直接报错（绝不等 tty）
fn pick_port(args: &MonitorArgs, color: bool) -> Result<PortConfig, i32> {
    if !std::io::stdin().is_terminal() {
        eerr(
            color,
            "未指定数据源：请传 -p <串口> / --tcp <HOST:PORT> / --tcp-listen <PORT> / --udp <PORT>",
        );
        return Err(2);
    }
    let all = list_ports();
    if all.is_empty() {
        eerr(color, "未发现可用串口");
        return Err(1);
    }
    let items = ports::format_port_rows(&all, &ports::scan_by_id());
    let theme = dialoguer::theme::ColorfulTheme::default();
    let sel = dialoguer::Select::with_theme(&theme)
        .with_prompt("选择串口")
        .items(&items)
        .default(0)
        .interact_opt();
    match sel {
        Ok(Some(i)) => Ok(args::serial_config(args, &all[i].name)),
        _ => {
            eprintln!("已取消");
            Err(1)
        }
    }
}

/// 人读连接描述（提示/汇总用）
fn describe_config(c: &PortConfig) -> String {
    match c.transport.as_deref() {
        Some("tcp-client") => format!(
            "TCP {}:{}",
            c.tcp_host.clone().unwrap_or_default(),
            c.tcp_port.unwrap_or_default()
        ),
        Some("tcp-server") => format!("TCP 服务 0.0.0.0:{}", c.tcp_port.unwrap_or_default()),
        Some("udp") => format!("UDP 监听 0.0.0.0:{}", c.udp_local_port.unwrap_or_default()),
        _ => {
            let parity = match c.parity.as_str() {
                "odd" => "O",
                "even" => "E",
                _ => "N",
            };
            format!(
                "{} @ {} {}{}{}",
                c.name, c.baud_rate, c.data_bits, parity, c.stop_bits
            )
        }
    }
}

/// 监控主流程：连接 -> 50ms 轮询（事件/输入/游标拉取）-> 收尾汇总。
/// 退出码：0=用户退出/干净 EOF/BrokenPipe；1=从未连上（连接失败）；2=连上后出错或参数非法。
fn run_session(
    args: &MonitorArgs,
    config: &PortConfig,
    color: bool,
    json: bool,
    show_ts: bool,
) -> i32 {
    let log_config = logfmt::LogConfig {
        log_path_template: args.record.clone().filter(|s| !s.is_empty()),
        line_ts_format: args.ts_format.clone().filter(|s| !s.is_empty()),
    };
    // 空 sessions_dir = 缺省不落盘（core 语义：仅 --record 模板给出时录制）
    let sessions_dir = PathBuf::new();

    // Ctrl-C：只置标志，主循环自然收敛；读线程由 disconnect 收尾
    let quit = Arc::new(AtomicBool::new(false));
    if ctrlc::set_handler({
        let q = quit.clone();
        move || q.store(true, Ordering::Relaxed)
    })
    .is_err()
    {
        eerr(color, "无法注册 Ctrl-C 处理器");
    }

    let (etx, erx) = mpsc::channel::<SinkEvent>();
    let manager = PortManager::new();
    let sink: Arc<dyn EventSink> = Arc::new(CliSink::new(etx));
    let desc = describe_config(config);
    let start = Instant::now();

    let mut id = match manager.connect(
        config.clone(),
        log_config.clone(),
        sink.clone(),
        sessions_dir.clone(),
    ) {
        Ok(id) => id,
        Err(e) => {
            eerr(color, &format!("启动会话失败: {e}"));
            return 2;
        }
    };

    // stdin 交互线程（仅终端）：只做解析，发送集中在主循环（manager 单线程访问）
    let interactive = std::io::stdin().is_terminal();
    let (itx, irx) = mpsc::channel::<InputCmd>();
    if interactive {
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines().map_while(Result::ok) {
                if itx.send(parse_input(&line)).is_err() {
                    break;
                }
            }
        });
    }

    let mut out = std::io::BufWriter::new(std::io::stdout());
    let mut cursor: u64 = 0; // ring 游标（重连归零：新会话 no 从 1 起，历史重新输出）
    let mut mode = Mode::Ascii;
    let append_nl = args::append_newline(args);
    let mut session_connected = false; // 当前会话是否连上（重连后重置）
    let mut run_connected = false; // 整个运行期是否连上过（决定退出码 1/2）
    let mut retries_used: u32 = 0;
    let mut session_ended = false;
    let mut last_error: Option<String> = None;
    let mut user_quit = false;
    let mut pipe_closed = false; // 如 `| head`：按干净退出处理
    let mut write_err: Option<String> = None;

    loop {
        // 1) 读线程事件（状态/错误/告警）
        while let Ok(ev) = erx.try_recv() {
            match ev {
                SinkEvent::Status(s) => match s.as_str() {
                    "connecting" => {
                        if config.transport.as_deref() == Some("tcp-server") {
                            einfo(color, &format!("等待接入 {desc}…"));
                        } else {
                            einfo(color, &format!("正在连接 {desc}…"));
                        }
                    }
                    "connected" => {
                        session_connected = true;
                        run_connected = true;
                        eok(color, &format!("已连接 {desc}"));
                        if interactive {
                            einfo(
                                color,
                                "输入回车发送；/hex AA 01 5a、/mode ascii|hex、/quit 退出",
                            );
                        }
                    }
                    "error" | "disconnected" => session_ended = true,
                    _ => {}
                },
                SinkEvent::Error(m) => {
                    eerr(color, &m);
                    last_error = Some(m);
                }
                SinkEvent::Alerts(hits) => {
                    for h in hits {
                        eerr(
                            color,
                            &format!("[alert] {} {}: {}", h.pattern, h.level, h.text),
                        );
                    }
                }
            }
        }

        // 2) stdin 命令
        while let Ok(cmd) = irx.try_recv() {
            match cmd {
                InputCmd::Send { text } => {
                    do_send(&manager, &id, mode, text, append_nl, color);
                }
                InputCmd::SendHex { text } => {
                    do_send(&manager, &id, Mode::Hex, text, append_nl, color);
                }
                InputCmd::SetMode(m) => {
                    mode = m;
                    einfo(
                        color,
                        if m == Mode::Ascii {
                            "发送模式: ASCII"
                        } else {
                            "发送模式: HEX"
                        },
                    );
                }
                InputCmd::Quit => user_quit = true,
                InputCmd::Noop => {}
            }
        }

        // 3) 游标拉取 + 渲染：整批拼一个 String 单次写出，避免行撕裂
        match manager.ring_lines_after_no(&id, cursor, PULL_MAX) {
            Ok(lines) if !lines.is_empty() => {
                let mut buf = String::with_capacity(lines.len() * 64);
                for l in &lines {
                    if json {
                        buf.push_str(&render::render_json(l));
                    } else {
                        buf.push_str(&render::render_line(l, show_ts, color));
                    }
                    buf.push('\n');
                }
                cursor = lines.last().map(|l| l.no).unwrap_or(cursor);
                if let Err(e) = out.write_all(buf.as_bytes()).and_then(|()| out.flush()) {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        pipe_closed = true;
                    } else {
                        write_err = Some(format!("写出数据失败: {e}"));
                    }
                }
            }
            Ok(_) => {}
            Err(_) => session_ended = true, // 会话已被移除（防御，正常流程不触发）
        }

        if user_quit || pipe_closed || write_err.is_some() || quit.load(Ordering::Relaxed) {
            break;
        }

        // 4) 会话结束：重试或退出（上面第 3 步已把 ring 尾巴拉干净）
        if session_ended {
            if retries_used < args.retry {
                retries_used += 1;
                einfo(
                    color,
                    &format!("会话结束，1 秒后重连（{}/{}）…", retries_used, args.retry),
                );
                if sleep_unless_quit(&quit, Duration::from_secs(1)) {
                    break;
                }
                // 旧会话已结束：join 立即返回，并冲刷其录制文件
                let _ = manager.disconnect(&id);
                match manager.connect(
                    config.clone(),
                    log_config.clone(),
                    sink.clone(),
                    sessions_dir.clone(),
                ) {
                    Ok(new_id) => {
                        id = new_id;
                        cursor = 0;
                        session_connected = false;
                        session_ended = false;
                        last_error = None;
                        continue;
                    }
                    Err(e) => {
                        last_error = Some(format!("重连失败: {e}"));
                        break;
                    }
                }
            }
            break;
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }

    // 收尾：统计/录制路径必须在 disconnect 之前取（disconnect 会移除会话）。
    // 未连上的会话直接跳过 disconnect：读线程可能卡在 accept/connect 上，join 会挂住；
    // 此刻无数据无录制内容，进程退出由系统回收。
    let stats = if session_connected {
        manager.bridge_stats(&id)
    } else {
        None
    };
    let record_path = if session_connected {
        manager
            .session_log_path(&id)
            .ok()
            .filter(|p| !p.is_empty())
    } else {
        None
    };
    if session_connected {
        let _ = manager.disconnect(&id); // 停读线程并冲刷录制
    }
    let _ = out.flush();

    // 汇总（stderr 青色）：时长取整个运行期；行/字节统计取最后一个会话
    let secs = start.elapsed().as_secs();
    let dur = if secs >= 60 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else {
        format!("{secs} 秒")
    };
    einfo(color, &format!("会话结束：时长 {dur}"));
    if let Some(st) = stats {
        einfo(color, &format!("接收 {} 行 / {} 字节", st.rx_lines, st.rx_bytes));
        einfo(color, &format!("发送 {} 行 / {} 字节", st.tx_lines, st.tx_bytes));
    }
    if let Some(p) = record_path {
        einfo(color, &format!("录制文件 {p}"));
    }

    if let Some(m) = write_err.as_deref() {
        eerr(color, m);
    }
    if user_quit || pipe_closed {
        return 0;
    }
    if write_err.is_some() {
        return 2;
    }
    if !run_connected {
        return 1; // 从未连上：连接失败
    }
    if last_error.is_some() {
        return 2; // 连上后出错（重试耗尽）
    }
    0
}

/// 发送辅助：ASCII 按需补换行；hex 模式先校验合法性，非法不打到设备
fn do_send(
    manager: &PortManager,
    id: &str,
    mode: Mode,
    text: String,
    append_nl: bool,
    color: bool,
) {
    match mode {
        Mode::Ascii => {
            let mut t = text;
            if append_nl {
                t.push('\n');
            }
            if let Err(e) = manager.send(id, SendRequest { mode: SendMode::Ascii, text: t }) {
                eerr(color, &format!("发送失败: {e}"));
            }
        }
        Mode::Hex => match parse_hex_pairs(&text) {
            Ok(b) if b.is_empty() => {}
            Ok(_) => {
                if let Err(e) = manager.send(id, SendRequest { mode: SendMode::Hex, text }) {
                    eerr(color, &format!("发送失败: {e}"));
                }
            }
            Err(m) => eerr(color, &m),
        },
    }
}

/// stderr 带色输出（着色与否另按 stderr 自身 tty 状态由 console 判定）
fn epaint(color: bool, style: console::Style, msg: &str) {
    if color {
        eprintln!("{}", style.for_stderr().apply_to(msg));
    } else {
        eprintln!("{msg}");
    }
}

fn einfo(color: bool, msg: &str) {
    epaint(color, console::Style::new().cyan(), msg);
}
fn eok(color: bool, msg: &str) {
    epaint(color, console::Style::new().green(), msg);
}
fn eerr(color: bool, msg: &str) {
    epaint(color, console::Style::new().red(), msg);
}

/// 分片睡眠，Ctrl-C 期间可打断；返回 true = 已请求退出
fn sleep_unless_quit(quit: &AtomicBool, total: Duration) -> bool {
    let step = Duration::from_millis(100);
    let mut left = total;
    while !left.is_zero() {
        if quit.load(Ordering::Relaxed) {
            return true;
        }
        let d = left.min(step);
        std::thread::sleep(d);
        left -= d;
    }
    quit.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_serial() {
        let c = PortConfig {
            name: "COM3".into(),
            ..PortConfig::default()
        };
        assert_eq!(describe_config(&c), "COM3 @ 115200 8N1");
        let c = PortConfig {
            name: "ttyUSB0".into(),
            baud_rate: 9600,
            data_bits: 7,
            parity: "odd".into(),
            stop_bits: "2".into(),
            ..PortConfig::default()
        };
        assert_eq!(describe_config(&c), "ttyUSB0 @ 9600 7O2");
    }

    #[test]
    fn describe_net() {
        let c = PortConfig {
            transport: Some("tcp-client".into()),
            tcp_host: Some("192.168.1.9".into()),
            tcp_port: Some(9000),
            ..PortConfig::default()
        };
        assert_eq!(describe_config(&c), "TCP 192.168.1.9:9000");
        let c = PortConfig {
            transport: Some("tcp-server".into()),
            tcp_port: Some(9000),
            ..PortConfig::default()
        };
        assert_eq!(describe_config(&c), "TCP 服务 0.0.0.0:9000");
        let c = PortConfig {
            transport: Some("udp".into()),
            udp_local_port: Some(5000),
            ..PortConfig::default()
        };
        assert_eq!(describe_config(&c), "UDP 监听 0.0.0.0:5000");
    }

    #[test]
    fn sleep_unless_quit_returns_immediately_when_set() {
        let quit = AtomicBool::new(true);
        assert!(sleep_unless_quit(&quit, Duration::from_secs(10)));
        let quit = AtomicBool::new(false);
        assert!(!sleep_unless_quit(&quit, Duration::from_millis(10)));
    }
}
