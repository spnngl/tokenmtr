//! Per-thread priority.

use std::io;

/// Priority class of the calling thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPriority {
    /// Unattended work: efficiency cores and throttled disk I/O.
    Background,
    /// Work the user is waiting for: performance cores.
    UserInitiated,
}

/// Applies `priority` to the calling thread: a `QoS` class on macOS
/// (`pthread_set_qos_class_self_np`), a scheduling policy plus an I/O class on Linux.
///
/// # Errors
/// The OS error if the priority cannot be applied (macOS: e.g. the thread is owned by a dispatch
/// queue).
pub fn set_thread_priority(priority: ThreadPriority) -> io::Result<()> {
    imp::set_qos_class(priority)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::ThreadPriority;
    use std::io;

    pub(super) fn set_qos_class(_: ThreadPriority) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::ThreadPriority;
    use std::io;
    use std::os::raw::{c_int, c_long};

    /// `IOPRIO_WHO_PROCESS` from `<linux/ioprio.h>`; with a `who` of 0 it names the calling
    /// thread.
    pub(super) const IOPRIO_WHO_PROCESS: c_long = 1;
    /// `IOPRIO_CLASS_NONE`: the I/O class follows the thread's nice value.
    pub(super) const IOPRIO_CLASS_NONE: c_long = 0;
    /// `IOPRIO_CLASS_IDLE`: disk time only when nobody else wants it.
    pub(super) const IOPRIO_CLASS_IDLE: c_long = 3;
    /// The class sits above this many bits of the priority value.
    pub(super) const IOPRIO_CLASS_SHIFT: c_long = 13;

    /// `SCHED_BATCH` rather than `SCHED_IDLE`: the kernel lets a thread leave `SCHED_IDLE` only
    /// with `CAP_SYS_NICE` or a raised `RLIMIT_NICE`, so `UserInitiated` could not undo it. The
    /// I/O class only takes effect under the BFQ disk scheduler; the trait allows best effort.
    #[expect(
        unsafe_code,
        reason = "pthread_setschedparam and ioprio_set have no safe binding"
    )]
    pub(super) fn set_qos_class(priority: ThreadPriority) -> io::Result<()> {
        let (policy, io_class): (c_int, c_long) = match priority {
            ThreadPriority::Background => (libc::SCHED_BATCH, IOPRIO_CLASS_IDLE),
            ThreadPriority::UserInitiated => (libc::SCHED_OTHER, IOPRIO_CLASS_NONE),
        };
        let param = libc::sched_param { sched_priority: 0 };
        // SAFETY: `pthread_self()` is the calling thread's valid handle and `param` is a live
        // local.
        let rc =
            unsafe { libc::pthread_setschedparam(libc::pthread_self(), policy, &raw const param) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        // SAFETY: `ioprio_set` takes three integers and touches no memory.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_ioprio_set,
                IOPRIO_WHO_PROCESS,
                0 as c_long,
                io_class << IOPRIO_CLASS_SHIFT,
            )
        };
        if rc == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::ThreadPriority;
    use std::io;
    use std::os::raw::{c_int, c_uint};

    /// `QOS_CLASS_BACKGROUND` from `<sys/qos.h>`.
    pub(super) const QOS_CLASS_BACKGROUND: c_uint = 0x09;
    /// `QOS_CLASS_USER_INITIATED` from `<sys/qos.h>`.
    pub(super) const QOS_CLASS_USER_INITIATED: c_uint = 0x19;

    // `libc` has no bindings for these two functions.
    #[expect(unsafe_code, reason = "FFI declarations for libSystem QoS functions")]
    unsafe extern "C" {
        /// Returns 0 on success, else an errno value. Arguments are plain integers.
        safe fn pthread_set_qos_class_self_np(qos_class: c_uint, relative_priority: c_int)
        -> c_int;
        /// Writes through both out-pointers, which must be valid; hence `unsafe` to call.
        #[cfg(test)]
        pub(super) fn pthread_get_qos_class_np(
            thread: libc::pthread_t,
            qos_class: *mut c_uint,
            relative_priority: *mut c_int,
        ) -> c_int;
    }

    pub(super) fn set_qos_class(priority: ThreadPriority) -> io::Result<()> {
        let class = match priority {
            ThreadPriority::Background => QOS_CLASS_BACKGROUND,
            ThreadPriority::UserInitiated => QOS_CLASS_USER_INITIATED,
        };
        match pthread_set_qos_class_self_np(class, 0) {
            0 => Ok(()),
            errno => Err(io::Error::from_raw_os_error(errno)),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::imp::{
        IOPRIO_CLASS_IDLE, IOPRIO_CLASS_NONE, IOPRIO_CLASS_SHIFT, IOPRIO_WHO_PROCESS,
    };
    use super::*;
    use std::os::raw::c_long;

    /// `(scheduling policy, I/O class)` of the calling thread.
    #[expect(
        unsafe_code,
        reason = "syscall readback of the scheduling state in a test"
    )]
    fn current() -> (i32, c_long) {
        // SAFETY: `sched_getscheduler(0)` takes no pointer and `ioprio_get` takes two integers.
        let (policy, ioprio) = unsafe {
            (
                libc::sched_getscheduler(0),
                libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0 as c_long),
            )
        };
        assert!(policy >= 0 && ioprio >= 0);
        (policy, ioprio >> IOPRIO_CLASS_SHIFT)
    }

    #[test]
    fn scheduling_is_applied_to_the_calling_thread_and_reversible() {
        std::thread::spawn(|| {
            set_thread_priority(ThreadPriority::Background).unwrap();
            assert_eq!(current(), (libc::SCHED_BATCH, IOPRIO_CLASS_IDLE));
            set_thread_priority(ThreadPriority::UserInitiated).unwrap();
            assert_eq!(current(), (libc::SCHED_OTHER, IOPRIO_CLASS_NONE));
        })
        .join()
        .unwrap();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::imp::{QOS_CLASS_BACKGROUND, QOS_CLASS_USER_INITIATED, pthread_get_qos_class_np};
    use super::*;

    #[expect(unsafe_code, reason = "FFI readback of the QoS class in a test")]
    fn current_qos_class() -> u32 {
        let (mut class, mut relative) = (0, 0);
        // SAFETY: `pthread_self()` is the calling thread's valid handle and both out-pointers
        // reference live locals.
        let rc = unsafe {
            pthread_get_qos_class_np(libc::pthread_self(), &raw mut class, &raw mut relative)
        };
        assert_eq!(rc, 0);
        class
    }

    #[test]
    fn qos_class_is_applied_to_the_calling_thread() {
        std::thread::spawn(|| {
            set_thread_priority(ThreadPriority::Background).unwrap();
            assert_eq!(current_qos_class(), QOS_CLASS_BACKGROUND);
            set_thread_priority(ThreadPriority::UserInitiated).unwrap();
            assert_eq!(current_qos_class(), QOS_CLASS_USER_INITIATED);
        })
        .join()
        .unwrap();
    }
}
