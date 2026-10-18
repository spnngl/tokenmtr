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

/// Applies `priority` to the calling thread (`pthread_set_qos_class_self_np` on macOS; a no-op
/// elsewhere).
///
/// # Errors
/// The OS error if the `QoS` class cannot be applied (e.g. the thread is owned by a dispatch
/// queue).
pub fn set_thread_priority(priority: ThreadPriority) -> io::Result<()> {
    imp::set_qos_class(priority)
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::ThreadPriority;
    use std::io;

    pub(super) fn set_qos_class(_: ThreadPriority) -> io::Result<()> {
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
