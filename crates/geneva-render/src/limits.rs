//! How much of the machine one geneva process takes: one thread count
//! and one memory budget, set once at start (`--threads`,
//! `--memory-budget`) and read by every pool and cache, so several jobs
//! can share a host each within its share.
//!
//! Unset, the thread count is what the operating system says this
//! process may use, which on Linux follows a cgroup CPU quota, and the
//! caches keep their own fixed budgets.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static THREADS: AtomicUsize = AtomicUsize::new(0);
static MEMORY: AtomicU64 = AtomicU64::new(0);

/// Caps every pool at `count` threads (at least one).
pub fn set_threads(count: usize) {
    THREADS.store(count.max(1), Ordering::Relaxed);
}

/// The thread count set with [`set_threads`], if one was.
#[must_use]
pub fn threads_set() -> Option<usize> {
    match THREADS.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n),
    }
}

/// Threads a pool may use: the count set, or the machine's.
#[must_use]
pub fn threads() -> usize {
    threads_set().unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from))
}

/// Sets the memory budget in bytes, which the caches size themselves
/// from. It is a target for what geneva keeps, not a hard limit on the
/// process: decoders, encoders and frames in flight come on top.
pub fn set_memory_budget(bytes: u64) {
    MEMORY.store(bytes, Ordering::Relaxed);
}

/// The memory budget set, if one was.
#[must_use]
pub fn memory_budget() -> Option<u64> {
    match MEMORY.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n),
    }
}

/// A cache's budget in bytes: `default`, or a `share` of the memory
/// budget when that is smaller.
#[must_use]
pub fn cache_budget(default: usize, share: f64) -> usize {
    memory_budget().map_or(default, |b| {
        default.min(((b as f64) * share) as usize).max(1 << 20)
    })
}

/// The most memory this process has held at once, in bytes, where the
/// operating system says: Linux and macOS.
#[must_use]
pub fn peak_memory() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Parses a size such as `3G`, `512M`, `1.5GB` or a number of bytes.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let upper = t.to_ascii_uppercase();
    let digits = upper.trim_end_matches('B');
    let (number, unit) = match digits.chars().last() {
        Some('K') => (&digits[..digits.len() - 1], 1u64 << 10),
        Some('M') => (&digits[..digits.len() - 1], 1 << 20),
        Some('G') => (&digits[..digits.len() - 1], 1 << 30),
        Some('T') => (&digits[..digits.len() - 1], 1 << 40),
        _ => (digits, 1),
    };
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("{text:?} is not a size; use 3G, 512M or a number of bytes"))?;
    if value <= 0.0 || !value.is_finite() {
        return Err(format!("{text:?} is not a size above zero"));
    }
    Ok((value * unit as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn sizes_read_as_people_write_them() {
        assert_eq!(parse_size("3G"), Ok(3 << 30));
        assert_eq!(parse_size("512m"), Ok(512 << 20));
        assert_eq!(parse_size("1.5GB"), Ok(3 << 29));
        assert_eq!(parse_size("1000"), Ok(1000));
        assert!(parse_size("lots").is_err());
        assert!(parse_size("0").is_err());
    }
}
