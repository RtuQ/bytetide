use std::time::Duration;

use tauri::{AppHandle, Emitter};

use super::port::{list_ports, PortInfo};

/// 启动端口热插拔监听：每 1s 轮询 available_ports 做 diff，变化时 emit `port-changed`。
pub fn start_hotplug(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last: Vec<String> = Vec::new();
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            let ports: Vec<PortInfo> = list_ports();
            let names: Vec<String> = ports.iter().map(|p| p.name.clone()).collect();
            if names != last {
                last = names;
                let _ = app.emit("port-changed", ports);
            }
        }
    });
}
