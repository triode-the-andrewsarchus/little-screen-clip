//! Reload receipts name the accepted drive job instead of claiming completion.
use super::*;
use little_clip_core::drives::{DriveConfig, DriveOperation, DriveReply, DriveRequest};

fn drive_status(service: &clip_drives::Service) -> Box<little_clip_core::drives::DriveStatus> {
    let DriveReply::Status(status) = service.request(DriveRequest::Status { offset: 0 }).unwrap()
    else {
        panic!("expected drive status");
    };
    status
}

async fn completed(service: &clip_drives::Service, id: &str) -> DriveOperation {
    let started = tokio::time::Instant::now();
    loop {
        let status = drive_status(service);
        if let Some(operation) = status
            .operations
            .into_iter()
            .find(|op| op.id == id && op.finished.is_some())
        {
            return operation;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Drive reload never finished"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn ipc_reload_reports_accepted_jobs_and_preserves_later_drive_refusals() {
    if isolated_clipboard_test(
        "tests::drive_reload::ipc_reload_reports_accepted_jobs_and_preserves_later_drive_refusals",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let original = DriveConfig::default();
    let service =
        clip_drives::Service::start_fixture(original.clone(), temp.path().join("state"), source)
            .unwrap();
    let mut ctx = test_context(SlotManager::default());
    Arc::get_mut(&mut ctx).unwrap().drives = Some(service.clone());
    let (client, server) = UnixStream::pair().unwrap();
    let handler = tokio::spawn(handle_client(server, Arc::clone(&ctx)));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    for (sessions, expected) in [(0, "failed"), (4, "complete")] {
        std::fs::write(
            "config.toml",
            format!("[slots]\nleft_priors = 0\n[drives]\nram_max_sessions = {sessions}\n"),
        )
        .unwrap();
        let response = test_ipc_request(&mut writer, &mut lines, IpcRequest::ReloadConfig).await;
        let IpcResponse::Success { message } = response else {
            panic!("expected accepted reload, got {response:?}");
        };
        let status = drive_status(&service);
        let operation = status
            .operations
            .into_iter()
            .find(|op| message.contains(&op.id))
            .expect("Reload must disclose its accepted operation ID");
        assert!(message.contains("queued as operation"));
        assert!(message.contains("clipctl drives status"));
        let operation = completed(&service, &operation.id).await;
        assert_eq!(operation.state, expected, "{}", operation.message);
        let current = drive_status(&service);
        if expected == "failed" {
            assert!(operation.message.contains("RAM quotas"));
            assert_eq!(current.config, original);
        } else {
            assert_eq!(current.config.ram_max_sessions, sessions);
        }
        // Other settings are applied even when asynchronous drive validation fails.
        assert_eq!(ctx.manager.read().await.history_config.left_priors, 0);
    }
    let stopping = service.clone();
    tokio::task::spawn_blocking(move || stopping.shutdown())
        .await
        .unwrap()
        .unwrap();
    std::fs::write(
        "config.toml",
        "[slots]\nleft_priors = 1\n[drives]\nram_max_sessions = 5\n",
    )
    .unwrap();
    let response = test_ipc_request(&mut writer, &mut lines, IpcRequest::ReloadConfig).await;
    let IpcResponse::Error { error } = response else {
        panic!("expected drive admission refusal, got {response:?}");
    };
    assert!(error.contains("Application configuration applied"));
    assert!(error.contains("drive configuration was not accepted"));
    assert!(error.contains("admission is closed"));
    assert_eq!(ctx.manager.read().await.history_config.left_priors, 1);
    handler.abort();
}

#[tokio::test]
async fn unavailable_optional_drives_remain_disclosed_without_blocking_app_reload() {
    let response = reload_reply(
        drive_reply(
            None,
            DriveRequest::Configure {
                config: DriveConfig::default(),
            },
            None,
        )
        .await,
        false,
    );
    let IpcResponse::Success { message } = response else {
        panic!("expected app reload receipt");
    };
    assert!(message.contains("drive configuration was not applied"));
    assert!(message.contains("Drive services are unavailable"));
}
