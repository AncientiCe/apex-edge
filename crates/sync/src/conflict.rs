//! Conflict resolution policy (HQWins, EdgeWins, MergeRules).

#[derive(Debug, Clone, Copy)]
pub enum ConflictPolicy {
    HqWins,
    EdgeWins,
    MergeRules,
}

#[cfg(test)]
mod tests {
    use super::ConflictPolicy;

    #[test]
    fn conflict_policy_variants_exist() {
        let _ = ConflictPolicy::HqWins;
        let _ = ConflictPolicy::EdgeWins;
        let _ = ConflictPolicy::MergeRules;
    }
}
