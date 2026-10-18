//! Shared test helpers.

use crate::event::{EventCost, EventKind, TokenCounts, UsageEvent};
use crate::ids::{
    AgentId, DedupKey, EventId, MachineId, ModelId, ProfileId, ProviderId, SessionId,
};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A minimal usage event: default profile, provider `anthropic`, model `m`, no tokens, unpriced.
pub fn usage_event(
    agent: AgentId,
    session: &str,
    at: &str,
) -> Result<UsageEvent, Box<dyn std::error::Error>> {
    let machine = MachineId::try_from(uuid::Uuid::from_u128(1))?;
    let dedup_key = DedupKey::try_from(format!("{session}|{at}"))?;
    Ok(UsageEvent {
        id: EventId::new(&machine, agent, &dedup_key),
        agent,
        profile: ProfileId::default_profile(),
        dedup_key,
        at: at.parse()?,
        session: SessionId::try_from(session)?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        project: None,
        client: None,
        source_file: None,
        approximate: false,
        provider: Some(ProviderId::try_from("anthropic")?),
        model: ModelId::try_from("m")?,
        kind: EventKind::Message,
        tokens: TokenCounts::default(),
        cost: EventCost::default(),
        service_tier: None,
    })
}
