use little_clip_driver::TrofeoDriver;
use little_clip_render::ScreenRenderer;
use little_clip_core::SlotManager;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    println!("Testing Trofeo Vision USB Driver...");
    let start_open = Instant::now();
    let mut driver = TrofeoDriver::open()?;
    println!("Device opened and handshaked in {:?}", start_open.elapsed());
    println!("Device info: PM={}, SUB={}, cmd={}", driver.info.pm, driver.info.sub, driver.info.chunk_cmd);

    let renderer = ScreenRenderer::default_renderer()?;
    let mut mgr = SlotManager::new();
    mgr.push_left(little_clip_core::ClipItem::from_text("HELLO TROFEO VISION!\nLittle Screen Clip\nRust Driver Active"));

    let start_render = Instant::now();
    let frame = renderer.render_frame(&mgr)?;
    println!("Rendered frame ({} bytes) in {:?}", frame.len(), start_render.elapsed());

    let start_send = Instant::now();
    driver.send_frame(&frame)?;
    println!("Sent frame over USB in {:?}", start_send.elapsed());

    println!("SUCCESS! Check the screen to verify image display!");
    Ok(())
}
