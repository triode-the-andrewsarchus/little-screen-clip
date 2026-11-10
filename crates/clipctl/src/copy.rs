//! Explicit copy routing with identity-scoped cleanup before primary-selection fallback.

use super::send_ipc_req;
use little_clip_core::ipc::{ClipboardRouteStatus, IpcRequest, IpcResponse};
use little_clip_core::{HandSide, process::CommandRequest};
use std::time::Duration;
use tokio::io::{BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use uuid::Uuid;

type Error = Box<dyn std::error::Error>;

fn route_status(response: IpcResponse, id: Uuid) -> Result<ClipboardRouteStatus, Error> {
    match response {
        IpcResponse::ClipboardRoute { request_id, status } if request_id == id => Ok(status),
        IpcResponse::Error { error } => Err(error.into()),
        _ => Err(
            "Daemon did not acknowledge this clipboard route; restart the matching daemon".into(),
        ),
    }
}

async fn request_route(
    writer: &mut OwnedWriteHalf,
    lines: &mut Lines<BufReader<OwnedReadHalf>>,
    request: IpcRequest,
    id: Uuid,
) -> Result<ClipboardRouteStatus, Error> {
    // If this times out, the caller exits and never reuses this connection.
    // The route expires independently; never send another mutation on an uncertain stream.
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        send_ipc_req(writer, lines, &request),
    )
    .await??;
    route_status(response, id)
}

pub(super) async fn copy_to_hand(
    writer: &mut OwnedWriteHalf,
    lines: &mut Lines<BufReader<OwnedReadHalf>>,
    hand: HandSide,
    restore: bool,
) -> Result<(), Error> {
    let id = Uuid::new_v4();
    let status = request_route(
        writer,
        lines,
        IpcRequest::RouteNextClipboard {
            request_id: Some(id),
            hand,
            timeout_ms: 2000,
            restore_os_clipboard: restore,
        },
        id,
    )
    .await?;
    if status != ClipboardRouteStatus::Pending {
        return Err("Clipboard route was not admitted".into());
    }

    let shortcut = tokio::task::spawn_blocking(|| {
        CommandRequest::new("wtype")
            .args(["-m", "alt", "-M", "ctrl", "-k", "c", "-m", "ctrl"])
            .timeout(Duration::from_millis(500))
            .max_output_bytes(16 * 1024)
            .own_process_group()
            .run()
    })
    .await;
    let sent_shortcut = matches!(shortcut, Ok(Ok(ref output)) if output.status.success());
    if finish_route(writer, lines, id, sent_shortcut).await? {
        println!("Copy accepted for routing to {hand:?}");
        return Ok(());
    }

    let primary = tokio::task::spawn_blocking(|| {
        CommandRequest::new("wl-paste")
            .args(["--primary", "-n"])
            .timeout(Duration::from_secs(1))
            .max_output_bytes(1024 * 1024)
            .own_process_group()
            .run()
    })
    .await?;
    let output = primary?;
    if !output.status.success() || output.stdout_truncated || output.stdout.is_empty() {
        return Err("No usable primary selection after copy routing ended".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let response = send_ipc_req(writer, lines, &IpcRequest::PushText { text, hand }).await?;
    match response {
        IpcResponse::Success { .. } => {
            println!("Captured primary selection to {hand:?}");
            Ok(())
        }
        IpcResponse::Error { error } => Err(error.into()),
        _ => Err("Unexpected response to primary-selection push".into()),
    }
}

// Returns true when the watcher claimed the route, false when fallback is safe.
async fn finish_route(
    writer: &mut OwnedWriteHalf,
    lines: &mut Lines<BufReader<OwnedReadHalf>>,
    id: Uuid,
    sent_shortcut: bool,
) -> Result<bool, Error> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(350);
    if sent_shortcut {
        while tokio::time::Instant::now() < deadline {
            let status = request_route(
                writer,
                lines,
                IpcRequest::GetClipboardRoute { request_id: id },
                id,
            )
            .await?;
            match status {
                ClipboardRouteStatus::Claimed => {
                    return Ok(true);
                }
                ClipboardRouteStatus::Pending => {
                    tokio::time::sleep(Duration::from_millis(30)).await
                }
                ClipboardRouteStatus::Superseded => {
                    return Err("Copy route was superseded by another request".into());
                }
                _ => break,
            }
        }
    }

    // Atomic compare-and-cancel decides which side won the capture/fallback race.
    match request_route(
        writer,
        lines,
        IpcRequest::CancelClipboardRoute { request_id: id },
        id,
    )
    .await?
    {
        ClipboardRouteStatus::Claimed => {
            return Ok(true);
        }
        ClipboardRouteStatus::Cancelled | ClipboardRouteStatus::Expired => {}
        _ => return Err("Copy route changed; primary selection was not pushed".into()),
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_route_acknowledgements_require_identity_and_matching_protocol() {
        let id = Uuid::new_v4();
        assert!(
            route_status(
                IpcResponse::ClipboardRoute {
                    request_id: Uuid::new_v4(),
                    status: ClipboardRouteStatus::Pending
                },
                id
            )
            .is_err()
        );
        assert!(
            route_status(
                IpcResponse::Success {
                    message: "legacy".into()
                },
                id
            )
            .is_err()
        );
        assert_eq!(
            route_status(
                IpcResponse::ClipboardRoute {
                    request_id: id,
                    status: ClipboardRouteStatus::Claimed
                },
                id
            )
            .unwrap(),
            ClipboardRouteStatus::Claimed
        );
    }

    #[tokio::test]
    async fn clipboard_route_fallback_waits_for_atomic_cancellation_reply() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        for terminal in [
            ClipboardRouteStatus::Claimed,
            ClipboardRouteStatus::Cancelled,
            ClipboardRouteStatus::Expired,
            ClipboardRouteStatus::Superseded,
        ] {
            let id = Uuid::new_v4();
            let (client, server) = tokio::net::UnixStream::pair().unwrap();
            let (read, mut write) = client.into_split();
            let mut lines = BufReader::new(read).lines();
            let peer = tokio::spawn(async move {
                let (read, mut write) = server.into_split();
                let mut lines = BufReader::new(read).lines();
                let request: IpcRequest =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert!(
                    matches!(request, IpcRequest::CancelClipboardRoute { request_id } if request_id == id)
                );
                let response = IpcResponse::ClipboardRoute {
                    request_id: id,
                    status: terminal,
                };
                write
                    .write_all(
                        format!("{}\n", serde_json::to_string(&response).unwrap()).as_bytes(),
                    )
                    .await
                    .unwrap();
            });
            let result = finish_route(&mut write, &mut lines, id, false).await;
            match terminal {
                ClipboardRouteStatus::Claimed => assert!(result.unwrap()),
                ClipboardRouteStatus::Cancelled | ClipboardRouteStatus::Expired => {
                    assert!(!result.unwrap())
                }
                _ => assert!(result.is_err()),
            }
            peer.await.unwrap();
        }
    }
}
