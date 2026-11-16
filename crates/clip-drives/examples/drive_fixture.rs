//! Explicit test-support IPC host; never inspects or mutates real block devices.
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    time::Duration,
};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("Usage: drive_fixture STATE_DIRECTORY SOURCE_DIRECTORY".into());
    }
    let state = PathBuf::from(&args[0]);
    let source = PathBuf::from(&args[1]);
    let service = clip_drives::Service::start_fixture(
        clip_drives::DriveConfig {
            sync_interval_seconds: 1,
            ram_min_available: 0,
            ..Default::default()
        },
        state,
        source,
    )?;
    let socket = little_clip_core::ipc::socket_path();
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let jobs = little_clip_core::BackgroundJobs::new(4, 16);
    loop {
        let stream = tokio::select! {
            result = listener.accept() => Some(result?),
            _ = terminate.recv() => None,
            _ = interrupt.recv() => None,
        };
        let Some((stream, _)) = stream else {
            break;
        };
        let stream = stream.into_std()?;
        stream.set_nonblocking(false)?;
        let service = service.clone();
        jobs.submit(Duration::MAX, move || {
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                stream.set_read_timeout(Some(Duration::from_secs(3)))?;
                stream.set_write_timeout(Some(Duration::from_secs(3)))?;
                let mut writer = stream.try_clone()?;
                let mut reader = BufReader::new(stream).take(1024 * 1024 + 1);
                let mut bytes = Vec::new();
                reader.read_until(b'\n', &mut bytes)?;
                if bytes.len() > 1024 * 1024 {
                    return Err("Request too large".into());
                }
                let reply = match serde_json::from_slice::<IpcRequest>(&bytes) {
                    Ok(IpcRequest::Drives(request)) => match service.request(request) {
                        Ok(reply) => IpcResponse::Drives(reply),
                        Err(error) => IpcResponse::Error {
                            error: error.to_string(),
                        },
                    },
                    _ => IpcResponse::Error {
                        error: "Owned drive fixture serves only drive requests".into(),
                    },
                };
                serde_json::to_writer(&mut writer, &reply)?;
                writer.write_all(b"\n")?;
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("Fixture IPC: {error}");
            }
        });
    }
    drop(listener);
    tokio::task::spawn_blocking(move || service.shutdown()).await??;
    Ok(())
}
