//! Conflict resolution policy (HQWins, EdgeWins, MergeRules).

#[derive(Debug, Clone, Copy)]
pub enum ConflictPolicy {
    HqWins,
    EdgeWins,
    MergeRules,
}
