//! A session collection of references and captured content, with optional shared slots.

pub(in crate::file_manager) mod preview;
mod view;

use super::content::{Payload, from_item};
use super::display_path;
use little_clip_core::ipc::{IpcRequest, IpcResponse, SlotStateSummary};
use little_clip_core::{BackgroundJobs, HandSide, JobError, SlotId};
use std::sync::mpsc;
use std::time::Duration;

struct Entry {
    id: uuid::Uuid,
    label: String,
    payload: Payload,
    availability: Option<String>,
}
enum ResultValue {
    Payload(Payload, String),
    Summary(SlotStateSummary),
    Sent,
    Checked(Vec<(uuid::Uuid, String)>),
}
type Receiver = mpsc::Receiver<Result<Result<ResultValue, String>, JobError>>;
struct Pending {
    receiver: Receiver,
    sending: bool,
}

pub(super) struct Shelf {
    pub open: bool,
    entries: Vec<Entry>,
    selected: Option<Selected>,
    previews: preview::Previews,
    inventory_error: Option<String>,
    checked_at: std::time::Instant,
    pending: Option<Pending>,
    inventory_pending: Option<Receiver>,
    summary: Option<SlotStateSummary>,
    pub notice: Option<String>,
}

impl Default for Shelf {
    fn default() -> Self {
        Self {
            open: true,
            entries: Vec::new(),
            selected: None,
            previews: preview::Previews::default(),
            inventory_error: None,
            checked_at: std::time::Instant::now() - Duration::from_millis(500),
            pending: None,
            inventory_pending: None,
            summary: None,
            notice: None,
        }
    }
}

impl Shelf {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn sending(&self) -> bool {
        self.pending.as_ref().is_some_and(|pending| pending.sending)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn add(&mut self, mut payload: Payload, label: String) {
        if let Payload::Files { paths, .. } = &mut payload {
            if paths.is_empty() {
                self.notice = Some("Select files before collecting a group.".into());
                return;
            }
            paths.sort();
            paths.dedup();
        }
        if let Payload::Files { paths, cut } = &payload
            && self.entries.iter().any(|entry| matches!(&entry.payload, Payload::Files { paths: held, cut: intent } if held == paths && intent == cut)) {
            return;
        }
        let bytes: usize = self
            .entries
            .iter()
            .map(|entry| match &entry.payload {
                Payload::Content(content) => content.bytes.len(),
                _ => 0,
            })
            .sum();
        let count: usize = self
            .entries
            .iter()
            .map(|entry| match &entry.payload {
                Payload::Files { paths, .. } => paths.len(),
                _ => 0,
            })
            .sum();
        let fits = match &payload {
            Payload::Content(content) => bytes + content.bytes.len() <= 128 * 1024 * 1024,
            Payload::Files { paths, .. } => !paths.is_empty() && count + paths.len() <= 10_000,
        };
        if self.entries.len() >= 32 || !fits {
            self.notice = Some("Shelf is full (32 groups, 10,000 paths, or 128 MiB). Remove a group before collecting more.".into());
        } else {
            let id = uuid::Uuid::new_v4();
            self.selected = Some(Selected::Collected(id));
            self.entries.push(Entry {
                id,
                label,
                payload,
                availability: None,
            });
            self.notice = None;
        }
    }

    fn submit(
        &mut self,
        sending: bool,
        work: impl FnOnce() -> Result<ResultValue, String> + Send + 'static,
    ) {
        if self.busy() {
            self.notice = Some("Wait for the current Shelf request to finish.".into());
            return;
        }
        let (_, receiver) = BackgroundJobs::global().submit(
            if sending {
                Duration::MAX
            } else {
                Duration::from_secs(15)
            },
            work,
        );
        self.pending = Some(Pending { receiver, sending });
        self.notice = None;
    }

    pub fn tick(&mut self) {
        self.tick_inventory();
        let Some(pending) = &self.pending else {
            return;
        };
        let Some(result) = super::poll_job(&pending.receiver) else {
            return;
        };
        self.pending = None;
        match result {
            Ok(ResultValue::Payload(payload, label)) => self.add(payload, label),
            Ok(ResultValue::Summary(_)) => {}
            Ok(ResultValue::Sent) => {
                self.notice = Some("Sent to the shared hand.".into());
                self.checked_at = std::time::Instant::now() - Duration::from_millis(500);
            }
            Ok(ResultValue::Checked(results)) => {
                for (id, message) in results {
                    if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                        entry.availability = Some(message);
                        self.previews.invalidate(id);
                    }
                }
            }
            Err(error) => self.notice = Some(error),
        }
    }

    fn refresh_inventory(&mut self) {
        if self.inventory_pending.is_some() {
            return;
        }
        self.checked_at = std::time::Instant::now();
        let (_, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(15), || {
                match ipc(IpcRequest::GetStateSummary)? {
                    IpcResponse::StateSummary(summary) => Ok(ResultValue::Summary(summary)),
                    _ => Err("Unexpected shared inventory response".into()),
                }
            });
        self.inventory_pending = Some(receiver);
    }

    fn tick_inventory(&mut self) {
        let Some(receiver) = &self.inventory_pending else {
            return;
        };
        let Some(result) = super::poll_job(receiver) else {
            return;
        };
        self.inventory_pending = None;
        match result {
            Ok(ResultValue::Summary(summary)) => {
                if let Some(previous) = &self.summary {
                    for card in &summary.cards {
                        if previous.card(card.slot) != Some(card) {
                            self.previews.invalidate(card.item_id);
                        }
                    }
                }
                self.summary = Some(summary);
                self.inventory_error = None;
            }
            Err(error) => {
                self.inventory_error = Some(error);
                self.summary = None;
            }
            _ => {}
        }
    }

    fn collect_slot(&mut self, slot: SlotId, expected: uuid::Uuid) {
        self.submit(false, move || match ipc(IpcRequest::GetSlotItem(slot))? {
            IpcResponse::SlotItem(Some(item)) if item.id == expected => {
                let payload = from_item(&item)?;
                Ok(ResultValue::Payload(
                    payload,
                    format!("{}: {}", slot.label(), item.title),
                ))
            }
            _ => {
                Err("Shared slot changed or is empty; refresh inventory and collect again.".into())
            }
        });
    }

    fn check_references(&mut self) {
        let groups: Vec<_> = self
            .entries
            .iter()
            .filter_map(|entry| match &entry.payload {
                Payload::Files { paths, .. } => Some((entry.id, paths.clone())),
                _ => None,
            })
            .collect();
        self.submit(false, move || {
            Ok(ResultValue::Checked(
                groups
                    .into_iter()
                    .map(|(index, paths)| {
                        let unavailable: Vec<_> = paths
                            .iter()
                            .filter(|path| std::fs::symlink_metadata(path).is_err())
                            .collect();
                        let message = if unavailable.is_empty() {
                            "All references available at last check".into()
                        } else {
                            format!(
                                "{} unavailable: {}",
                                unavailable.len(),
                                unavailable
                                    .iter()
                                    .take(3)
                                    .map(|path| display_path(path))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        (index, message)
                    })
                    .collect(),
            ))
        });
    }

    pub fn consume_moves(&mut self, completed: &[std::path::PathBuf]) {
        for entry in self.entries.iter_mut() {
            if let Payload::Files { paths, cut: true } = &mut entry.payload {
                let previous = paths.len();
                paths.retain(|path| !completed.iter().any(|source| path.starts_with(source)));
                if previous != paths.len() {
                    self.previews.invalidate(entry.id);
                    entry.availability = None;
                }
            }
        }
        self.entries.retain(
            |entry| !matches!(&entry.payload, Payload::Files { paths, .. } if paths.is_empty()),
        );
    }

    fn selected_entry_mut(&mut self) -> Option<&mut Entry> {
        match self.selected? {
            Selected::Collected(id) => self.entries.iter_mut().find(|entry| entry.id == id),
            Selected::Slot(_) => None,
        }
    }
}

fn ipc(request: IpcRequest) -> Result<IpcResponse, String> {
    let response = crate::desktop_ipc::request(
        &request,
        Some(Duration::from_secs(2)),
        96 * 1024 * 1024,
        1024 * 1024,
    )
    .map_err(|error| {
        format!("Shared inventory unavailable: {error}. Local Shelf and clipboard still work.")
    })?;
    if let IpcResponse::Error { error } = response {
        Err(error)
    } else {
        Ok(response)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Selected {
    Collected(uuid::Uuid),
    Slot(SlotId),
}

enum Action {
    Select(Selected),
    Place(Payload),
    Remove(Selected),
    CollectSlot(SlotId, uuid::Uuid),
    CollectSelection,
    Capture,
    Check,
    Refresh,
    Send(Payload, HandSide),
}

/// Preserve references while translating a chosen Shelf payload for IPC.
fn send_request(payload: Payload, hand: HandSide) -> Result<IpcRequest, String> {
    Ok(match payload {
        Payload::Files { paths, .. } => IpcRequest::PushFileReferences { paths, hand },
        Payload::Content(content) if content.mime.starts_with("image/") => IpcRequest::PushImage {
            bytes: content.bytes.to_vec(),
            mime: content.mime,
            hand,
        },
        Payload::Content(content) => IpcRequest::PushText {
            text: String::from_utf8(content.bytes.to_vec()).map_err(|error| error.to_string())?,
            hand,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_send_never_hydrates_file_references_including_single_images() {
        for paths in [
            vec!["/proc/uptime".into()],
            vec!["/missing/image.png".into()],
            vec!["/dev/zero".into(), "/sys".into()],
        ] {
            let request = send_request(
                Payload::Files {
                    paths: paths.clone(),
                    cut: true,
                },
                HandSide::Left,
            )
            .unwrap();
            let IpcRequest::PushFileReferences { paths: sent, hand } = request else {
                panic!("Shelf file groups must use reference-only IPC");
            };
            assert_eq!(sent, paths);
            assert_eq!(hand, HandSide::Left);
        }
    }

    #[test]
    fn shelf_send_uses_captured_bytes_without_source_paths() {
        let content =
            super::super::content::Content::capture(b"snapshot".to_vec(), "text/plain").unwrap();
        let request = send_request(Payload::Content(content), HandSide::Right).unwrap();
        let IpcRequest::PushText { text, hand } = request else {
            panic!("captured text must remain text");
        };
        assert_eq!(text, "snapshot");
        assert_eq!(hand, HandSide::Right);
    }
}
