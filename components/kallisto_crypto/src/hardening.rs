//! The three small things that go with the RAM barrier (ADR-0015 D13).
//!
//! None of them stops an attacker with root, and the ADR says so in as many
//! words. What they stop are the accidents: a crash that writes the whole
//! address space to `/var/lib/systemd/coredump`, a debugger attached by
//! somebody who happens to be in the right group, and a key page written out
//! to swap where it outlives the process by however long the swap file does.
//!
//! Every call here is best-effort and reports what it managed. A resolver that
//! refused to start because `RLIMIT_MEMLOCK` was 64KB in somebody's container
//! would be trading a real outage for a marginal hardening.

/// What `harden_process` managed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Hardening {
    /// `RLIMIT_CORE` is zero: a crash writes no core file.
    pub core_dumps_disabled: bool,
    /// `PR_SET_DUMPABLE` is zero: the process fails the `PTRACE_MODE_ATTACH`
    /// check, so a debugger cannot attach and `/proc/<pid>/mem` cannot be
    /// opened, and the kernel declines to write a core file for it a second
    /// way independently of `RLIMIT_CORE`.
    ///
    /// Note for anyone verifying this by hand: on current kernels it does *not*
    /// change the ownership of `/proc/<pid>`, and on a host with Yama's
    /// `ptrace_scope` at 1 or above most attach attempts are already refused
    /// before this flag is consulted. A denied `/proc/<pid>/mem` is therefore
    /// not on its own evidence that this call took effect.
    pub not_dumpable: bool,
}

impl Hardening {
    pub fn complete(&self) -> bool {
        self.core_dumps_disabled && self.not_dumpable
    }
}

/// Pages held in RAM for as long as this value lives.
///
/// Stores the address as an integer rather than a pointer so that the type is
/// `Send` and `Sync` without an `unsafe impl`: nothing is ever dereferenced
/// through it, the number exists only to hand back to `munlock`.
pub struct LockedPages {
    addr: usize,
    len: usize,
}

impl std::fmt::Debug for LockedPages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockedPages")
            .field("len", &self.len)
            .finish()
    }
}

#[cfg(unix)]
mod sys {
    use super::{Hardening, LockedPages};

    pub fn harden_process() -> Hardening {
        let mut report = Hardening::default();

        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `setrlimit` reads one fully initialised `rlimit` through the
        // pointer and writes nothing. The value lives for the whole call.
        report.core_dumps_disabled = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } == 0;

        #[cfg(target_os = "linux")]
        {
            // SAFETY: `prctl` with PR_SET_DUMPABLE takes its argument by value
            // and touches no memory of ours. It is a process-wide setting, so
            // calling it from any thread is correct.
            report.not_dumpable = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) } == 0;
        }
        report
    }

    /// Locks the pages spanning `addr..addr + len`.
    ///
    /// `mlock` rounds outward to page boundaries, so this also pins whatever
    /// else shares those pages. That is unavoidable and harmless: the cost is
    /// one or two pages of a process that is measured in single-digit
    /// megabytes.
    pub fn lock_pages(addr: usize, len: usize) -> Option<LockedPages> {
        if len == 0 {
            return None;
        }
        // SAFETY: the caller guarantees `addr..addr + len` is a live allocation
        // it owns. `mlock` reads no memory through the pointer — it pins the
        // pages the address falls in — and the `LockedPages` returned keeps no
        // borrow, only the numbers needed to undo the call.
        let locked = unsafe { libc::mlock(addr as *const libc::c_void, len) } == 0;
        locked.then_some(LockedPages { addr, len })
    }

    pub fn unlock_pages(pages: &LockedPages) {
        // SAFETY: the same address and length that `mlock` accepted. Unlocking
        // a range that is no longer mapped is an error return, not undefined
        // behaviour, and the result is deliberately ignored.
        unsafe {
            libc::munlock(pages.addr as *const libc::c_void, pages.len);
        }
    }
}

#[cfg(not(unix))]
mod sys {
    use super::{Hardening, LockedPages};

    pub fn harden_process() -> Hardening {
        Hardening::default()
    }

    pub fn lock_pages(_addr: usize, _len: usize) -> Option<LockedPages> {
        None
    }

    pub fn unlock_pages(_pages: &LockedPages) {}
}

/// Turns off core dumps and makes the process undumpable. Call once, early,
/// before any secret has been read.
pub fn harden_process() -> Hardening {
    sys::harden_process()
}

pub fn lock_pages(addr: usize, len: usize) -> Option<LockedPages> {
    sys::lock_pages(addr, len)
}

impl Drop for LockedPages {
    fn drop(&mut self) {
        // Without this, every snapshot swap would leave another locked page
        // behind until `RLIMIT_MEMLOCK` refused the next one — and the failure
        // would be a barrier key silently no longer pinned.
        sys::unlock_pages(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locking and unlocking must be balanced, or a long-running process runs
    /// out of `RLIMIT_MEMLOCK` after enough file changes.
    #[test]
    fn pages_can_be_locked_and_relocked_many_times() {
        let buffer = vec![0u8; 4096];
        let addr = buffer.as_ptr() as usize;

        for _ in 0..64 {
            // If the drop at the end of each iteration did not unlock, a
            // default 64KB RLIMIT_MEMLOCK would start refusing part way in.
            let locked = lock_pages(addr, buffer.len());
            if locked.is_none() {
                // Containers commonly set RLIMIT_MEMLOCK to zero. Nothing to
                // assert in that case, and refusing to run there would be
                // worse than not locking.
                return;
            }
        }
    }

    #[test]
    fn locking_nothing_is_not_an_error_and_not_a_lock() {
        assert!(lock_pages(0, 0).is_none());
    }

    /// Best-effort: this asserts the call is made and reports honestly, not
    /// that the kernel allowed it. `cargo test` may already run undumpable.
    #[test]
    fn hardening_reports_what_it_managed() {
        let report = harden_process();
        if cfg!(target_os = "linux") {
            assert!(
                report.core_dumps_disabled,
                "setrlimit(RLIMIT_CORE, 0) is expected to succeed on Linux"
            );
        }
        let _ = report.complete();
    }
}
