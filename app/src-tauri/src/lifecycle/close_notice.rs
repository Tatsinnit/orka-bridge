use std::{
    fs::{self, OpenOptions},
    io,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, PartialEq, Eq)]
pub enum CloseAction {
    Explain,
    Pending,
    Hide,
}

/// A non-secret UI preference. Failure to save it must never prevent closing Bridge.
pub struct CloseNotice {
    marker: Option<PathBuf>,
    seen: AtomicBool,
    pending: AtomicBool,
}
impl CloseNotice {
    pub fn new(root: Option<PathBuf>) -> Self {
        let marker = root.map(|root| root.join("close-notice-seen-v1"));
        let seen = marker.as_ref().is_some_and(|path| path.is_file());
        Self {
            marker,
            seen: AtomicBool::new(seen),
            pending: AtomicBool::new(false),
        }
    }
    pub fn admit(&self) -> CloseAction {
        if self.seen.load(Ordering::Acquire) {
            CloseAction::Hide
        } else if self.pending.swap(true, Ordering::AcqRel) {
            CloseAction::Pending
        } else {
            CloseAction::Explain
        }
    }
    pub fn finish(&self, acknowledged: bool) -> io::Result<()> {
        if acknowledged {
            self.seen.store(true, Ordering::Release);
        }
        self.pending.store(false, Ordering::Release);
        if !acknowledged {
            return Ok(());
        }
        if let Some(marker) = &self.marker {
            if let Some(parent) = marker.parent() {
                fs::create_dir_all(parent)?;
            }
            match OpenOptions::new().write(true).create_new(true).open(marker) {
                Ok(file) => file.sync_all()?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
