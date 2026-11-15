//! Saved recipe discovery and explicitly accepted daemon execution.
use little_clip_core::{
    BackgroundJobs, JobError,
    ipc::{IpcRequest, IpcResponse},
    jobs::task::Task,
};
use std::time::Duration;

type ResultTask = Task<Result<IpcResponse, String>>;
#[derive(Default)]
pub(in crate::file_manager) struct Recipes {
    pub names: Vec<String>,
    pub error: Option<String>,
    loading: Option<ResultTask>,
    loaded: bool,
    queued: Option<IpcRequest>,
    execution: Option<ResultTask>,
    pub failure: Option<String>,
}
fn send(request: &IpcRequest, timeout: Option<Duration>) -> Result<IpcResponse, String> {
    crate::desktop_ipc::request(request, timeout, 4 * 1024 * 1024, 64 * 1024)
        .map_err(|e| e.to_string())
}
impl Recipes {
    pub fn load(&mut self, refresh: bool) {
        if self.loading.is_some() || (self.loaded && !refresh) {
            return;
        }
        self.error = None;
        self.loading = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(5),
            |_| send(&IpcRequest::ListRecipes, Some(Duration::from_secs(3))),
        ));
    }
    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }
    pub fn writing(&self) -> bool {
        self.queued.is_some() || self.execution.is_some()
    }
    pub fn run(&mut self, name: String, prompt: Option<String>) {
        if self.writing() {
            return;
        }
        self.queued = Some(IpcRequest::ExecuteRecipeWithPrompt {
            recipe_name: name,
            prompt_text: prompt,
        });
        self.failure = None;
    }
    pub fn retire_reads(&mut self) {
        self.loading = None;
    }
    pub fn tick(&mut self) {
        self.poll_names();
        self.tick_run_with(crate::desktop_ipc::mutation_jobs(), |request| {
            send(&request, None)
        });
    }
    fn poll_names(&mut self) {
        if let Some(result) = self.loading.as_ref().and_then(Task::poll) {
            self.loading = None;
            self.loaded = true;
            match result {
                Ok(Ok(IpcResponse::RecipeList(mut names))) => {
                    names.retain(|name| !name.trim().is_empty() && name.len() <= 256);
                    names.sort();
                    names.dedup();
                    names.truncate(256);
                    self.names = names;
                }
                Ok(Ok(IpcResponse::Error { error })) => self.error = Some(error),
                Ok(Ok(_)) => self.error = Some("Unexpected recipe-list response".into()),
                Ok(Err(error)) => self.error = Some(error),
                Err(JobError::QueueFull) => self.loaded = false,
                Err(error) => self.error = Some(error.to_string()),
            }
            self.error = self
                .error
                .take()
                .map(|error| error.chars().take(2048).collect());
        }
    }
    fn tick_run_with(
        &mut self,
        jobs: &BackgroundJobs,
        transport: impl FnOnce(IpcRequest) -> Result<IpcResponse, String> + Send + 'static,
    ) {
        if let Some(result) = self.execution.as_ref().and_then(Task::poll) {
            self.execution = None;
            if !matches!(result, Err(JobError::QueueFull)) {
                self.queued = None;
                self.failure = match result {
                    Ok(Ok(IpcResponse::PipelineExecuted { .. } | IpcResponse::Success { .. })) => {
                        None
                    }
                    Ok(Ok(IpcResponse::Error { error })) => Some(error),
                    Ok(Ok(IpcResponse::NeedsPrompt { message, .. })) => Some(format!(
                        "Edit this region's optional prompt before running again: {message}"
                    )),
                    Ok(Ok(_)) => {
                        Some("Unexpected recipe execution response; outcome is unknown".into())
                    }
                    Ok(Err(error)) => {
                        Some(format!("Recipe reply failed; it may have run: {error}"))
                    }
                    Err(error) => Some(format!(
                        "Recipe delivery failed; outcome is unknown: {error}"
                    )),
                }
                .map(|error: String| error.chars().take(2048).collect());
            }
        }
        if self.execution.is_none()
            && let Some(request) = self.queued.clone()
        {
            // Admission alone may retry. After submission there is no response
            // deadline/cancellation: the daemon may already have applied effects.
            self.execution = Some(Task::start_unbounded(jobs, move |_| transport(request)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    fn finish(recipes: &mut Recipes, pool: &BackgroundJobs) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while recipes.writing() {
            recipes.tick_run_with(pool, |_| {
                panic!("An accepted request must never be retried")
            });
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn accepted_recipe_survives_hidden_read_retirement_and_completes_once() {
        let pool = BackgroundJobs::new(1, 1);
        let (sent_tx, sent_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut recipes = Recipes::default();
        recipes.run("Normalize Audio".into(), Some("softly".into()));
        recipes.tick_run_with(&pool, move |request| {
            sent_tx.send(request).unwrap();
            let _ = release_rx.recv();
            Ok(IpcResponse::Success {
                message: "Done".into(),
            })
        });
        assert!(
            matches!(sent_rx.recv_timeout(Duration::from_secs(3)).unwrap(),IpcRequest::ExecuteRecipeWithPrompt {recipe_name,prompt_text:Some(prompt)} if recipe_name=="Normalize Audio" && prompt=="softly")
        );
        recipes.retire_reads();
        assert!(recipes.writing());
        recipes.tick_run_with(&pool, |_| panic!("Duplicate accepted execution"));
        assert!(recipes.writing());
        release_tx.send(()).unwrap();
        finish(&mut recipes, &pool);
        assert!(recipes.failure.is_none());
    }
    #[test]
    fn only_queue_admission_retries_and_executes_effects_once() {
        let pool = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _running = Task::start_unbounded(&pool, move |_| {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let _queued = Task::start_unbounded(&pool, |_| 42);
        let count = Arc::new(AtomicUsize::new(0));
        let mut recipes = Recipes::default();
        recipes.run("Example".into(), None);
        for _ in 0..3 {
            let count = count.clone();
            recipes.tick_run_with(&pool, move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(IpcResponse::Success {
                    message: String::new(),
                })
            });
        }
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(recipes.writing());
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while recipes.writing() {
            let count = count.clone();
            recipes.tick_run_with(&pool, move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(IpcResponse::Success {
                    message: String::new(),
                })
            });
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(recipes.failure.is_none());
    }
    #[test]
    fn a_lost_reply_is_reported_without_resending_the_recipe() {
        let pool = BackgroundJobs::new(1, 1);
        let mut recipes = Recipes::default();
        recipes.run("Example".into(), None);
        recipes.tick_run_with(&pool, |_| Err("Reply connection lost".into()));
        finish(&mut recipes, &pool);
        assert!(recipes.failure.as_deref().unwrap().contains("may have run"));
    }
}
