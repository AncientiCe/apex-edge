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
        let policies = [
            ConflictPolicy::HqWins,
            ConflictPolicy::EdgeWins,
            ConflictPolicy::MergeRules,
        ];
        assert_eq!(policies.len(), 3);
    }
}
