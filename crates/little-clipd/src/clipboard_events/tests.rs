//! A private Wayland compositor exercises offer identity and FD transfers.
use super::*;
use std::io::Read;
use wayland_protocols_wlr::data_control::v1::server::{
    zwlr_data_control_device_v1 as server_device, zwlr_data_control_manager_v1 as server_manager,
    zwlr_data_control_offer_v1 as server_offer, zwlr_data_control_source_v1 as server_source,
};
use wayland_server::{
    Client, DataInit, Dispatch as ServerDispatch, Display, DisplayHandle, GlobalDispatch, New,
    Resource, protocol::wl_seat as server_seat,
};

enum Change {
    Copy(Vec<u8>, String),
    Read(UnixStream),
    Replay(Vec<u8>),
}
struct Server {
    device: Option<server_device::ZwlrDataControlDeviceV1>,
    source: Option<server_source::ZwlrDataControlSourceV1>,
    publications: Arc<AtomicU64>,
}
#[derive(Clone)]
enum Data {
    External(Arc<[u8]>),
    Daemon(server_source::ZwlrDataControlSourceV1),
}
impl Server {
    fn offer(&mut self, handle: &DisplayHandle, data: Data, types: Vec<String>) {
        let device = self.device.as_ref().unwrap();
        let offer = device
            .client()
            .unwrap()
            .create_resource::<server_offer::ZwlrDataControlOfferV1, Data, Self>(handle, 1, data)
            .unwrap();
        device.data_offer(&offer);
        for mime in types {
            offer.offer(mime);
        }
        device.selection(Some(&offer));
    }
}
// Both advertised globals initialize their resource with stateless user data.
impl<I: Resource + 'static> GlobalDispatch<I, ()> for Server
where
    Self: ServerDispatch<I, ()>,
{
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<I>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}
impl ServerDispatch<server_seat::WlSeat, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &server_seat::WlSeat,
        _: server_seat::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}
impl ServerDispatch<server_manager::ZwlrDataControlManagerV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &server_manager::ZwlrDataControlManagerV1,
        request: server_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            server_manager::Request::GetDataDevice { id, .. } => {
                let device = init.init(id, ());
                device.selection(None);
                device.primary_selection(None);
                state.device = Some(device);
            }
            server_manager::Request::CreateDataSource { id } => {
                init.init(id, Mutex::new(Vec::<String>::new()));
            }
            _ => {}
        }
    }
}
impl ServerDispatch<server_device::ZwlrDataControlDeviceV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &server_device::ZwlrDataControlDeviceV1,
        request: server_device::Request,
        _: &(),
        handle: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let server_device::Request::SetSelection {
            source: Some(source),
        } = request
        {
            if let Some(old) = state.source.replace(source.clone()) {
                old.cancelled();
            }
            let types = source
                .data::<Mutex<Vec<String>>>()
                .unwrap()
                .lock()
                .unwrap()
                .clone();
            state.offer(handle, Data::Daemon(source), types);
            state.publications.fetch_add(1, Ordering::Release);
        }
    }
}
impl ServerDispatch<server_source::ZwlrDataControlSourceV1, Mutex<Vec<String>>> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &server_source::ZwlrDataControlSourceV1,
        request: server_source::Request,
        types: &Mutex<Vec<String>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let server_source::Request::Offer { mime_type } = request {
            types.lock().unwrap().push(mime_type);
        }
    }
}
impl ServerDispatch<server_offer::ZwlrDataControlOfferV1, Data> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &server_offer::ZwlrDataControlOfferV1,
        request: server_offer::Request,
        data: &Data,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let server_offer::Request::Receive { mime_type, fd } = request {
            match data {
                Data::External(bytes) => {
                    File::from(fd).write_all(bytes).unwrap();
                }
                Data::Daemon(source) => source.send(mime_type, fd.as_fd()),
            }
        }
    }
}
struct Fixture {
    changes: mpsc::Sender<Change>,
    publications: Arc<AtomicU64>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Disconnect the server command loop before joining it.
        let (replacement, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.changes, replacement));
        self.worker.take().unwrap().join().unwrap();
    }
}
fn fixture() -> (Clipboard, watch::Receiver<Option<Arc<Incoming>>>, Fixture) {
    let (client, server) = UnixStream::pair().unwrap();
    let mut display = Display::<Server>::new().unwrap();
    let mut handle = display.handle();
    handle.insert_client(server, Arc::new(())).unwrap();
    handle.create_global::<Server, server_seat::WlSeat, ()>(1, ());
    handle.create_global::<Server, server_manager::ZwlrDataControlManagerV1, ()>(2, ());
    let (changes, receiver) = mpsc::channel();
    let publications = Arc::new(AtomicU64::new(0));
    let observed = publications.clone();
    let worker = std::thread::spawn(move || {
        let mut state = Server {
            device: None,
            source: None,
            publications: observed,
        };
        loop {
            display.dispatch_clients(&mut state).unwrap();
            if state.device.is_some() {
                match receiver.try_recv() {
                    Ok(Change::Copy(bytes, mime)) => {
                        if let Some(source) = state.source.take() {
                            source.cancelled();
                        }
                        state.offer(&handle, Data::External(bytes.into()), vec![mime]);
                    }
                    Ok(Change::Replay(bytes)) => {
                        let source = state.source.take().unwrap();
                        let types = source
                            .data::<Mutex<Vec<String>>>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .clone();
                        source.cancelled();
                        state.offer(&handle, Data::External(bytes.into()), types);
                    }
                    Ok(Change::Read(fd)) => state
                        .source
                        .as_ref()
                        .unwrap()
                        .send("text/plain".into(), fd.as_fd()),
                    Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            display.flush_clients().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    let (clipboard, selections) = Clipboard::spawn_on(move || {
        Connection::from_socket(client).map_err(|error| error.to_string())
    })
    .unwrap();
    (
        clipboard,
        selections,
        Fixture {
            changes,
            publications,
            worker: Some(worker),
        },
    )
}
async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn identical_external_offers_are_distinct_and_tagged_publications_do_not_echo() {
    let (clipboard, mut selections, server) = fixture();
    // The initial empty selection is a baseline, and never claims a pending copy.
    selections.changed().await.unwrap();
    assert!(selections.borrow_and_update().is_none());
    let bytes = b"same selection".to_vec();
    let mut previous = 0;
    for _ in 0..2 {
        server
            .changes
            .send(Change::Copy(bytes.clone(), "text/plain".into()))
            .unwrap();
        selections.changed().await.unwrap();
        let incoming = selections.borrow_and_update().clone().unwrap();
        assert!(!incoming.initial);
        assert!(incoming.serial > previous);
        assert_eq!(incoming.read().await.unwrap().0, bytes);
        previous = incoming.serial;
    }
    assert!(
        clipboard
            .publish(bytes.clone().into(), "text/plain;charset=utf-8".into())
            .await
            .unwrap()
    );
    selections.changed().await.unwrap();
    assert!(
        selections.borrow_and_update().is_none(),
        "daemon marker suppresses its own offer"
    );
    assert!(!clipboard.is_current(previous));
    let (mut reader, writer) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    server.changes.send(Change::Read(writer)).unwrap();
    let transferred = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        bytes
    })
    .await
    .unwrap();
    assert_eq!(transferred, bytes);
    server.changes.send(Change::Replay(bytes.clone())).unwrap();
    selections.changed().await.unwrap();
    let replay = selections
        .borrow_and_update()
        .clone()
        .expect("retired source marker cannot suppress a later offer");
    assert_eq!(replay.read().await.unwrap().0, bytes);
    // A user copying bytes equal to the daemon's publication remains observable.
    server
        .changes
        .send(Change::Copy(bytes.clone(), "text/plain".into()))
        .unwrap();
    selections.changed().await.unwrap();
    let incoming = selections.borrow_and_update().clone().unwrap();
    assert_eq!(incoming.read().await.unwrap().0, bytes);
    assert_eq!(server.publications.load(Ordering::Acquire), 1);
    drop(clipboard);
}

#[tokio::test]
async fn repeated_identical_text_and_images_claim_only_new_routes() {
    use little_clip_core::{ClipPayload, HandSide, SlotManager, ipc::ClipboardRouteStatus};
    let (clipboard, selections, server) = fixture();
    let mut ctx = super::super::tests::test_context(SlotManager::default());
    Arc::get_mut(&mut ctx).unwrap().clipboard = Some(clipboard);
    let watcher = tokio::spawn(super::super::watch_clipboard_events(
        ctx.clone(),
        selections,
    ));
    wait_for(|| {
        ctx.clipboard
            .as_ref()
            .unwrap()
            .serial
            .load(Ordering::Acquire)
            > 0
    })
    .await;
    let mut png = Vec::new();
    image_fixture(&mut png);
    for (bytes, mime) in [(b"equal text".to_vec(), "text/plain"), (png, "image/png")] {
        let matches = |item: &little_clip_core::ClipItem| match &item.payload {
            ClipPayload::Text(text) => text.as_bytes() == bytes,
            ClipPayload::Image { bytes: found, .. } => found == &bytes,
            _ => false,
        };
        server
            .changes
            .send(Change::Copy(bytes.clone(), mime.into()))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if ctx.manager.read().await.left_hand().is_some_and(&matches) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let id = uuid::Uuid::new_v4();
        let response = ctx
            .arm_clipboard_route(Some(id), HandSide::Right, 10_000, false)
            .await;
        assert!(matches!(
            response,
            little_clip_core::ipc::IpcResponse::ClipboardRoute {
                status: ClipboardRouteStatus::Pending,
                ..
            }
        ));
        let own = ctx.manager.read().await.left_hand().unwrap().clone();
        super::super::copy_to_os_clipboard(&own, &ctx).await;
        assert!(ctx.clipboard.as_ref().unwrap().barrier().await.unwrap());
        assert_eq!(
            ctx.route_next_clipboard
                .read()
                .await
                .status(id, tokio::time::Instant::now()),
            ClipboardRouteStatus::Pending
        );
        server
            .changes
            .send(Change::Copy(bytes.clone(), mime.into()))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if ctx
                    .route_next_clipboard
                    .read()
                    .await
                    .status(id, tokio::time::Instant::now())
                    == ClipboardRouteStatus::Claimed
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let manager = ctx.manager.read().await;
        assert!(manager.right_hand().is_some_and(matches));
    }
    watcher.abort();
    let _ = watcher.await;
    for slot in little_clip_core::SlotId::all() {
        if let Some(item) = ctx.manager.read().await.get(slot)
            && let ClipPayload::Image {
                path: Some(path), ..
            } = &item.payload
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn image_fixture(bytes: &mut Vec<u8>) {
    // A real one-pixel PNG, copied through the protocol and ordinary image reader.
    bytes.extend_from_slice(&[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99, 248, 207, 192, 240, 31,
        0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]);
}

#[tokio::test]
async fn a_lost_selection_connection_rejects_new_routes() {
    use little_clip_core::{HandSide, SlotManager, ipc::IpcResponse};
    let (clipboard, mut selections, server) = fixture();
    assert!(clipboard.barrier().await.unwrap());
    let mut ctx = super::super::tests::test_context(SlotManager::default());
    Arc::get_mut(&mut ctx).unwrap().clipboard = Some(clipboard);
    drop(server);
    while selections.changed().await.is_ok() {
        selections.borrow_and_update();
    }
    let response = ctx
        .arm_clipboard_route(Some(uuid::Uuid::new_v4()), HandSide::Right, 2000, false)
        .await;
    assert!(matches!(response, IpcResponse::Error { .. }));
}

#[tokio::test]
async fn poisoned_selection_state_reports_a_read_failure_instead_of_panicking() {
    let incoming = Incoming {
        serial: 1,
        observed: tokio::time::Instant::now(),
        initial: false,
        mime: "text/plain".into(),
        reader: Mutex::new(None),
    };
    assert!(
        std::panic::catch_unwind(|| {
            let _guard = incoming.reader.lock().unwrap();
            panic!("simulate a selection-state failure");
        })
        .is_err()
    );
    assert_eq!(
        incoming.read().await.unwrap_err(),
        "Clipboard selection state is unavailable"
    );
}
