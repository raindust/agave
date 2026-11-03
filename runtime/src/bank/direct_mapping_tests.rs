use {
    super::*,
    crate::{genesis_utils::activate_feature, snapshot_bank_utils, snapshot_utils},
    solana_accounts_db::accounts_db::ACCOUNTS_DB_CONFIG_FOR_TESTING,
    solana_sdk::{
        bpf_loader,
        epoch_schedule::EpochSchedule,
        feature_set::{
            blake3_syscall_enabled, bpf_account_data_direct_mapping,
            enable_program_runtime_v2_and_loader_v4, zk_elgamal_proof_program_enabled,
            zk_token_sdk_enabled,
        },
        instruction::Instruction,
        signature::{Keypair, Signer},
        system_program,
    },
    tempfile::TempDir,
};

fn unsafe_feature_ids() -> HashSet<Pubkey> {
    [
        bpf_account_data_direct_mapping::id(),
        blake3_syscall_enabled::id(),
        zk_token_sdk_enabled::id(),
        zk_elgamal_proof_program_enabled::id(),
        enable_program_runtime_v2_and_loader_v4::id(),
    ]
    .into_iter()
    .collect()
}

fn genesis_and_runtime_config(disable_slot: Option<Slot>) -> (GenesisConfig, RuntimeConfig) {
    let (mut genesis, _) = solana_sdk::genesis_config::create_genesis_config(1_000_000_000);
    genesis.epoch_schedule = EpochSchedule::custom(32, 32, false);
    activate_feature(&mut genesis, bpf_account_data_direct_mapping::id());
    let runtime = RuntimeConfig {
        feature_deactivation: disable_slot.map(|slot| FeatureDeactivationConfig {
            slot,
            feature_ids: [bpf_account_data_direct_mapping::id()]
                .into_iter()
                .collect(),
        }),
        ..RuntimeConfig::default()
    };
    (genesis, runtime)
}

fn new_bank(genesis: &GenesisConfig, runtime: &RuntimeConfig) -> Bank {
    Bank::new_with_paths_for_tests(
        genesis,
        Arc::new(runtime.clone()),
        Vec::new(),
        AccountSecondaryIndexes::default(),
        AccountShrinkThreshold::default(),
    )
}

fn assert_direct_mapping(bank: &Bank, active: bool) {
    assert_eq!(
        bank.feature_set
            .is_active(&bpf_account_data_direct_mapping::id()),
        active,
        "slot {}",
        bank.slot()
    );
    let cache = bank.transaction_processor.program_cache.read().unwrap();
    let env = cache.get_environments_for_epoch(bank.epoch());
    assert_eq!(
        env.program_runtime_v1.get_config().aligned_memory_mapping,
        !active
    );
    assert_eq!(
        env.program_runtime_v1.get_config().enable_stack_frame_gaps,
        !active
    );
}

#[test]
fn direct_mapping_hard_fork_slot_and_epoch_boundaries() {
    for disable_slot in [1, 7, 32] {
        let (genesis, runtime) = genesis_and_runtime_config(Some(disable_slot));
        let mut bank = Arc::new(new_bank(&genesis, &runtime));
        assert_direct_mapping(&bank, true);
        let mut slots = vec![disable_slot - 1, disable_slot, disable_slot + 1, 64, 96];
        slots.sort_unstable();
        slots.dedup();
        for slot in slots.into_iter().filter(|slot| *slot > 0) {
            bank = Arc::new(Bank::new_from_parent(bank, &Pubkey::default(), slot));
            assert_direct_mapping(&bank, slot < disable_slot);
            // The original genesis activation record must remain untouched.
            let account = bank
                .get_account(&bpf_account_data_direct_mapping::id())
                .unwrap();
            assert_eq!(
                feature::from_account(&account).unwrap().activated_at,
                Some(0)
            );
        }
    }
}

#[test]
fn direct_mapping_hard_fork_skipped_slot_and_no_schedule() {
    for disable_slot in [None, Some(7)] {
        let (genesis, runtime) = genesis_and_runtime_config(disable_slot);
        let bank = Arc::new(new_bank(&genesis, &runtime));
        let bank = Arc::new(Bank::new_from_parent(bank, &Pubkey::default(), 6));
        assert_direct_mapping(&bank, true);
        let bank = Bank::new_from_parent(bank, &Pubkey::default(), 9);
        assert_direct_mapping(&bank, disable_slot.is_none());
    }
    let (genesis, runtime) = genesis_and_runtime_config(Some(0));
    assert_direct_mapping(&new_bank(&genesis, &runtime), false);
}

#[test]
fn direct_mapping_hard_fork_preserves_parent_cache() {
    let (genesis, runtime) = genesis_and_runtime_config(Some(7));
    let parent = Arc::new(new_bank(&genesis, &runtime));
    let parent = Arc::new(Bank::new_from_parent(parent, &Pubkey::default(), 6));
    let old_cache = parent.transaction_processor.program_cache.clone();
    let child = Arc::new(Bank::new_from_parent(parent.clone(), &Pubkey::default(), 7));
    assert_direct_mapping(&child, false);
    assert_direct_mapping(&parent, true);
    assert!(!Arc::ptr_eq(
        &old_cache,
        &child.transaction_processor.program_cache
    ));
    let next = Bank::new_from_parent(child.clone(), &Pubkey::default(), 8);
    assert_direct_mapping(&next, false);
    assert!(Arc::ptr_eq(
        &child.transaction_processor.program_cache,
        &next.transaction_processor.program_cache
    ));
}

#[test]
fn direct_mapping_hard_fork_reloads_warm_program_and_discards_upcoming_environment() {
    for disable_slot in [7, 32] {
        let (genesis, runtime) = genesis_and_runtime_config(Some(disable_slot));
        let bank = new_bank(&genesis, &runtime);
        let payer = Keypair::new();
        bank.store_account_and_update_capitalization(
            &payer.pubkey(),
            &AccountSharedData::new(1_000_000_000, 0, &system_program::id()),
        );
        let program_id = Pubkey::new_unique();
        let elf = include_bytes!("../../../programs/bpf_loader/test_elfs/out/noop_aligned.so");
        bank.store_account_and_update_capitalization(
            &program_id,
            &AccountSharedData::from(Account {
                lamports: genesis.rent.minimum_balance(elf.len()),
                data: elf.to_vec(),
                owner: bpf_loader::id(),
                executable: true,
                rent_epoch: u64::MAX,
            }),
        );
        let (bank, forks) = bank.wrap_with_bank_forks_for_tests();
        bank.set_fork_graph_in_program_cache(Arc::downgrade(&forks));
        let parent = Arc::new(Bank::new_from_parent(
            bank,
            &Pubkey::default(),
            disable_slot - 1,
        ));
        let execute = |bank: &Bank, data| {
            let tx = Transaction::new_signed_with_payer(
                &[Instruction {
                    program_id,
                    accounts: vec![],
                    data: vec![data],
                }],
                Some(&payer.pubkey()),
                &[&payer],
                bank.last_blockhash(),
            );
            bank.process_transaction(&tx).unwrap();
        };
        execute(&parent, 0);
        let old_cache = parent.transaction_processor.program_cache.clone();
        {
            let mut cache = old_cache.write().unwrap();
            assert!(cache
                .get_flattened_entries(true, true)
                .iter()
                .any(|(id, _)| *id == program_id));
            cache.upcoming_environments = Some(cache.environments.clone());
            cache.programs_to_recompile = cache.get_flattened_entries(true, true);
        }
        let child = Bank::new_from_parent(parent.clone(), &Pubkey::default(), disable_slot);
        assert_direct_mapping(&child, false);
        assert_direct_mapping(&parent, true);
        // An epoch boundary can prepare a fresh upcoming environment immediately.
        // It must never reuse the parent's direct-mapping environment.
        if let Some(upcoming) = &child
            .transaction_processor
            .program_cache
            .read()
            .unwrap()
            .upcoming_environments
        {
            assert!(
                upcoming
                    .program_runtime_v1
                    .get_config()
                    .aligned_memory_mapping
            );
            assert!(!Arc::ptr_eq(
                &upcoming.program_runtime_v1,
                &old_cache.read().unwrap().environments.program_runtime_v1
            ));
        }
        assert!(old_cache.read().unwrap().upcoming_environments.is_some());
        execute(&child, 1);
        let cache = child.transaction_processor.program_cache.read().unwrap();
        let environment = &cache
            .get_environments_for_epoch(child.epoch())
            .program_runtime_v1;
        assert!(cache
            .get_slot_versions_for_tests(&program_id)
            .iter()
            .any(|entry| {
                entry
                    .program
                    .get_environment()
                    .is_some_and(|env| Arc::ptr_eq(env, environment))
            }));
    }
}

#[test]
fn direct_mapping_hard_fork_cannot_be_reactivated_by_feature_account() {
    let (genesis, runtime) = genesis_and_runtime_config(Some(7));
    let bank = Arc::new(new_bank(&genesis, &runtime));
    let mut bank = Bank::new_from_parent(bank, &Pubkey::default(), 7);
    for activated_at in [Some(0), None] {
        let id = bpf_account_data_direct_mapping::id();
        bank.store_account(
            &id,
            &feature::create_account(&feature::Feature { activated_at }, 1_000_000),
        );
        bank.apply_feature_activations(ApplyFeatureActivationsCaller::NewFromParent, false);
        assert_direct_mapping(&bank, false);
        assert_eq!(
            feature::from_account(&bank.get_account(&id).unwrap())
                .unwrap()
                .activated_at,
            activated_at
        );
    }
}

#[test]
fn direct_mapping_hard_fork_snapshot_restore_before_and_after() {
    for snapshot_slot in [6, 7, 33] {
        let (genesis, runtime) = genesis_and_runtime_config(Some(7));
        let bank = Arc::new(new_bank(&genesis, &runtime));
        let bank = Bank::new_from_parent(bank, &Pubkey::default(), snapshot_slot);
        bank.fill_bank_with_ticks_for_tests();
        bank.squash();
        let (_accounts_temp, accounts_dir) = snapshot_utils::create_tmp_accounts_dir_for_tests();
        let snapshots = TempDir::new().unwrap();
        let full = TempDir::new().unwrap();
        let incremental = TempDir::new().unwrap();
        let archive = snapshot_bank_utils::bank_to_full_snapshot_archive(
            snapshots.path(),
            &bank,
            None,
            full.path(),
            incremental.path(),
            snapshot_utils::ArchiveFormat::Tar,
        )
        .unwrap();
        let (restored, _) = snapshot_bank_utils::bank_from_snapshot_archives(
            &[accounts_dir],
            snapshots.path(),
            &archive,
            None,
            &genesis,
            &runtime,
            None,
            None,
            AccountSecondaryIndexes::default(),
            None,
            AccountShrinkThreshold::default(),
            false,
            false,
            false,
            false,
            Some(ACCOUNTS_DB_CONFIG_FOR_TESTING),
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        restored.wait_for_initial_accounts_hash_verification_completed_for_tests();
        assert_eq!(restored.hash(), bank.hash());
        assert_direct_mapping(&restored, snapshot_slot < 7);
        let mut restored = Arc::new(restored);
        for slot in [snapshot_slot + 1, 64, 96] {
            restored = Arc::new(Bank::new_from_parent(restored, &Pubkey::default(), slot));
            assert_direct_mapping(&restored, slot < 7);
        }
    }
}

#[test]
fn scheduled_feature_deactivation_masks_all_configured_features_and_builtins() {
    let (mut genesis, _) = solana_sdk::genesis_config::create_genesis_config(1_000_000_000);
    for id in unsafe_feature_ids() {
        activate_feature(&mut genesis, id);
    }
    let runtime = RuntimeConfig {
        feature_deactivation: Some(FeatureDeactivationConfig {
            slot: 7,
            feature_ids: unsafe_feature_ids(),
        }),
        ..RuntimeConfig::default()
    };
    let parent = Arc::new(new_bank(&genesis, &runtime));
    let parent = Arc::new(Bank::new_from_parent(parent, &Pubkey::default(), 6));
    for id in unsafe_feature_ids() {
        assert!(parent.feature_set.is_active(&id));
    }
    let parent_programs: HashSet<_> = parent
        .transaction_processor
        .program_cache
        .read()
        .unwrap()
        .get_flattened_entries_for_tests()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(parent_programs.contains(&solana_zk_token_sdk::zk_token_proof_program::id()));
    assert!(parent_programs.contains(&solana_zk_sdk::zk_elgamal_proof_program::id()));
    assert!(parent_programs.contains(&solana_sdk::loader_v4::id()));

    let mut child = Bank::new_from_parent(parent, &Pubkey::default(), 7);
    for id in unsafe_feature_ids() {
        assert!(!child.feature_set.is_active(&id));
        assert_eq!(
            feature::from_account(&child.get_account(&id).unwrap())
                .unwrap()
                .activated_at,
            Some(0)
        );
    }
    let child_programs: HashSet<_> = child
        .transaction_processor
        .program_cache
        .read()
        .unwrap()
        .get_flattened_entries_for_tests()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(!child_programs.contains(&solana_zk_token_sdk::zk_token_proof_program::id()));
    assert!(!child_programs.contains(&solana_zk_sdk::zk_elgamal_proof_program::id()));
    assert!(!child_programs.contains(&solana_sdk::loader_v4::id()));

    child.apply_feature_activations(ApplyFeatureActivationsCaller::NewFromParent, false);
    for id in unsafe_feature_ids() {
        assert!(!child.feature_set.is_active(&id));
    }
}
