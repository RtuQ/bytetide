use std::io::Read;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// Drain both pipes while waiting, and bound child lifetime so a stalled scenario fails visibly.
pub fn output(command: &mut Command) -> Output {
    let mut child = command.spawn().expect("spawn bytetide binary");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stdout = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let stderr = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().expect("poll bytetide") {
            break (status, false);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break (child.wait().expect("reap stalled bytetide"), true);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    };
    assert!(
        !timed_out,
        "bytetide exceeded 30 seconds; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
