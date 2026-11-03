use solana_compute_budget::compute_budget::ComputeBudget;
use solana_sdk::clock::Slot;

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
    /// Consensus upgrade: use copying-mode account memory at and after this slot.
    /// Must be supplied consistently when loading genesis and snapshots.
    pub bpf_account_data_direct_mapping_disable_slot: Option<Slot>,
    pub compute_budget: Option<ComputeBudget>,
    pub log_messages_bytes_limit: Option<usize>,
    pub transaction_account_lock_limit: Option<usize>,
}
