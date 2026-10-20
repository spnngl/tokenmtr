-- The agent skill a tool call loaded (Claude Code `Skill` input, pi `read` of a `SKILL.md`): the
-- one value kept from tool arguments, a validated name, never a path. NULL for every other call
-- and for calls ingested before this column existed (no backfill).
ALTER TABLE tool_call ADD COLUMN skill TEXT;
