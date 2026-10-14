//! Formatting and OS helpers.

/// Raise the soft open-files limit to the hard limit. Every in-flight
/// HTTP/1.1 request holds a socket, and macOS defaults to 256.
/// Returns the resulting soft limit.
#[allow(clippy::unnecessary_cast)] // rlim_t is not u64 everywhere
pub fn raise_fd_limit() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit/setrlimit only read/write the struct we pass.
        unsafe {
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
                return None;
            }
            let want = lim.rlim_max.min(1 << 20);
            if lim.rlim_cur < want {
                let mut new = libc::rlimit {
                    rlim_cur: want,
                    rlim_max: lim.rlim_max,
                };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &new) != 0 {
                    // macOS refuses values above OPEN_MAX for some hard limits.
                    new.rlim_cur = want.min(10_240);
                    if libc::setrlimit(libc::RLIMIT_NOFILE, &new) != 0 {
                        return Some(lim.rlim_cur as u64);
                    }
                }
                return Some(new.rlim_cur as u64);
            }
            Some(lim.rlim_cur as u64)
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// 1234567 -> "1,234,567".
pub fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn fmt_rate(v: f64) -> String {
    if v > 0.0 && v < 10.0 {
        format!("{v:.1}")
    } else {
        fmt_int(v.max(0.0).round() as u64)
    }
}

/// Milliseconds with sensible precision, or "-" when unknown.
pub fn fmt_ms(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(v) if v < 10.0 => format!("{v:.1}"),
        Some(v) => format!("{v:.0}"),
    }
}

/// 83.2 -> "01:23".
pub fn fmt_clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1000), "1,000");
        assert_eq!(fmt_int(1234567), "1,234,567");
        assert_eq!(fmt_rate(2.25), "2.2");
        assert_eq!(fmt_rate(12345.6), "12,346");
        assert_eq!(fmt_ms(None), "-");
        assert_eq!(fmt_ms(Some(3.25)), "3.2");
        assert_eq!(fmt_ms(Some(31.4)), "31");
        assert_eq!(fmt_clock(83.2), "01:23");
        assert_eq!(fmt_clock(3725.0), "1:02:05");
    }
}
