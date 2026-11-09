//! Syscall boundary for a single-threaded, byte-only extraction worker.
//! No document descriptors or paths enter this worker. Install before parsing.
use std::{io, mem::offset_of};

#[cfg(target_arch = "x86_64")]
const ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const ARCH: u32 = 0xc000_00b7;

/// Drop ambient descriptors and forbid opening files, execution and networking.
/// The caller must exit on failure, without processing its input.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(super) fn install() -> Result<(), String> {
    // Output descriptors must be anonymous transport, never a writable file or
    // terminal inherited from an unusual launcher. Only these may receive writes.
    // Stdin must be a pipe too: permitting mmap on an inherited writable file
    // there would otherwise allow shared-memory writes despite denying write().
    for fd in [0, 1, 2] {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, metadata.as_mut_ptr()) } != 0
            || unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFIFO
        {
            return Err("Extraction worker requires piped input and output".into());
        }
    }
    for (resource, value) in [
        (libc::RLIMIT_AS, 512 * 1024 * 1024),
        (libc::RLIMIT_CPU, 2),
        (libc::RLIMIT_CORE, 0),
    ] {
        let limit = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(io::Error::last_os_error().to_string());
        }
    }
    if unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 0_u32) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    let mut instructions = program();
    let filter = libc::sock_fprog {
        len: instructions.len() as u16,
        filter: instructions.as_mut_ptr(),
    };
    // Synchronize every thread even though the production entry point starts
    // none. A thread outside the filter must never retain ambient capabilities.
    let result = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_TSYNC,
            &filter,
        )
    };
    if result != 0 {
        return Err("Could not enforce the extraction syscall policy".into());
    }
    Ok(())
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) fn install() -> Result<(), String> {
    Err("Isolated extraction is unavailable on this architecture".into())
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn program() -> Vec<libc::sock_filter> {
    let stmt = |code: u32, k| libc::sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k,
    };
    let eq = |k, jt, jf| libc::sock_filter {
        code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
        jt,
        jf,
        k,
    };
    let load = libc::BPF_LD | libc::BPF_W | libc::BPF_ABS;
    let ret = libc::BPF_RET | libc::BPF_K;
    let denied = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let mut code = vec![
        stmt(load, offset_of!(libc::seccomp_data, arch) as u32),
        eq(ARCH, 1, 0),
        stmt(ret, libc::SECCOMP_RET_KILL_PROCESS),
        stmt(load, offset_of!(libc::seccomp_data, nr) as u32),
        eq(libc::SYS_write as u32, 0, 5),
        stmt(load, offset_of!(libc::seccomp_data, args) as u32),
        eq(1, 2, 0),
        eq(2, 1, 0),
        stmt(ret, denied),
        stmt(ret, libc::SECCOMP_RET_ALLOW),
    ];
    for syscall in [
        libc::SYS_read,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_lseek,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_getrandom,
        libc::SYS_clock_gettime,
        libc::SYS_clock_getres,
        libc::SYS_futex,
        libc::SYS_sched_yield,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_sigaltstack,
        libc::SYS_rt_sigreturn,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_restart_syscall,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ] {
        code.push(eq(syscall as u32, 0, 1));
        code.push(stmt(ret, libc::SECCOMP_RET_ALLOW));
    }
    // In particular: open/openat/openat2, exec, clone, sockets, ptrace, namespace
    // changes, descriptor duplication and every other write API are denied.
    code.push(stmt(ret, denied));
    code
}

#[cfg(all(test, any(target_arch = "x86_64", target_arch = "aarch64")))]
mod tests {
    use super::*;
    use std::{ffi::CString, os::fd::AsRawFd};

    const CHILD: &str = "LITTLE_CLIP_EXTRACTION_SANDBOX_TEST";

    #[test]
    fn kernel_boundary_denies_file_access_network_execution_and_inherited_writes() {
        if let Some(path) = std::env::var_os(CHILD) {
            let fd: i32 = std::env::var("LITTLE_CLIP_EXTRACTION_TEST_FD")
                .unwrap()
                .parse()
                .unwrap();
            let path = CString::new(path.as_encoded_bytes()).unwrap();
            let executable = CString::new("/bin/true").unwrap();
            let args = [executable.as_ptr(), std::ptr::null()];
            assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
            install().unwrap();
            // The inherited handle was writable; it is now closed even for
            // permitted read syscalls, and writes are independently denied.
            let mut byte = 0_u8;
            assert_eq!(
                unsafe { libc::read(fd, (&mut byte as *mut u8).cast(), 1) },
                -1
            );
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
            assert_eq!(unsafe { libc::write(fd, b"x".as_ptr().cast(), 1) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            assert_eq!(unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            assert_eq!(unsafe { libc::open(path.as_ptr(), libc::O_WRONLY) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            assert_eq!(
                unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) },
                -1
            );
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            assert_eq!(
                unsafe { libc::execve(executable.as_ptr(), args.as_ptr(), std::ptr::null()) },
                -1
            );
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            let message = b"EXTRACTION_SANDBOX_OK\n";
            assert_eq!(
                unsafe { libc::write(1, message.as_ptr().cast(), message.len()) },
                message.len() as isize
            );
            std::process::exit(0);
        }
        let fixture = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(fixture.path(), b"unchanged").unwrap();
        let fd = fixture.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) },
            0
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .stdin(std::process::Stdio::piped())
            .args(["--exact", "file_manager::search::answer::extraction::sandbox::tests::kernel_boundary_denies_file_access_network_execution_and_inherited_writes", "--nocapture", "--test-threads=1"])
            .env(CHILD, fixture.path())
            .env("LITTLE_CLIP_EXTRACTION_TEST_FD", fd.to_string())
            .output();
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) }, 0);
        let output = output.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("EXTRACTION_SANDBOX_OK"));
        assert_eq!(std::fs::read(fixture.path()).unwrap(), b"unchanged");
    }
}
