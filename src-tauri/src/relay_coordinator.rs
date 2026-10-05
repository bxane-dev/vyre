//! Coordinates relay setup and rollback through an injectable backend.
//!
//! The crate intentionally provides no Windows route backend yet. A future
//! backend must make `restore_direct` idempotent and restrict cleanup to VYRE
//! owned state. The coordinator persists recovery intent before invoking it.

use crate::{relay_lifecycle::RestoreJournal, relay_tunnel::TunnelConfig};
use rusqlite::Connection;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub trait TunnelRouteBackend {
    fn start_tunnel(&mut self, session_id: &str, config: &str) -> Result<(), ()>;
    fn activate_scoped_routes(&mut self, session_id: &str) -> Result<(), ()>;
    fn restore_direct(&mut self, session_id: &str, protected_snapshot: &[u8]) -> Result<(), ()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatorError {
    Journal,
    TunnelStart,
    RouteActivation,
    RestorePending,
    UnknownSession,
}

impl std::fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Journal => "Relay recovery journal operation failed.",
            Self::TunnelStart => "Relay tunnel could not be started.",
            Self::RouteActivation => "Scoped relay routes could not be activated.",
            Self::RestorePending => "Relay cleanup is pending; Direct will be retried.",
            Self::UnknownSession => "Relay session is not active in this process.",
        })
    }
}

impl std::error::Error for CoordinatorError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub restored: usize,
    pub pending: usize,
}

pub struct RelayCoordinator<'a, B: TunnelRouteBackend> {
    journal: RestoreJournal<'a>,
    backend: B,
    heartbeat_timeout: Duration,
    heartbeats: HashMap<String, Instant>,
}

impl<'a, B: TunnelRouteBackend> RelayCoordinator<'a, B> {
    pub fn new(database: &'a Connection, backend: B, heartbeat_timeout: Duration) -> Self {
        Self {
            journal: RestoreJournal::new(database),
            backend,
            heartbeat_timeout,
            heartbeats: HashMap::new(),
        }
    }

    pub fn start(
        &mut self,
        session_id: &str,
        protected_snapshot: &[u8],
        config: &TunnelConfig,
    ) -> Result<(), CoordinatorError> {
        self.journal
            .prepare(session_id, protected_snapshot)
            .map_err(|_| CoordinatorError::Journal)?;

        if self
            .backend
            .start_tunnel(session_id, config.as_str())
            .is_err()
        {
            self.rollback(session_id, protected_snapshot)?;
            return Err(CoordinatorError::TunnelStart);
        }
        if self.journal.tunnel_ready(session_id).is_err() {
            self.rollback(session_id, protected_snapshot)?;
            return Err(CoordinatorError::Journal);
        }

        if self.backend.activate_scoped_routes(session_id).is_err() {
            self.rollback(session_id, protected_snapshot)?;
            return Err(CoordinatorError::RouteActivation);
        }
        if self.journal.routes_active(session_id).is_err() {
            self.rollback(session_id, protected_snapshot)?;
            return Err(CoordinatorError::Journal);
        }
        self.heartbeats
            .insert(session_id.to_string(), Instant::now());
        Ok(())
    }

    pub fn heartbeat(&mut self, session_id: &str) -> Result<(), CoordinatorError> {
        let last_seen = self
            .heartbeats
            .get_mut(session_id)
            .ok_or(CoordinatorError::UnknownSession)?;
        *last_seen = Instant::now();
        Ok(())
    }

    pub fn stop(&mut self, session_id: &str) -> Result<(), CoordinatorError> {
        let entry = self
            .journal
            .pending()
            .map_err(|_| CoordinatorError::Journal)?
            .into_iter()
            .find(|entry| entry.session_id == session_id)
            .ok_or(CoordinatorError::UnknownSession)?;
        self.rollback(session_id, &entry.protected_restore_blob)?;
        self.heartbeats.remove(session_id);
        Ok(())
    }

    /// Retries cleanup for all sessions left by a previous crash. The backend
    /// must treat restore as idempotent because a crash may have occurred after
    /// restoring Windows state but before deleting this journal row.
    pub fn recover_pending(&mut self) -> Result<RecoveryReport, CoordinatorError> {
        let entries = self
            .journal
            .pending()
            .map_err(|_| CoordinatorError::Journal)?;
        let mut report = RecoveryReport::default();
        for entry in entries {
            match self.rollback(&entry.session_id, &entry.protected_restore_blob) {
                Ok(()) => report.restored += 1,
                Err(_) => report.pending += 1,
            }
        }
        Ok(report)
    }

    /// Restore sessions whose service heartbeat exceeded the configured limit.
    pub fn watchdog_check(&mut self, now: Instant) -> RecoveryReport {
        let expired = self
            .heartbeats
            .iter()
            .filter(|(_, last_seen)| {
                now.checked_duration_since(**last_seen).unwrap_or_default()
                    >= self.heartbeat_timeout
            })
            .map(|(session_id, _)| session_id.clone())
            .collect::<Vec<_>>();
        let mut report = RecoveryReport::default();
        for session_id in expired {
            match self.stop(&session_id) {
                Ok(()) => report.restored += 1,
                Err(_) => report.pending += 1,
            }
        }
        report
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    fn rollback(
        &mut self,
        session_id: &str,
        protected_snapshot: &[u8],
    ) -> Result<(), CoordinatorError> {
        self.journal
            .begin_restore(session_id)
            .map_err(|_| CoordinatorError::Journal)?;
        if self
            .backend
            .restore_direct(session_id, protected_snapshot)
            .is_err()
        {
            self.journal
                .restore_failed(session_id)
                .map_err(|_| CoordinatorError::Journal)?;
            return Err(CoordinatorError::RestorePending);
        }
        self.journal
            .restore_confirmed(session_id)
            .map_err(|_| CoordinatorError::Journal)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{relay_lifecycle::RelayJournalPhase, storage};
    use std::path::Path;

    struct FakeBackend {
        fail_start: bool,
        fail_activate: bool,
        fail_restore: bool,
        calls: Vec<&'static str>,
    }

    impl TunnelRouteBackend for FakeBackend {
        fn start_tunnel(&mut self, _session: &str, config: &str) -> Result<(), ()> {
            assert!(config.contains("PrivateKey"));
            self.calls.push("start");
            if self.fail_start {
                Err(())
            } else {
                Ok(())
            }
        }
        fn activate_scoped_routes(&mut self, _session: &str) -> Result<(), ()> {
            self.calls.push("activate");
            if self.fail_activate {
                Err(())
            } else {
                Ok(())
            }
        }
        fn restore_direct(&mut self, _session: &str, snapshot: &[u8]) -> Result<(), ()> {
            assert_eq!(snapshot, b"protected route state");
            self.calls.push("restore");
            if self.fail_restore {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    fn coordinator<'a>(
        db: &'a Connection,
        backend: FakeBackend,
    ) -> RelayCoordinator<'a, FakeBackend> {
        RelayCoordinator::new(db, backend, Duration::from_secs(10))
    }

    #[test]
    fn activation_failure_restores_before_returning() {
        let db = storage::open(Path::new(":memory:")).unwrap();
        let mut app = coordinator(
            &db,
            FakeBackend {
                fail_start: false,
                fail_activate: true,
                fail_restore: false,
                calls: Vec::new(),
            },
        );
        let config = crate::relay_tunnel::tests::coordinator_fixture();
        assert_eq!(
            app.start("coordinator-012345", b"protected route state", &config),
            Err(CoordinatorError::RouteActivation)
        );
        assert_eq!(app.backend().calls, vec!["start", "activate", "restore"]);
        assert!(app.journal.pending().unwrap().is_empty());
    }

    #[test]
    fn failed_rollback_remains_recoverable_and_retries() {
        let db = storage::open(Path::new(":memory:")).unwrap();
        let mut app = coordinator(
            &db,
            FakeBackend {
                fail_start: false,
                fail_activate: true,
                fail_restore: true,
                calls: Vec::new(),
            },
        );
        let config = crate::relay_tunnel::tests::coordinator_fixture();
        assert_eq!(
            app.start("coordinator-112345", b"protected route state", &config),
            Err(CoordinatorError::RestorePending)
        );
        assert_eq!(
            app.journal.pending().unwrap()[0].phase,
            RelayJournalPhase::Restoring
        );
        app.backend_mut().fail_restore = false;
        assert_eq!(
            app.recover_pending().unwrap(),
            RecoveryReport {
                restored: 1,
                pending: 0
            }
        );
        assert!(app.journal.pending().unwrap().is_empty());
    }

    #[test]
    fn watchdog_restores_a_stale_session() {
        let db = storage::open(Path::new(":memory:")).unwrap();
        let mut app = coordinator(
            &db,
            FakeBackend {
                fail_start: false,
                fail_activate: false,
                fail_restore: false,
                calls: Vec::new(),
            },
        );
        let config = crate::relay_tunnel::tests::coordinator_fixture();
        app.start("coordinator-212345", b"protected route state", &config)
            .unwrap();
        app.heartbeat("coordinator-212345").unwrap();
        let report = app.watchdog_check(Instant::now() + Duration::from_secs(20));
        assert_eq!(
            report,
            RecoveryReport {
                restored: 1,
                pending: 0
            }
        );
        assert_eq!(app.backend().calls, vec!["start", "activate", "restore"]);
        assert!(app.journal.pending().unwrap().is_empty());
    }

    #[test]
    fn startup_recovery_restores_interrupted_setup() {
        let db = storage::open(Path::new(":memory:")).unwrap();
        let journal = crate::relay_lifecycle::RestoreJournal::new(&db);
        journal
            .prepare("coordinator-312345", b"protected route state")
            .unwrap();
        journal.tunnel_ready("coordinator-312345").unwrap();
        journal.routes_active("coordinator-312345").unwrap();
        let mut app = coordinator(
            &db,
            FakeBackend {
                fail_start: false,
                fail_activate: false,
                fail_restore: false,
                calls: Vec::new(),
            },
        );
        assert_eq!(
            app.recover_pending().unwrap(),
            RecoveryReport {
                restored: 1,
                pending: 0
            }
        );
        assert_eq!(app.backend().calls, vec!["restore"]);
        assert!(app.journal.pending().unwrap().is_empty());
    }
}
