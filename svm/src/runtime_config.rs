use {
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_sdk::{clock::Slot, pubkey::Pubkey},
    std::collections::HashSet,
};

/// Consensus policy for deactivating feature gates without rewriting their
/// on-chain feature accounts.
#[derive(Debug, Default, Clone)]
pub struct FeatureDeactivationConfig {
    pub slot: Slot,
    pub feature_ids: HashSet<Pubkey>,
}

#[cfg(all(RUSTC_WITH_SPECIALIZATION, feature = "frozen-abi"))]
impl ::solana_frozen_abi::abi_example::AbiExample for RuntimeConfig {
    fn example() -> Self {
        // RuntimeConfig is not Serialize so just rely on Default.
        RuntimeConfig::default()
    }
}

/// Encapsulates flags that can be used to tweak the runtime behavior.
#[derive(Debug, Default, Clone)]
pub struct RuntimeConfig {
    /// Consensus upgrade: keep these feature gates inactive at and after the
    /// configured slot. Must be supplied consistently for genesis and snapshots.
    pub feature_deactivation: Option<FeatureDeactivationConfig>,
    pub compute_budget: Option<ComputeBudget>,
    pub log_messages_bytes_limit: Option<usize>,
    pub transaction_account_lock_limit: Option<usize>,
}
