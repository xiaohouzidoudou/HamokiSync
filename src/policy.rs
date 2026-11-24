/// Transfer coordination is isolated from clipboard and network adapters.
/// No new data is read or sent until the user has both acknowledged privacy
/// and requested Start, and a fresh remote baseline has completed.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub kind: &'static str,
    pub hash: String,
}

impl Identity {
    pub fn new(kind: &'static str, hash: impl Into<String>) -> Self {
        Self {
            kind,
            hash: hash.into().to_ascii_uppercase(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Ignore,
    Publish,
    Fetch,
    NeedsReconciliation,
}

#[derive(Debug, Default)]
pub struct SyncPolicy {
    started: bool,
    has_baseline: bool,
    uncertain: bool,
    last_remote: Option<Identity>,
    local_revision: u64,
    local_pending: Option<Identity>,
    applying_remote: bool,
}

impl SyncPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&mut self, privacy_acknowledged: bool) -> bool {
        if !privacy_acknowledged {
            return false;
        }
        *self = Self {
            started: true,
            ..Self::default()
        };
        true
    }

    pub fn stop(&mut self) {
        *self = Self::default();
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn has_baseline(&self) -> bool {
        self.has_baseline
    }

    pub fn revision(&self) -> u64 {
        self.local_revision
    }

    /// Called only with a remote GET read while syncing; first successful GET
    /// is the baseline and MUST NOT overwrite local clipboard on startup.
    pub fn see_remote(&mut self, identity: Identity, revision_at_request: u64) -> Decision {
        if !self.started || self.uncertain {
            return Decision::Ignore;
        }
        if !self.has_baseline {
            self.has_baseline = true;
            self.last_remote = Some(identity);
            return Decision::Ignore;
        }
        if self.last_remote.as_ref() == Some(&identity) {
            return Decision::Ignore;
        }
        if self.local_revision != revision_at_request
            || self.local_pending.is_some()
            || self.applying_remote
        {
            return Decision::Ignore;
        }
        Decision::Fetch
    }

    /// Caller should invoke for a real new local copy (never for startup
    /// clipboard content). Files and unsupported MIME types use None.
    pub fn copied(&mut self, identity: Option<Identity>) -> Decision {
        self.local_revision = self.local_revision.wrapping_add(1);
        if !self.started || self.applying_remote {
            return Decision::Ignore;
        }
        // Changing an unsupported MIME must cancel an older remote download.
        if !self.has_baseline || self.uncertain {
            self.local_pending = None;
            return Decision::Ignore;
        }
        self.local_pending = identity;
        if self.local_pending.is_some() && self.local_pending == self.last_remote {
            // Re-copying a value that already exists remotely is not pending
            // work. Leaving it queued would block subsequent remote changes.
            self.local_pending = None;
            Decision::Ignore
        } else if self.local_pending.is_some() {
            Decision::Publish
        } else {
            Decision::Ignore
        }
    }

    pub fn pending(&self) -> Option<&Identity> {
        self.local_pending.as_ref()
    }

    pub fn applied_remote(&mut self, identity: Identity, revision: u64) -> bool {
        if !self.can_apply_remote(revision) {
            return false;
        }
        self.applying_remote = true;
        self.local_revision = self.local_revision.wrapping_add(1);
        self.last_remote = Some(identity);
        self.applying_remote = false;
        true
    }

    /// Check again immediately before changing the actual clipboard. Call
    /// applied_remote only AFTER the OS accepted the write.
    pub fn can_apply_remote(&self, revision: u64) -> bool {
        self.started
            && !self.uncertain
            && self.has_baseline
            && self.local_revision == revision
            && self.local_pending.is_none()
    }

    /// A transport failure after sending PUT is uncertain: never automatically
    /// resend data, which might overwrite another device's latest clipboard.
    pub fn finish_publish(&mut self, identity: &Identity, accepted: bool) -> Decision {
        if !self.started {
            return Decision::Ignore;
        }
        if !accepted {
            self.uncertain = true;
            return Decision::NeedsReconciliation;
        }
        self.last_remote = Some(identity.clone());
        if self.local_pending.as_ref() == Some(identity) {
            self.local_pending = None;
        }
        Decision::Ignore
    }

    pub fn uncertain(&self) -> bool {
        self.uncertain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(hash: &str) -> Identity {
        Identity::new("Text", hash)
    }

    #[test]
    fn consent_and_baseline_prevent_old_uploads() {
        let mut s = SyncPolicy::new();
        assert!(!s.start(false));
        assert_eq!(s.copied(Some(id("local"))), Decision::Ignore);
        assert!(!s.is_started());
        assert!(s.start(true));
        assert_eq!(s.copied(Some(id("old"))), Decision::Ignore);
        assert_eq!(s.see_remote(id("srv"), s.revision()), Decision::Ignore);
        assert_eq!(s.see_remote(id("srv2"), s.revision()), Decision::Fetch);
        assert_eq!(s.copied(Some(id("new"))), Decision::Publish);
    }

    #[test]
    fn remote_fails_if_local_clipboard_changes_after_get() {
        let mut s = SyncPolicy::new();
        s.start(true);
        s.see_remote(id("a"), 0);
        let before = s.revision();
        s.copied(None);
        assert_eq!(s.see_remote(id("b"), before), Decision::Ignore);
        assert!(!s.applied_remote(id("b"), before));
    }

    #[test]
    fn failed_put_requires_manual_reconciliation() {
        let mut s = SyncPolicy::new();
        s.start(true);
        s.see_remote(id("a"), 0);
        s.copied(Some(id("b")));
        assert_eq!(
            s.finish_publish(&id("b"), false),
            Decision::NeedsReconciliation
        );
        assert!(s.uncertain());
        assert_eq!(s.copied(Some(id("c"))), Decision::Ignore);
        s.stop();
        assert!(!s.uncertain());
    }

    #[test]
    fn recopying_remote_baseline_does_not_block_later_remote_change() {
        let mut s = SyncPolicy::new();
        s.start(true);
        assert_eq!(s.see_remote(id("already"), 0), Decision::Ignore);
        assert_eq!(s.copied(Some(id("already"))), Decision::Ignore);
        assert!(s.pending().is_none());
        let revision = s.revision();
        assert_eq!(s.see_remote(id("different"), revision), Decision::Fetch);
    }
}
