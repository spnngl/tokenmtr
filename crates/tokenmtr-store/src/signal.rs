//! The `session_signal` table: API errors and compactions per session (D32).

use jiff::Timestamp;
use rusqlite::{Connection, TransactionBehavior, named_params};
use tokenmtr_core::{AgentId, SessionId, SignalKind};

use crate::codec::{enum_text, to_us};
use crate::error::StoreError;

/// One signal seen in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSignal {
    /// Agent that wrote the session.
    pub agent: AgentId,
    /// The session.
    pub session: SessionId,
    /// What happened.
    pub kind: SignalKind,
    /// When it happened.
    pub at: Timestamp,
}

const INSERT: &str = "INSERT OR IGNORE INTO session_signal (agent, session, kind, at_us) \
    VALUES (:agent, :session, :kind, :at_us)";

/// Stores signals not seen before and returns how many were new.
pub(crate) fn record(conn: &mut Connection, signals: &[SessionSignal]) -> Result<u64, StoreError> {
    if signals.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut inserted = 0;
    {
        let mut statement = tx.prepare_cached(INSERT)?;
        for signal in signals {
            inserted += statement.execute(named_params! {
                ":agent": signal.agent.as_str(),
                ":session": signal.session.as_str(),
                ":kind": enum_text(&signal.kind)?,
                ":at_us": to_us(signal.at),
            })? as u64;
        }
    }
    tx.commit()?;
    Ok(inserted)
}
