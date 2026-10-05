//! Durable relay setup and recovery journal API.
//!
//! Call `prepare` with a DPAPI-protected route snapshot before any tunnel or
//! route operation. On restart, restore every entry returned by `pending`;
//! delete an entry only after the adapter confirms the original route state.

use rusqlite::Connection;

pub use crate::storage::{RelayJournalEntry, RelayJournalPhase};

pub struct RestoreJournal<'a> {
    database: &'a Connection,
}

impl<'a> RestoreJournal<'a> {
    pub fn new(database: &'a Connection) -> Self {
        Self { database }
    }

    pub fn prepare(&self, session_id: &str, protected_snapshot: &[u8]) -> Result<(), String> {
        crate::storage::begin_relay_session(self.database, session_id, protected_snapshot)
    }

    pub fn tunnel_ready(&self, session_id: &str) -> Result<(), String> {
        self.advance(session_id, RelayJournalPhase::TunnelReady)
    }

    pub fn routes_active(&self, session_id: &str) -> Result<(), String> {
        self.advance(session_id, RelayJournalPhase::RoutesActive)
    }

    pub fn begin_restore(&self, session_id: &str) -> Result<(), String> {
        self.advance(session_id, RelayJournalPhase::Restoring)
    }

    pub fn restore_failed(&self, session_id: &str) -> Result<(), String> {
        crate::storage::record_relay_restore_failure(self.database, session_id)
    }

    pub fn pending(&self) -> Result<Vec<RelayJournalEntry>, String> {
        crate::storage::pending_relay_recovery(self.database)
    }

    pub fn restore_confirmed(&self, session_id: &str) -> Result<(), String> {
        crate::storage::clear_relay_session(self.database, session_id)
    }

    fn advance(&self, session_id: &str, phase: RelayJournalPhase) -> Result<(), String> {
        crate::storage::advance_relay_session(self.database, session_id, phase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn lifecycle_retains_restore_work_until_explicit_confirmation() {
        let database = crate::storage::open(Path::new(":memory:")).unwrap();
        let journal = RestoreJournal::new(&database);
        let session = "lifecycle-0123456789";
        journal
            .prepare(session, b"encrypted-snapshot-fixture")
            .unwrap();
        journal.tunnel_ready(session).unwrap();
        journal.routes_active(session).unwrap();
        journal.begin_restore(session).unwrap();
        journal.restore_failed(session).unwrap();

        let pending = journal.pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].session_id, session);
        assert_eq!(pending[0].phase, RelayJournalPhase::Restoring);
        assert_eq!(
            pending[0].last_error.as_deref(),
            Some("Route restoration did not complete; retry is required.")
        );

        journal.restore_confirmed(session).unwrap();
        assert!(journal.pending().unwrap().is_empty());
    }
}
