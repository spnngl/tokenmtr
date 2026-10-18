//! What parsers produce: events, tool calls and signals, deduplicated as they are pushed.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use tokenmtr_core::{
    AgentId, DedupKey, EventId, MachineId, ParsedEvent, SessionId, SignalKind, ToolCall,
};

/// A tool call as parsed. [`ToolCall::event`] is empty because an [`EventId`] needs the machine;
/// `event_key` names the usage event of the same message, and
/// [`into_tool_call`](Self::into_tool_call) derives the id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedToolCall {
    /// The call, with `event` unset.
    pub call: ToolCall,
    /// Dedup key of the usage event of the same message, when the line produced one.
    pub event_key: Option<DedupKey>,
}

impl ParsedToolCall {
    /// Links the call to its usage event for this machine.
    #[must_use]
    pub fn into_tool_call(self, machine: &MachineId) -> ToolCall {
        let event = self
            .event_key
            .map(|key| EventId::new(machine, self.call.agent, &key));
        ToolCall { event, ..self.call }
    }
}

/// A session-level signal, stored in `session_signal`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ParsedSignal {
    /// Agent that wrote it.
    pub agent: AgentId,
    /// Owning session.
    pub session: SessionId,
    /// What happened.
    pub kind: SignalKind,
    /// When it happened.
    pub at: jiff::Timestamp,
}

/// A tool result whose tool call is not in the batch, e.g. the call was parsed by an earlier
/// scan. The ingestor applies it to the stored call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedToolResult {
    /// Agent that wrote it.
    pub agent: AgentId,
    /// The call the result answers.
    pub tool_use_id: String,
    /// Whether the tool reported an error.
    pub is_error: bool,
}

/// The parts of a [`ParseBatch`].
#[derive(Debug, Default)]
pub struct BatchParts {
    /// Usage events, one per dedup key.
    pub events: Vec<ParsedEvent>,
    /// Tool calls, one per tool-use id.
    pub tool_calls: Vec<ParsedToolCall>,
    /// Signals, without duplicates.
    pub signals: Vec<ParsedSignal>,
    /// Tool results that matched no call of the batch.
    pub orphan_tool_results: Vec<ParsedToolResult>,
}

/// Output of one or more parse calls, deduplicated on push:
/// - **Events:** unique per `(agent, dedup_key)`. A later push replaces the earlier event in place
///   (last wins: Claude writes several streaming lines per message, and the first under-counts
///   output); the event keeps the position of its first occurrence.
/// - **Tool calls:** unique per `(agent, tool_use_id)`. The first push wins; a later push only
///   fills a missing `event_key`.
/// - **Tool results:** set `is_error` on the call, whether the call was pushed before or after the
///   result. Results without a call in the batch are kept apart.
/// - **Signals:** unique per `(agent, session, kind, at)`.
///
/// Parsing a file in two calls into one batch therefore gives the same batch as parsing it whole.
#[derive(Debug, Default)]
pub struct ParseBatch {
    events: Vec<ParsedEvent>,
    event_slots: HashMap<(AgentId, DedupKey), usize>,
    tool_calls: Vec<ParsedToolCall>,
    tool_slots: HashMap<(AgentId, String), usize>,
    orphan_results: BTreeMap<(AgentId, String), bool>,
    signals: Vec<ParsedSignal>,
    signal_set: HashSet<ParsedSignal>,
}

impl ParseBatch {
    /// An empty batch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a usage event; replaces the earlier one with the same key (last wins).
    pub fn push_event(&mut self, event: ParsedEvent) {
        match self
            .event_slots
            .entry((event.agent, event.dedup_key.clone()))
        {
            Entry::Occupied(slot) => self.events[*slot.get()] = event,
            Entry::Vacant(slot) => {
                slot.insert(self.events.len());
                self.events.push(event);
            }
        }
    }

    /// Adds a tool call unless its id is already in the batch.
    pub fn push_tool_call(&mut self, mut parsed: ParsedToolCall) {
        let key = (parsed.call.agent, parsed.call.tool_use_id.clone());
        match self.tool_slots.entry(key) {
            Entry::Occupied(slot) => {
                let kept = &mut self.tool_calls[*slot.get()];
                if kept.event_key.is_none() {
                    kept.event_key = parsed.event_key;
                }
            }
            Entry::Vacant(slot) => {
                let key = slot.key();
                if let Some(is_error) = self.orphan_results.remove(key) {
                    parsed.call.is_error = Some(is_error);
                }
                slot.insert(self.tool_calls.len());
                self.tool_calls.push(parsed);
            }
        }
    }

    /// Records a tool result: sets the flag on the call when it is in the batch, else keeps it
    /// as an orphan. The last result for an id wins.
    pub fn push_tool_result(&mut self, result: ParsedToolResult) {
        let key = (result.agent, result.tool_use_id);
        if let Some(slot) = self.tool_slots.get(&key) {
            self.tool_calls[*slot].call.is_error = Some(result.is_error);
            return;
        }
        self.orphan_results.insert(key, result.is_error);
    }

    /// Adds a signal unless an equal one is already in the batch.
    pub fn push_signal(&mut self, signal: ParsedSignal) {
        if self.signal_set.insert(signal.clone()) {
            self.signals.push(signal);
        }
    }

    /// Usage events in first-occurrence order.
    #[must_use]
    pub fn events(&self) -> &[ParsedEvent] {
        &self.events
    }

    /// Tool calls in first-occurrence order.
    #[must_use]
    pub fn tool_calls(&self) -> &[ParsedToolCall] {
        &self.tool_calls
    }

    /// Signals in first-occurrence order.
    #[must_use]
    pub fn signals(&self) -> &[ParsedSignal] {
        &self.signals
    }

    /// Tool results that matched no call of the batch, ordered by agent and id.
    pub fn orphan_tool_results(&self) -> impl Iterator<Item = ParsedToolResult> + '_ {
        self.orphan_results
            .iter()
            .map(|((agent, tool_use_id), is_error)| ParsedToolResult {
                agent: *agent,
                tool_use_id: tool_use_id.clone(),
                is_error: *is_error,
            })
    }

    /// Whether the batch holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.tool_calls.is_empty()
            && self.signals.is_empty()
            && self.orphan_results.is_empty()
    }

    /// Splits the batch into its parts, e.g. to hand to the writer.
    #[must_use]
    pub fn into_parts(self) -> BatchParts {
        let orphan_tool_results = self.orphan_tool_results().collect();
        BatchParts {
            events: self.events,
            tool_calls: self.tool_calls,
            signals: self.signals,
            orphan_tool_results,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokenmtr_core::{EventKind, ModelId, ProfileId, TokenCounts};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn event(key: &str, output: u64) -> Result<ParsedEvent, Box<dyn std::error::Error>> {
        Ok(ParsedEvent {
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            dedup_key: DedupKey::try_from(key)?,
            at: "2026-01-01T00:00:00Z".parse()?,
            session: SessionId::try_from("s")?,
            parent_session: None,
            subagent: None,
            is_sidechain: false,
            cwd: None,
            git_branch: None,
            source_file: None,
            approximate: false,
            provider: None,
            model: ModelId::try_from("m")?,
            kind: EventKind::Message,
            tokens: TokenCounts {
                output,
                ..TokenCounts::default()
            },
            reported_cost: None,
            service_tier: None,
            modifiers: None,
        })
    }

    fn call(id: &str) -> Result<ParsedToolCall, Box<dyn std::error::Error>> {
        Ok(ParsedToolCall {
            call: ToolCall {
                agent: AgentId::CLAUDE,
                tool_use_id: id.to_owned(),
                event: None,
                session: SessionId::try_from("s")?,
                at: "2026-01-01T00:00:00Z".parse()?,
                tool_name: "Bash".to_owned(),
                mcp_server: None,
                is_error: None,
            },
            event_key: None,
        })
    }

    fn result(id: &str, is_error: bool) -> ParsedToolResult {
        ParsedToolResult {
            agent: AgentId::CLAUDE,
            tool_use_id: id.to_owned(),
            is_error,
        }
    }

    #[test]
    fn last_event_wins_in_the_first_position() -> TestResult {
        let mut batch = ParseBatch::new();
        batch.push_event(event("a", 1)?);
        batch.push_event(event("b", 5)?);
        batch.push_event(event("a", 9)?);
        let outputs: Vec<_> = batch.events().iter().map(|e| e.tokens.output).collect();
        assert_eq!(outputs, [9, 5]);
        Ok(())
    }

    #[test]
    fn a_result_before_its_call_still_lands() -> TestResult {
        let mut batch = ParseBatch::new();
        batch.push_tool_result(result("t1", true));
        assert_eq!(batch.orphan_tool_results().count(), 1);
        batch.push_tool_call(call("t1")?);
        assert_eq!(batch.tool_calls()[0].call.is_error, Some(true));
        assert_eq!(batch.orphan_tool_results().count(), 0);
        Ok(())
    }

    #[test]
    fn a_call_is_kept_once_and_fills_its_event_key() -> TestResult {
        let mut batch = ParseBatch::new();
        batch.push_tool_call(call("t1")?);
        let mut again = call("t1")?;
        again.event_key = Some(DedupKey::try_from("k")?);
        batch.push_tool_call(again);
        assert_eq!(batch.tool_calls().len(), 1);
        assert!(batch.tool_calls()[0].event_key.is_some());
        Ok(())
    }

    #[test]
    fn equal_signals_collapse() -> TestResult {
        let mut batch = ParseBatch::new();
        let signal = ParsedSignal {
            agent: AgentId::CLAUDE,
            session: SessionId::try_from("s")?,
            kind: SignalKind::Compaction,
            at: "2026-01-01T00:00:00Z".parse()?,
        };
        batch.push_signal(signal.clone());
        batch.push_signal(signal);
        assert_eq!(batch.signals().len(), 1);
        Ok(())
    }
}
