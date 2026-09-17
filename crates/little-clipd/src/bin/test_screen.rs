//! Standalone diagnostic utility for rendering and streaming a display frame.

use little_clip_core::SlotManager;
use little_clip_driver::TrofeoDriver;
use little_clip_render::ScreenRenderer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    println!("Testing Trofeo Vision USB Driver...");
    let mut driver = TrofeoDriver::open()?;
    println!(
        "Device info: PM={}, SUB={}, cmd={}",
        driver.info.pm, driver.info.sub, driver.info.chunk_cmd
    );
    let renderer = ScreenRenderer::default_renderer()?;
    let mut slots = SlotManager::new();
    slots.push_left(little_clip_core::ClipItem::from_text(
        "HELLO TROFEO VISION!",
    ));
    let frame = renderer.render_frame(&slots)?;
    for _ in 0..5 {
        driver.send_frame(&frame)?;
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Ok(())
}
