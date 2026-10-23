//! Opt-in controller input. Enumeration and connection lifetimes stay off the UI thread.
//!
//! MIDI notes 36–51 map left to right, bottom to top. Note-off and velocity-zero
//! messages do not retrigger these one-shot pads. Input is gated by view visibility;
//! an epoch rejects notes queued before navigation, Escape, or saved-pack browsing.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};

#[derive(Clone, Default)]
pub(super) struct Status {
    pub ports: Vec<String>,
    pub connected: Option<usize>,
    pub error: Option<String>,
}

enum Command {
    Refresh,
    Connect(usize, String),
    Disconnect,
}

pub(super) struct Controller {
    commands: mpsc::SyncSender<Command>,
    notes: mpsc::Receiver<(u64, usize, u8)>,
    status: Arc<Mutex<Status>>,
    active: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
}

impl Controller {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        let (commands, requests) = mpsc::sync_channel(8);
        let (events, notes) = mpsc::sync_channel(128);
        let status = Arc::new(Mutex::new(Status::default()));
        let active = Arc::new(AtomicBool::new(false));
        let epoch = Arc::new(AtomicU64::new(0));
        let state = status.clone();
        let gate = active.clone();
        let generation = epoch.clone();
        std::thread::Builder::new()
            .name("constellation-midi".into())
            .spawn(move || {
                let mut connection = None;
                let mut command = Command::Refresh;
                loop {
                    let result = execute(
                        command,
                        &mut connection,
                        &state,
                        &events,
                        &gate,
                        &generation,
                        &ctx,
                    );
                    if let Ok(mut status) = state.lock() {
                        status.error = result.err();
                    }
                    ctx.request_repaint();
                    let Ok(next) = requests.recv() else {
                        break;
                    };
                    command = next;
                }
                // Closing midir may join its callback thread; keep that off the UI.
                drop(connection);
            })
            .map_err(|error| format!("Cannot start MIDI input: {error}"))?;
        Ok(Self {
            commands,
            notes,
            status,
            active,
            epoch,
        })
    }

    pub fn status(&self) -> Status {
        self.status
            .try_lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn refresh(&self) {
        self.send(Command::Refresh);
    }
    pub fn connect(&self, index: usize, name: String) {
        self.send(Command::Connect(index, name));
    }
    pub fn disconnect(&self) {
        self.suspend();
        self.send(Command::Disconnect);
    }

    fn send(&self, command: Command) {
        if self.commands.try_send(command).is_err()
            && let Ok(mut status) = self.status.try_lock()
        {
            status.error = Some("MIDI input is busy; try again".into());
        }
    }

    pub fn suspend(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn poll(&self, visible: bool) -> Vec<(usize, u8)> {
        if !visible {
            self.suspend();
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let notes = self
            .notes
            .try_iter()
            .filter_map(|(generation, pad, velocity)| {
                (visible && generation == epoch).then_some((pad, velocity))
            })
            .collect();
        self.active.store(visible, Ordering::SeqCst);
        notes
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.suspend();
    }
}

fn execute(
    command: Command,
    connection: &mut Option<midir::MidiInputConnection<()>>,
    state: &Mutex<Status>,
    events: &mpsc::SyncSender<(u64, usize, u8)>,
    active: &Arc<AtomicBool>,
    epoch: &Arc<AtomicU64>,
    ctx: &egui::Context,
) -> Result<(), String> {
    if matches!(command, Command::Disconnect) {
        *connection = None;
        state.lock().map_err(|e| e.to_string())?.connected = None;
        return Ok(());
    }
    let mut input = midir::MidiInput::new("Little Clip sample pads").map_err(|e| e.to_string())?;
    input.ignore(midir::Ignore::All);
    let ports = input.ports();
    let names: Vec<_> = ports
        .iter()
        .take(64)
        .map(|port| {
            input
                .port_name(port)
                .unwrap_or_else(|_| "Unavailable input".into())
        })
        .collect();
    if let Command::Connect(index, expected) = command {
        // A refresh or device removal may have changed port ordering.
        if names.get(index) != Some(&expected) {
            return Err("MIDI inputs changed; refresh and select again".into());
        }
        *connection = None;
        state.lock().map_err(|e| e.to_string())?.connected = None;
        let events = events.clone();
        let active = active.clone();
        let epoch = epoch.clone();
        let ctx = ctx.clone();
        *connection = Some(
            input
                .connect(
                    &ports[index],
                    "Little Clip drum rack",
                    move |_, message, _| {
                        let generation = epoch.load(Ordering::SeqCst);
                        if active.load(Ordering::SeqCst)
                            && let Some((pad, velocity)) = note_hit(message)
                            && events.try_send((generation, pad, velocity)).is_ok()
                        {
                            ctx.request_repaint();
                        }
                    },
                    (),
                )
                .map_err(|e| e.to_string())?,
        );
        state.lock().map_err(|e| e.to_string())?.connected = Some(index);
    } else {
        // Port indices only describe this inventory, so close the old connection.
        *connection = None;
        state.lock().map_err(|e| e.to_string())?.connected = None;
    }
    state.lock().map_err(|e| e.to_string())?.ports = names;
    Ok(())
}

fn note_hit(message: &[u8]) -> Option<(usize, u8)> {
    let &[status, note, velocity] = message else {
        return None;
    };
    (status & 0xf0 == 0x90 && (36..52).contains(&note) && (1..=127).contains(&velocity))
        .then(|| ((note - 36) as usize, velocity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drum_notes_cover_all_pads_and_preserve_velocity_on_every_channel() {
        for channel in 0..16 {
            for note in 36..52 {
                assert_eq!(
                    note_hit(&[0x90 | channel, note, 42]),
                    Some(((note - 36) as usize, 42))
                );
            }
        }
        for message in [
            &[0x90, 36, 0][..],
            &[0x80, 36, 127],
            &[0x90, 35, 127],
            &[0x90, 52, 127],
            &[0x90, 36],
            &[0x90, 36, 255],
        ] {
            assert_eq!(note_hit(message), None);
        }
    }

    #[test]
    fn suspended_epoch_rejects_pre_navigation_hits() {
        let (commands, _requests) = mpsc::sync_channel(8);
        let (events, notes) = mpsc::sync_channel(128);
        let controller = Controller {
            commands,
            notes,
            status: Default::default(),
            active: Arc::new(AtomicBool::new(true)),
            epoch: Arc::new(AtomicU64::new(0)),
        };
        events.send((0, 0, 100)).unwrap();
        controller.suspend();
        assert!(controller.poll(true).is_empty());
        events.send((1, 15, 64)).unwrap();
        assert_eq!(controller.poll(true), vec![(15, 64)]);
        events.send((1, 0, 127)).unwrap();
        assert!(controller.poll(false).is_empty());
    }
}
