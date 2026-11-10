//! Retained pipeline execution, isolated from async IPC and preview workers.

use little_clip_core::BackgroundJobs;
use little_clip_ops::PipelineExecutionResult;
use std::sync::LazyLock;
use std::time::Duration;

pub(super) async fn execute(
    operation: impl FnOnce() -> Result<PipelineExecutionResult, String>
    + Send
    + std::panic::UnwindSafe
    + 'static,
) -> Result<PipelineExecutionResult, String> {
    // Admission is bounded; accepted writes have no post-submission deadline.
    // Preview work and daemon IPC must remain available during a long save.
    static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 8));
    let (_, completion) = JOBS.submit(Duration::MAX, move || {
        // A failed graph must not permanently retire one of the fixed workers.
        std::panic::catch_unwind(operation)
            .unwrap_or_else(|_| Err("Pipeline worker panicked; see the daemon log".into()))
    });
    tokio::task::spawn_blocking(move || {
        completion
            .recv()
            .map_err(|error| format!("Pipeline worker disconnected: {error}"))?
            .map_err(|error| format!("Pipeline admission failed: {error}"))?
    })
    .await
    .map_err(|error| format!("Pipeline completion worker failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn panicking_pipeline_reports_failure_without_losing_the_worker() {
        assert!(
            execute(|| panic!("controlled graph panic"))
                .await
                .unwrap_err()
                .contains("panicked")
        );
        assert!(
            execute(|| Ok(PipelineExecutionResult::default()))
                .await
                .is_ok()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn accepted_pipeline_retains_late_completion_and_leaves_async_work_available() {
        let (started, ready) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let execution = tokio::spawn(execute(move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(PipelineExecutionResult {
                saved_files: vec![std::path::PathBuf::from("completed-output.txt")],
                ..Default::default()
            })
        }));
        tokio::task::spawn_blocking(move || ready.recv_timeout(Duration::from_secs(5)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tokio::spawn(async { 42 }).await.unwrap(), 42);
        tokio::time::advance(Duration::from_secs(66)).await;
        tokio::task::yield_now().await;
        assert!(
            !execution.is_finished(),
            "accepted execution must keep its reply owner"
        );
        release.send(()).unwrap();
        let result = execution.await.unwrap().unwrap();
        assert_eq!(
            result.saved_files,
            [std::path::PathBuf::from("completed-output.txt")]
        );
    }
}
