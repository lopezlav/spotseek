//! The limit of open files (and so sockets) of this process.
//!
//! A popular search brings answers from more than a thousand peers in a few seconds, each on
//! its own connection. A GUI app on macOS starts with a soft limit of 256 open files, so the
//! client raises the soft limit (up to the hard limit) and keeps its peer sockets under it.

/// Raises the soft limit to `want` (or the hard limit, if lower) and returns the limit now
/// in force. On systems without the call, returns `want`.
pub fn raise(want: u64) -> u64 {
    imp::raise(want)
}

#[cfg(unix)]
mod imp {
    #[repr(C)]
    struct RLimit {
        cur: u64,
        max: u64,
    }

    #[cfg(target_os = "linux")]
    const RLIMIT_NOFILE: i32 = 7;
    #[cfg(not(target_os = "linux"))]
    const RLIMIT_NOFILE: i32 = 8;

    unsafe extern "C" {
        fn getrlimit(resource: i32, rlim: *mut RLimit) -> i32;
        fn setrlimit(resource: i32, rlim: *const RLimit) -> i32;
    }

    pub fn raise(want: u64) -> u64 {
        let mut l = RLimit { cur: 0, max: 0 };
        // SAFETY: `l` is a valid rlimit struct (rlim_t is 64 bits on the 64-bit targets).
        if unsafe { getrlimit(RLIMIT_NOFILE, &mut l) } != 0 {
            return 256;
        }
        if l.cur >= want {
            return l.cur;
        }
        let new = RLimit { cur: want.min(l.max), max: l.max };
        // SAFETY: as above.
        if unsafe { setrlimit(RLIMIT_NOFILE, &new) } == 0 { new.cur } else { l.cur }
    }

    #[cfg(test)]
    #[test]
    fn raise_keeps_a_sane_limit() {
        let n = raise(1024);
        assert!(n >= 64, "{n}");
        assert!(raise(16) >= 16);
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn raise(want: u64) -> u64 {
        want
    }
}
