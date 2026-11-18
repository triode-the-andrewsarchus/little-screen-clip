//! Owned subprocess lifetime and suspension for disposable integration fixtures.
use std::process::Child;

pub(crate) struct Owned(pub Child);
impl Owned {
    pub fn wait(&mut self, timeout: std::time::Duration) -> std::process::ExitStatus {
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(
                started.elapsed() < timeout,
                "Owned fixture process did not exit within {timeout:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn suspended(child: &mut Child, test: impl FnOnce()) {
    struct Resume<'a>(&'a mut Child);
    impl Drop for Resume<'_> {
        fn drop(&mut self) {
            // SAFETY: the borrowed child remains unreaped for this scope.
            unsafe { libc::kill(self.0.id() as i32, libc::SIGCONT) };
        }
    }
    assert!(
        child.try_wait().unwrap().is_none(),
        "Cannot suspend an exited fixture child"
    );
    let resume = Resume(child);
    let pid = resume.0.id() as i32;
    // SAFETY: this is our live, unreaped fixture child. No other owner waits.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
    let mut status = 0;
    // SAFETY: status is writable; WUNTRACED observes the owned child's stop.
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) },
        pid
    );
    assert!(libc::WIFSTOPPED(status));
    assert_eq!(libc::WSTOPSIG(status), libc::SIGSTOP);
    test();
    drop(resume);
}
