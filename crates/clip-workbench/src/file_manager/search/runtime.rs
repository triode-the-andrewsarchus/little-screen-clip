//! One owned, cancellable model process; prepared bytes are its only content input.
use super::store::DIMENSION;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(super) const ANSWER_PROVIDER: &str = "qwen3.5-9b";

#[derive(Clone)]
pub(super) struct Paths {
    pub data: PathBuf,
    pub helpers: PathBuf,
}
impl Paths {
    pub fn for_user(home: &Path) -> Self {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"))
            .join("little-screen-clip/search");
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/search_model");
        Self {
            data,
            helpers: bundled,
        }
    }
    /// Resolve packaged helpers on the worker, independently of the launch directory.
    pub fn resolve_helpers(&mut self) -> Result<(), String> {
        let worker = helper_script("worker.py", &self.helpers)?;
        let parent = worker.parent().ok_or("Cannot locate search helpers")?;
        if !parent.join("setup.py").is_file() {
            return Err("Search setup helper is not installed".into());
        }
        self.helpers = parent.into();
        Ok(())
    }
    pub fn runtime(&self) -> PathBuf {
        self.data.join("runtime")
    }
    pub fn models(&self) -> PathBuf {
        self.data.join("models")
    }
    pub fn configured(&self, role: Role) -> bool {
        let files: &[&str] = match role {
            Role::Embedding => &[
                "embeddinggemma-2-Q8_0.gguf",
                "mmproj-embeddinggemma-2-Q8_0.gguf",
            ],
            Role::Answer => &["Qwen_Qwen3.5-9B-Q8_0.gguf"],
        };
        self.runtime().join("lib/libclip-search.so").is_file()
            && (role == Role::Embedding || self.runtime().join("lib/libclip-answer.so").is_file())
            && files.iter().all(|name| self.models().join(name).is_file())
    }
}

pub(super) fn helper_script(name: &str, fallback: &Path) -> Result<PathBuf, String> {
    let here = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = here.parent().ok_or("Cannot locate search helpers")?;
    for candidate in [
        parent.join("search_model"),
        parent.join("../share/little-screen-clip/search_model"),
        fallback.into(),
    ] {
        let script = candidate.join(name);
        if script.is_file() {
            return Ok(script);
        }
    }
    Err(format!("Search helper {name} is not installed"))
}

type Replies = mpsc::Receiver<Result<Value, ModelFailure>>;
pub(super) struct Model {
    child: Child,
    input: ChildStdin,
    replies: Replies,
    reader: Option<JoinHandle<()>>,
    pub info: Value,
    pub last_used: Instant,
    next_id: u64,
    failed: bool,
    pub role: Role,
    paths: Paths,
    fallback_from: Vec<String>,
}
const BACKENDS: [&str; 3] = ["cuda", "vulkan", "cpu"];

enum ModelFailure {
    Request(String),
    Backend(String),
}
impl ModelFailure {
    fn into_message(self) -> String {
        match self {
            Self::Request(message) | Self::Backend(message) => message,
        }
    }
}
impl From<String> for ModelFailure {
    fn from(message: String) -> Self {
        Self::Request(message)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Embedding,
    Answer,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Embedding => "embedding",
            Self::Answer => "answer",
        }
    }
}
impl Model {
    pub fn start_preferred(paths: &Paths, role: Role, token: &AtomicBool) -> Result<Self, String> {
        let mut failures = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(120);
        for backend in BACKENDS {
            if token.load(Ordering::Relaxed) {
                return Err("Model loading cancelled".into());
            }
            match Self::start(paths, backend, role, deadline, token) {
                Ok(model) => return Ok(model),
                Err(error) => failures.push(format!("{backend}: {error}")),
            }
        }
        if token.load(Ordering::Relaxed) {
            return Err("Model loading cancelled".into());
        }
        check_deadline(deadline, token)?;
        Err(format!(
            "No inference backend could load the model: {}",
            failures.join("; ")
        ))
    }
    fn start(
        paths: &Paths,
        backend: &str,
        role: Role,
        deadline: Instant,
        token: &AtomicBool,
    ) -> Result<Self, String> {
        check_deadline(deadline, token)?;
        let mut request = little_clip_core::process::CommandRequest::new("python3")
            .die_with_parent()
            .arg("-I")
            .arg(paths.helpers.join("worker.py"))
            .arg("--runtime")
            .arg(paths.runtime())
            .arg("--models")
            .arg(paths.models())
            .arg("--parent")
            .arg(std::process::id().to_string())
            .arg("--device")
            .arg(backend)
            .arg("--role")
            .arg(role.name());
        if role == Role::Answer {
            request = request.arg("--answer-provider").arg(ANSWER_PROVIDER);
        }
        let mut command = request.build_command();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Model input/output pipes unavailable".into());
        };
        let flags = unsafe { libc::fcntl(input.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(input.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                != 0
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::last_os_error().to_string());
        }
        let (replies, reader) = match reply_reader(output) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let mut model = Self {
            child,
            input,
            replies,
            reader: Some(reader),
            info: Value::Null,
            last_used: Instant::now(),
            next_id: 0,
            failed: false,
            role,
            paths: paths.clone(),
            fallback_from: Vec::new(),
        };
        let ready = model
            .receive(deadline, token)
            .map_err(ModelFailure::into_message)?;
        let compatible = match role {
            Role::Embedding => ready["dimension"].as_u64() == Some(DIMENSION as u64),
            Role::Answer => {
                ready["role"] == "answer" && ready["pipeline"]["provider"] == ANSWER_PROVIDER
            }
        };
        if ready["ready"] != true || !compatible {
            return Err(ready["error"]
                .as_str()
                .unwrap_or("Incompatible model helper")
                .into());
        }
        if ready["backend"].as_str() != Some(backend) {
            return Err("Model helper selected a different backend".into());
        }
        model.info = ready;
        Ok(model)
    }
    fn receive(&self, deadline: Instant, token: &AtomicBool) -> Result<Value, ModelFailure> {
        loop {
            check_deadline(deadline, token)?;
            match self.replies.recv_timeout(Duration::from_millis(25)) {
                Ok(reply) => return reply,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(ModelFailure::Backend("Model helper disconnected".into())),
            }
        }
    }
    fn exchange(
        &mut self,
        request: Value,
        timeout: Duration,
        token: &AtomicBool,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.exchange_once(request.clone(), deadline, token) {
                Ok(response) => {
                    self.last_used = Instant::now();
                    return Ok(response);
                }
                Err(failure) => {
                    let recoverable = matches!(failure, ModelFailure::Backend(_));
                    let message = failure.into_message();
                    self.retire();
                    check_deadline(deadline, token)?;
                    if !recoverable || self.info["backend"] == "cpu" {
                        return Err(message);
                    }
                    self.recover(deadline, token, message)?;
                }
            }
        }
    }
    fn exchange_once(
        &mut self,
        mut request: Value,
        deadline: Instant,
        token: &AtomicBool,
    ) -> Result<Value, ModelFailure> {
        if self.failed {
            return Err(ModelFailure::Request(
                "Model helper must be restarted".into(),
            ));
        }
        check_deadline(deadline, token)?;
        self.next_id = self.next_id.wrapping_add(1);
        request["id"] = json!(self.next_id);
        let mut bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            check_deadline(deadline, token)?;
            match self.input.write(remaining) {
                Ok(0) => return Err(ModelFailure::Backend("Model input closed".into())),
                Ok(count) => remaining = &remaining[count..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => return Err(ModelFailure::Backend(error.to_string())),
            }
        }
        let response = self.receive(deadline, token)?;
        if response["id"].as_u64() != Some(self.next_id) {
            return Err(ModelFailure::Request(
                "Obsolete/malformed model reply".into(),
            ));
        }
        if response["ok"] != true {
            let message = response["error"]
                .as_str()
                .unwrap_or("Embedding failed")
                .to_owned();
            return Err(if response["kind"] == "backend" {
                ModelFailure::Backend(message)
            } else {
                ModelFailure::Request(message)
            });
        }
        Ok(response)
    }
    fn recover(
        &mut self,
        deadline: Instant,
        token: &AtomicBool,
        reason: String,
    ) -> Result<(), String> {
        let current = BACKENDS
            .iter()
            .position(|backend| self.info["backend"] == *backend)
            .ok_or("Unknown failed inference backend")?;
        let mut failures = vec![reason];
        let mut history = self.fallback_from.clone();
        history.push(BACKENDS[current].into());
        for backend in BACKENDS.iter().skip(current + 1) {
            check_deadline(deadline, token)?;
            match Self::start(&self.paths, backend, self.role, deadline, token) {
                Ok(mut replacement) => {
                    if replacement.info["pipeline"] != self.info["pipeline"] {
                        return Err(
                            "Model pipeline changed during recovery; retry the request".into()
                        );
                    }
                    replacement.fallback_from = history;
                    *self = replacement;
                    return Ok(());
                }
                Err(error) => {
                    failures.push(format!("{backend}: {error}"));
                    history.push((*backend).into());
                }
            }
        }
        check_deadline(deadline, token)?;
        Err(format!(
            "Inference backend recovery failed: {}",
            failures.join("; ")
        ))
    }
    pub fn backend_status(&self) -> Option<String> {
        if !self.usable() {
            return None;
        }
        let mut label = self.info["device"].as_str()?.to_owned();
        if !self.fallback_from.is_empty() {
            label.push_str(&format!(
                " · fallback from {}",
                self.fallback_from.join(", ")
            ));
        }
        Some(label)
    }
    pub fn usable(&self) -> bool {
        !self.failed
    }
    fn retire(&mut self) {
        self.failed = true;
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
    pub fn passage(&mut self, text: &str, token: &AtomicBool) -> Result<(usize, usize), String> {
        let response = self.exchange(
            json!({"op":"passage", "text":text}),
            Duration::from_secs(10),
            token,
        )?;
        let end = response["end"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok());
        let next = response["next"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok());
        match (end, next) {
            (Some(end), Some(next))
                if next > 0
                    && next <= end
                    && end <= text.len()
                    && text.is_char_boundary(end)
                    && text.is_char_boundary(next) =>
            {
                Ok((end, next))
            }
            _ => Err("Invalid tokenizer passage offsets".into()),
        }
    }

    pub fn texts(
        &mut self,
        inputs: &[String],
        query: bool,
        token: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, String> {
        let response = self.exchange(
            json!({"op":"text", "inputs":inputs, "token_limit":if query {512} else {2048}}),
            Duration::from_secs(if query { 10 } else { 60 }),
            token,
        )?;
        decode(&response, inputs.len())
    }
    pub fn answer(
        &mut self,
        mut request: Value,
        timeout: Duration,
        token: &AtomicBool,
    ) -> Result<Value, String> {
        if self.role != Role::Answer {
            return Err("An answering model is required".into());
        }
        let operation = request["op"].as_str().unwrap_or("answer");
        let field = match operation {
            "answer" | "visual_answer" => "answer",
            "evidence" | "visual_evidence" => "evidence_result",
            "extraction_plan" => "plan",
            "extraction_review" => "review",
            "text_search_plan" => "search_plan",
            "text_search_review" => "search_review",
            _ => return Err("Unknown answer operation".into()),
        };
        request["op"] = json!(operation);
        let response = self.exchange(request, timeout, token)?;
        response
            .get(field)
            .cloned()
            .ok_or_else(|| "Missing structured answer".into())
    }
    pub fn media(
        &mut self,
        bytes: &[u8],
        image: bool,
        budget: u32,
        token: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Prepared media exceeds model budget".into());
        }
        let response = self.exchange(json!({"op": if image {"image"} else {"audio"},"data":STANDARD.encode(bytes),"budget":budget}), Duration::from_secs(60), token)?;
        Ok(decode(&response, 1)?.remove(0))
    }
}
fn reply_reader(output: ChildStdout) -> Result<(Replies, JoinHandle<()>), String> {
    let (sender, replies) = mpsc::sync_channel(1);
    let reader = std::thread::Builder::new()
        .name("search-model-replies".into())
        .spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let result = (&mut reader)
                    .take(256 * 1024 + 1)
                    .read_until(b'\n', &mut bytes);
                let response = match result {
                    Ok(0) => Err(ModelFailure::Backend("Model helper exited".into())),
                    Ok(count) if count > 256 * 1024 => Err(ModelFailure::Request(
                        "Model helper exceeded response budget".into(),
                    )),
                    Ok(_) => serde_json::from_slice(&bytes)
                        .map_err(|e| ModelFailure::Request(format!("Invalid model reply: {e}"))),
                    Err(error) => Err(ModelFailure::Backend(error.to_string())),
                };
                let failed = response.is_err();
                if sender.try_send(response).is_err() || failed {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((replies, reader))
}

fn check_deadline(deadline: Instant, token: &AtomicBool) -> Result<(), String> {
    if token.load(Ordering::Relaxed) {
        return Err("Search model request cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("Search model request timed out".into());
    }
    Ok(())
}
fn decode(response: &Value, count: usize) -> Result<Vec<Vec<f32>>, String> {
    if response["dimension"].as_u64() != Some(DIMENSION as u64)
        || response["count"].as_u64() != Some(count as u64)
    {
        return Err("Embedding shape mismatch".into());
    }
    let bytes = STANDARD
        .decode(response["vectors"].as_str().ok_or("Missing embeddings")?)
        .map_err(|e| e.to_string())?;
    if bytes.len() != count * DIMENSION * 4 {
        return Err("Invalid embedding payload length".into());
    }
    bytes
        .as_chunks::<{ DIMENSION * 4 }>()
        .0
        .iter()
        .map(|row| {
            let values: Vec<_> = row
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect();
            let norm = values.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>();
            if values.iter().any(|v| !v.is_finite()) || !(0.99..=1.01).contains(&norm) {
                return Err("Model produced invalid/unnormalized embeddings".into());
            }
            Ok(values)
        })
        .collect()
}
impl Drop for Model {
    fn drop(&mut self) {
        self.retire();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recovery_fixture(mode: &str) -> (tempfile::TempDir, Paths) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("mode"), mode).unwrap();
        std::fs::write(directory.path().join("worker.py"), r#"import json,os,sys,time
from pathlib import Path
root=Path(__file__).parent
mode=(root/'mode').read_text()
backend=sys.argv[sys.argv.index('--device')+1]
role=sys.argv[sys.argv.index('--role')+1]
starts=root/'starts.jsonl'
previous=[json.loads(line) for line in starts.read_text().splitlines()] if starts.exists() else []
with starts.open('a') as out:
    out.write(json.dumps({'pid':os.getpid(),'backend':backend,'previous_alive':any(Path('/proc/'+str(p['pid'])).exists() for p in previous)})+'\n')
def emit(value): print(json.dumps(value),flush=True)
if (mode=='load' and backend=='vulkan') or (mode=='cancel-load' and backend!='cpu'):
    emit({'ready':False,'error':'fixture load failure'})
    time.sleep(30)
if mode=='deadline' and backend=='vulkan': time.sleep(30)
if mode=='cancel-load' and backend=='cpu':
    (root/'cpu-start.tmp').write_text(str(os.getpid()))
    (root/'cpu-start.tmp').replace(root/'cpu-start')
    time.sleep(30)
pipeline={'provider':sys.argv[sys.argv.index('--answer-provider')+1] if role=='answer' else 'embedding','id':'changed' if mode=='change' and backend=='vulkan' else 'same'}
emit({'ready':True,'role':role,'dimension':768,'backend':backend,'device':backend,'pipeline':pipeline})
for line in sys.stdin:
    request=json.loads(line)
    with (root/'requests.jsonl').open('a') as out: out.write(json.dumps({'backend':backend,'request':request})+'\n')
    if mode=='request': emit({'id':request['id'],'ok':False,'kind':'request','error':'out of memory is literal invalid input'})
    elif backend=='cuda' or mode=='all':
        if mode=='crash': os._exit(23)
        emit({'id':request['id'],'ok':False,'kind':'backend','error':'fixture native compute failure'})
    else: emit({'id':request['id'],'ok':True,'backend':backend,'payload':request['payload']})
"#).unwrap();
        let paths = Paths {
            data: directory.path().into(),
            helpers: directory.path().into(),
        };
        (directory, paths)
    }

    fn recovery_starts(paths: &Paths) -> Vec<Value> {
        std::fs::read_to_string(paths.helpers.join("starts.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn native_failure_or_exit_reaps_before_replay_and_retains_successful_backend() {
        for (mode, expected) in [("native", "vulkan"), ("crash", "vulkan"), ("load", "cpu")] {
            for role in [Role::Embedding, Role::Answer] {
                let (_directory, paths) = recovery_fixture(mode);
                let token = AtomicBool::new(false);
                let mut model = Model::start_preferred(&paths, role, &token).unwrap();
                let request = json!({"op":"text","payload":"unchanged owned input"});
                let response = model
                    .exchange(request.clone(), Duration::from_secs(5), &token)
                    .unwrap();
                assert_eq!(response["backend"], expected);
                assert_eq!(response["payload"], request["payload"]);
                assert!(
                    model
                        .backend_status()
                        .unwrap()
                        .contains("fallback from cuda")
                );
                let starts = recovery_starts(&paths);
                assert_eq!(starts.len(), if mode == "load" { 3 } else { 2 });
                assert!(starts.iter().all(|row| row["previous_alive"] == false));
                model
                    .exchange(request.clone(), Duration::from_secs(5), &token)
                    .unwrap();
                assert_eq!(
                    recovery_starts(&paths).len(),
                    starts.len(),
                    "successful backend was not retained"
                );
                let requests =
                    std::fs::read_to_string(paths.helpers.join("requests.jsonl")).unwrap();
                for line in requests.lines() {
                    let attempt: Value = serde_json::from_str(line).unwrap();
                    assert_eq!(attempt["request"]["payload"], request["payload"]);
                }
                drop(model);
                assert!(
                    starts
                        .iter()
                        .all(|row| !Path::new(&format!("/proc/{}", row["pid"])).exists())
                );
            }
        }
    }

    #[test]
    fn inference_recovery_is_bounded_and_never_replays_validation_or_changed_pipeline() {
        for (mode, attempts) in [("request", 1), ("change", 2), ("all", 3)] {
            let (_directory, paths) = recovery_fixture(mode);
            let token = AtomicBool::new(false);
            let mut model = Model::start_preferred(&paths, Role::Embedding, &token).unwrap();
            let error = model
                .exchange(
                    json!({"op":"text","payload":"owned"}),
                    Duration::from_secs(5),
                    &token,
                )
                .unwrap_err();
            assert!(!error.is_empty());
            assert!(!model.usable());
            assert!(model.backend_status().is_none());
            let starts = recovery_starts(&paths);
            assert_eq!(starts.len(), attempts);
            assert!(
                starts
                    .iter()
                    .all(|row| !Path::new(&format!("/proc/{}", row["pid"])).exists())
            );
        }
    }

    #[test]
    fn recovery_shares_the_original_deadline_and_cancellation_prevents_replay() {
        for cancelled in [false, true] {
            let (_directory, paths) = recovery_fixture("deadline");
            let token = AtomicBool::new(false);
            let mut model = Model::start_preferred(&paths, Role::Embedding, &token).unwrap();
            token.store(cancelled, Ordering::Relaxed);
            let started = Instant::now();
            let error = model
                .exchange(
                    json!({"op":"text","payload":"owned"}),
                    Duration::from_millis(200),
                    &token,
                )
                .unwrap_err();
            assert_eq!(
                error,
                if cancelled {
                    "Search model request cancelled"
                } else {
                    "Search model request timed out"
                }
            );
            assert!(started.elapsed() < Duration::from_secs(3));
            let starts = recovery_starts(&paths);
            assert_eq!(starts.len(), if cancelled { 1 } else { 2 });
            assert!(
                starts
                    .iter()
                    .all(|row| !Path::new(&format!("/proc/{}", row["pid"])).exists())
            );
        }
    }

    #[test]
    fn last_backend_load_cancellation_retains_its_cancellation_identity() {
        let (_directory, paths) = recovery_fixture("cancel-load");
        let token = std::sync::Arc::new(AtomicBool::new(false));
        let flag = token.clone();
        let marker = paths.helpers.join("cpu-start");
        let cancel = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() {
                assert!(Instant::now() < deadline, "last backend did not start");
                std::thread::sleep(Duration::from_millis(5));
            }
            flag.store(true, Ordering::Relaxed);
        });
        let error = Model::start_preferred(&paths, Role::Embedding, &token)
            .err()
            .unwrap();
        cancel.join().unwrap();
        assert_eq!(error, "Model loading cancelled");
        let starts = recovery_starts(&paths);
        assert_eq!(starts.len(), 3);
        assert!(
            starts
                .iter()
                .all(|row| !Path::new(&format!("/proc/{}", row["pid"])).exists())
        );
    }

    #[test]
    fn answer_startup_requires_the_explicitly_selected_provider() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("worker.py"), r#"import ctypes,json,sys,time
from pathlib import Path
parent_signal = ctypes.c_int()
if sys.platform == 'linux':
    assert ctypes.CDLL(None).prctl(2, ctypes.byref(parent_signal), 0, 0, 0) == 0
requested = sys.argv[sys.argv.index('--answer-provider') + 1]
selected = Path(__file__).with_suffix('.provider').read_text()
if selected == 'requested': selected = requested
print(json.dumps({'ready': True, 'role': 'answer', 'backend': 'cpu', 'device': 'CPU', 'parent_signal': parent_signal.value, 'pipeline': {'provider': selected}}), flush=True)
time.sleep(30)
"#).unwrap();
        let paths = Paths {
            data: folder.path().into(),
            helpers: folder.path().into(),
        };
        let marker = folder.path().join("worker.provider");
        std::fs::write(&marker, "qwen3.5").unwrap();
        let error = Model::start(
            &paths,
            "cpu",
            Role::Answer,
            Instant::now() + Duration::from_secs(120),
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert!(error.contains("Incompatible"), "{error}");
        std::fs::write(&marker, "requested").unwrap();
        let model = Model::start(
            &paths,
            "cpu",
            Role::Answer,
            Instant::now() + Duration::from_secs(120),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(model.info["pipeline"]["provider"], ANSWER_PROVIDER);
        #[cfg(target_os = "linux")]
        assert_eq!(model.info["parent_signal"], libc::SIGKILL);
    }

    #[test]
    fn preferred_backend_rejects_silent_cpu_selection_and_tries_vulkan_next() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("worker.py"), r#"import json,sys,time
backend = sys.argv[sys.argv.index('--device') + 1]
with open(__file__ + '.attempts', 'a') as output: output.write(backend + '\n')
selected = 'cpu' if backend == 'cuda' else backend
print(json.dumps({'ready': True, 'dimension': 768, 'backend': selected, 'device': selected}), flush=True)
time.sleep(30)
"#).unwrap();
        let paths = Paths {
            data: folder.path().into(),
            helpers: folder.path().into(),
        };
        let model =
            Model::start_preferred(&paths, Role::Embedding, &AtomicBool::new(false)).unwrap();
        assert_eq!(model.info["backend"], "vulkan");
        assert_eq!(
            std::fs::read_to_string(folder.path().join("worker.py.attempts")).unwrap(),
            "cuda\nvulkan\n"
        );
    }
    #[test]
    fn unavailable_gpu_backends_fall_back_to_cpu_and_cancelled_loads_do_not_start() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            folder.path().join("worker.py"),
            r#"import json,sys,time
backend = sys.argv[sys.argv.index('--device') + 1]
with open(__file__ + '.attempts', 'a') as output: output.write(backend + '\n')
if backend != 'cpu':
    print(json.dumps({'ready': False, 'error': 'backend unavailable'}), flush=True)
    sys.exit(2)
print(json.dumps({'ready': True, 'dimension': 768, 'backend': 'cpu', 'device': 'CPU'}), flush=True)
time.sleep(30)
"#,
        )
        .unwrap();
        let paths = Paths {
            data: folder.path().into(),
            helpers: folder.path().into(),
        };
        let model =
            Model::start_preferred(&paths, Role::Embedding, &AtomicBool::new(false)).unwrap();
        assert_eq!(model.info["backend"], "cpu");
        let attempts = folder.path().join("worker.py.attempts");
        let expected = "cuda\nvulkan\ncpu\n";
        assert_eq!(std::fs::read_to_string(&attempts).unwrap(), expected);
        assert!(Model::start_preferred(&paths, Role::Embedding, &AtomicBool::new(true)).is_err());
        assert_eq!(std::fs::read_to_string(&attempts).unwrap(), expected);
    }

    #[test]
    fn embedding_shape_finiteness_and_norm_are_validated_at_boundary() {
        let values = vec![1.0_f32 / (DIMENSION as f32).sqrt(); DIMENSION];
        let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut response =
            json!({"dimension":DIMENSION,"count":1,"vectors":STANDARD.encode(&bytes)});
        assert_eq!(decode(&response, 1).unwrap()[0].len(), DIMENSION);
        assert!(decode(&response, 2).is_err());
        response["vectors"] = json!(STANDARD.encode(vec![0; DIMENSION * 4]));
        assert!(decode(&response, 1).is_err());
        response["vectors"] = json!(STANDARD.encode(f32::NAN.to_le_bytes().repeat(DIMENSION)));
        assert!(decode(&response, 1).is_err());
    }

    #[test]
    fn stalled_model_input_is_bounded_and_child_is_reaped_before_return() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("worker.py"),
            "import json,time\nprint(json.dumps({'ready':True,'dimension':768,'backend':'cpu','device':'CPU'}),flush=True)\ntime.sleep(30)\n").unwrap();
        let paths = Paths {
            data: folder.path().into(),
            helpers: folder.path().into(),
        };
        let token = AtomicBool::new(false);
        let mut model = Model::start(
            &paths,
            "cpu",
            Role::Embedding,
            Instant::now() + Duration::from_secs(120),
            &token,
        )
        .unwrap();
        let started = Instant::now();
        let error = model
            .exchange(
                json!({"op":"text","inputs":["x".repeat(1024 * 1024)]}),
                Duration::from_millis(40),
                &token,
            )
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(model.child.try_wait().unwrap().is_some());
        assert!(model.failed);
    }
}
