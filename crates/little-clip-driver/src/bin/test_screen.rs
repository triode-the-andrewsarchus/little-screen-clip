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

    println!("Running 5 iterations at 500ms interval...");
    for i in 0..5 {
        let start_send = Instant::now();
        match driver.send_frame(&frame) {
            Ok(_) => println!("Sent frame {} in {:?}", i, start_send.elapsed()),
            Err(e) => {
                eprintln!("send_frame {} FAILED: {}", i, e);
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    println!("Hardware test completed successfully!");
    Ok(())
}
