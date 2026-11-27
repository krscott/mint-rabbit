//! Noticing when another program has changed a file, such as Blender
//! exporting over a project's model file.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

/// How often a watched file is checked.
pub const POLL: Duration = Duration::from_millis(250);

/// How long a changed file must stay unchanged before it is read, so a file
/// another program is still writing isn't read half-written.
const SETTLE: Duration = Duration::from_millis(500);

/// One version of a file, as far as its metadata tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    modified: SystemTime,
    len: u64,
}

/// The stamp of the file at `path` now, or `None` if it can't be read, as
/// when it doesn't exist.
pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok()?,
        len: meta.len(),
    })
}

/// Follows one file, telling when a new version is ready to read.
#[derive(Debug)]
pub struct Watch {
    /// The version last read or written here.
    known: Option<Stamp>,
    /// A newer version, and when it was first seen.
    changed: Option<(Stamp, Instant)>,
}

impl Watch {
    /// A watch of a file whose version `known` needs no reading.
    pub fn new(known: Option<Stamp>) -> Self {
        Self {
            known,
            changed: None,
        }
    }

    /// Records that the file was read or written here as version `known`.
    pub fn known(&mut self, known: Option<Stamp>) {
        *self = Self::new(known);
    }

    /// Whether to read the file, given its version `now` at time `at`: it
    /// differs from the version known here and has stayed the same for a
    /// while. A file that has gone missing waits until it comes back.
    pub fn poll(&mut self, now: Option<Stamp>, at: Instant) -> bool {
        let Some(now) = now.filter(|&s| Some(s) != self.known) else {
            self.changed = None;
            return false;
        };
        match self.changed {
            Some((seen, since)) if seen == now => {
                let ready = at.duration_since(since) >= SETTLE;
                if ready {
                    self.known(Some(now));
                }
                ready
            }
            _ => {
                self.changed = Some((now, at));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(len: u64) -> Stamp {
        Stamp {
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(len),
            len,
        }
    }

    #[test]
    fn a_change_is_read_once_it_settles() {
        let t0 = Instant::now();
        let mut w = Watch::new(Some(version(1)));
        assert!(!w.poll(Some(version(1)), t0));
        // Still being written: each new version restarts the wait.
        assert!(!w.poll(Some(version(2)), t0));
        assert!(!w.poll(Some(version(3)), t0 + SETTLE));
        assert!(!w.poll(Some(version(3)), t0 + SETTLE + POLL));
        assert!(w.poll(Some(version(3)), t0 + SETTLE * 2));
        // Read once.
        assert!(!w.poll(Some(version(3)), t0 + SETTLE * 4));
    }

    #[test]
    fn a_missing_file_waits_for_its_return() {
        let t0 = Instant::now();
        let mut w = Watch::new(Some(version(1)));
        assert!(!w.poll(None, t0));
        assert!(!w.poll(None, t0 + SETTLE * 2));
        assert!(!w.poll(Some(version(1)), t0 + SETTLE * 3));
        assert!(!w.poll(Some(version(2)), t0 + SETTLE * 4));
        assert!(w.poll(Some(version(2)), t0 + SETTLE * 5));
    }

    #[test]
    fn a_version_written_here_is_not_read() {
        let t0 = Instant::now();
        let mut w = Watch::new(Some(version(1)));
        assert!(!w.poll(Some(version(2)), t0));
        w.known(Some(version(2)));
        assert!(!w.poll(Some(version(2)), t0 + SETTLE * 2));
    }
}
