use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use reshot_capture::ScreenCaptureService;

fn main() -> Result<()> {
    let service = ScreenCaptureService::new();
    let stream = service.start_monitor_stream(0, 0)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(frame) = stream.try_next_frame() {
            println!(
                "{}x{} monitor frame @ ({},{})",
                frame.width, frame.height, stream.monitor.left, stream.monitor.top
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    bail!("no WGC frame within three seconds")
}
