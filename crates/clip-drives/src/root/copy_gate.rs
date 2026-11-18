//! One-shot test interleavings at actual streamed staging writes; absent in binaries.
use super::*;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

static GATES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

struct Entry {
    token: Arc<()>,
    root: Scope,
    target: PathBuf,
    after: u64,
    entered: mpsc::SyncSender<Checkpoint>,
    resume: mpsc::Receiver<()>,
}
pub(crate) struct Checkpoint {
    pub file: File,
    pub path: PathBuf,
    pub written: u64,
}
pub(crate) struct Gate {
    token: Arc<()>,
    entered: mpsc::Receiver<Checkpoint>,
    resume: Option<mpsc::Sender<()>>,
}
impl Gate {
    pub fn arm(root: &Root, target: &Path, after: u64) -> Self {
        root.validate().unwrap();
        assert!(root.namespace.known() && valid_relative(target) && after > 0);
        let (notify, entered) = mpsc::sync_channel(1);
        let (resume, wait) = mpsc::channel();
        let token = Arc::new(());
        let entry = Entry {
            token: token.clone(),
            root: root.scope(),
            target: target.into(),
            after,
            entered: notify,
            resume: wait,
        };
        let mut gates = GATES.lock().unwrap();
        assert!(gates.len() < 16, "Copy gate registration budget exceeded");
        gates.push(entry);
        Self {
            token,
            entered,
            resume: Some(resume),
        }
    }
    pub fn wait(&self) -> Checkpoint {
        self.entered
            .recv_timeout(Duration::from_secs(15))
            .expect("copy never reached its actual staging checkpoint")
    }
    pub fn release(&mut self) {
        self.resume.take();
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        GATES
            .lock()
            .unwrap()
            .retain(|entry| !Arc::ptr_eq(&entry.token, &self.token));
        self.release();
    }
}
struct Pending {
    entry: Entry,
    file: File,
    path: PathBuf,
    written: u64,
}
pub(super) fn progress(root: &Root, target: &Path, file: &File) -> Result<impl FnMut(u64) + use<>> {
    let scope = root.scope();
    let entry = {
        let mut gates = GATES.lock().unwrap();
        gates
            .iter()
            .position(|entry| entry.root == scope && entry.target == target)
            .map(|index| gates.swap_remove(index))
    };
    let mut pending = entry
        .map(|entry| -> Result<_> {
            Ok(Pending {
                entry,
                file: file.try_clone()?,
                path: fs::read_link(anchor(file))?,
                written: 0,
            })
        })
        .transpose()?;
    Ok(move |bytes| {
        let reached = pending.as_mut().is_some_and(|pending| {
            pending.written += bytes;
            pending.written >= pending.entry.after
        });
        if reached {
            let pending = pending.take().unwrap();
            pending
                .file
                .sync_all()
                .expect("staging checkpoint fsync failed before removal");
            let _ = pending.entry.entered.send(Checkpoint {
                file: pending.file,
                path: pending.path,
                written: pending.written,
            });
            // A dropped controller releases the accepted writer too. Never keep
            // the registry mutex or silently retire write ownership while held.
            let _ = pending.entry.resume.recv_timeout(Duration::from_secs(20));
        }
    })
}
