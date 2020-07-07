// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#[allow(dead_code)]
mod helpers;

use crate::helpers::block_builders::{append_block_with_coinbase, generate_new_block};
use helpers::{
    block_builders::{create_coinbase, create_genesis_block},
    nodes::create_network_with_2_base_nodes_with_config,
};
use tari_core::{
    base_node::{
        service::BaseNodeServiceConfig,
        states::{
            BestChainMetadataBlockSyncInfo,
            BlockSyncConfig,
            HeaderSync,
            HorizonStateSync,
            HorizonSyncConfig,
            StateEvent,
            SyncPeerConfig,
        },
        BaseNodeStateMachine,
        BaseNodeStateMachineConfig,
        ChainBalanceValidator,
        SyncValidators,
    },
    chain_storage::{BlockchainDatabaseConfig, MmrTree},
    consensus::{ConsensusConstantsBuilder, ConsensusManagerBuilder, Network},
    mempool::MempoolServiceConfig,
    transactions::{
        tari_amount::{uT, T},
        transaction::TransactionOutput,
        types::CryptoFactories,
    },
    txn_schema,
    validation::mocks::MockValidator,
};
use tari_mmr::MmrCacheConfig;
use tari_p2p::services::liveness::LivenessConfig;
use tari_shutdown::Shutdown;
use tari_test_utils::unpack_enum;
use tempfile::tempdir;
use tokio::runtime::Runtime;

#[test]
fn test_pruned_mode_sync_with_future_horizon_sync_height() {
    // Number of blocks to create in addition to the genesis
    const NUM_BLOCKS: u64 = 10;
    const SYNC_OFFSET: u64 = 2;
    const PRUNING_HORIZON: u64 = 4;
    let mut runtime = Runtime::new().unwrap();
    let factories = CryptoFactories::default();
    let temp_dir = tempdir().unwrap();
    let network = Network::LocalNet;
    let consensus_constants = ConsensusConstantsBuilder::new(network)
        .with_emission_amounts(100_000_000.into(), 0.999, 100.into())
        .build();
    let (genesis_block, _) = create_genesis_block(&factories, &consensus_constants);
    let consensus_manager = ConsensusManagerBuilder::new(network)
        .with_consensus_constants(consensus_constants)
        .with_block(genesis_block.clone())
        .build();
    let blockchain_db_config = BlockchainDatabaseConfig {
        orphan_storage_capacity: 3,
        pruning_horizon: PRUNING_HORIZON,
        pruned_mode_cleanup_interval: 3,
    };
    let (alice_node, bob_node, consensus_manager) = create_network_with_2_base_nodes_with_config(
        &mut runtime,
        blockchain_db_config,
        BaseNodeServiceConfig::default(),
        MmrCacheConfig::default(),
        MempoolServiceConfig::default(),
        LivenessConfig::default(),
        consensus_manager,
        temp_dir.path().to_str().unwrap(),
    );
    let mut horizon_sync_config = HorizonSyncConfig::default();
    horizon_sync_config.horizon_sync_height_offset = SYNC_OFFSET;
    let state_machine_config = BaseNodeStateMachineConfig {
        block_sync_config: BlockSyncConfig::default(),
        horizon_sync_config,
        sync_peer_config: SyncPeerConfig::default(),
    };
    let shutdown = Shutdown::new();
    let mut alice_state_machine = BaseNodeStateMachine::new(
        &alice_node.blockchain_db,
        &alice_node.local_nci,
        &alice_node.outbound_nci,
        alice_node.comms.peer_manager(),
        alice_node.comms.connectivity(),
        alice_node.chain_metadata_handle.get_event_stream(),
        state_machine_config,
        SyncValidators::new(MockValidator::new(true), MockValidator::new(true)),
        shutdown.to_signal(),
    );

    runtime.block_on(async {
        let alice_db = &alice_node.blockchain_db;
        let bob_db = &bob_node.blockchain_db;
        let mut prev_block = genesis_block.clone();
        for _ in 0..NUM_BLOCKS {
            // Need coinbases for kernels and utxos
            let (block, _) =
                append_block_with_coinbase(&factories, bob_db, &prev_block, vec![], &consensus_manager, 1.into())
                    .unwrap();
            prev_block = block;
        }

        // Both nodes are running in pruned mode and can not use block sync to synchronize state. Sync horizon state
        // from genesis block to horizon_sync_height and then block sync to the tip.
        let network_tip = bob_db.get_metadata().unwrap();
        let mut sync_peers = vec![bob_node.node_identity.node_id().clone()];

        // Synchronize headers
        let state_event = HeaderSync::new(network_tip.clone(), sync_peers.clone())
            .next_event(&mut alice_state_machine)
            .await;
        unpack_enum!(StateEvent::HeadersSynchronized(local_metadata, sync_height) = state_event);

        // Synchronize Kernels and UTXOs
        assert_eq!(sync_height, NUM_BLOCKS - PRUNING_HORIZON + SYNC_OFFSET);
        let state_event = HorizonStateSync::new(local_metadata, network_tip.clone(), sync_peers.clone(), sync_height)
            .next_event(&mut alice_state_machine)
            .await;
        assert_eq!(state_event, StateEvent::HorizonStateSynchronized);
        let alice_metadata = alice_db.get_metadata().unwrap();

        // Check Kernel MMR nodes after horizon sync
        let alice_num_kernels = alice_db.fetch_mmr_node_count(MmrTree::Kernel, sync_height).unwrap();
        let bob_num_kernels = bob_db.fetch_mmr_node_count(MmrTree::Kernel, sync_height).unwrap();
        assert_eq!(alice_num_kernels, bob_num_kernels);
        let alice_kernel_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, alice_num_kernels, Some(sync_height))
            .unwrap();
        let bob_kernel_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, bob_num_kernels, Some(sync_height))
            .unwrap();
        assert_eq!(alice_kernel_nodes, bob_kernel_nodes);
        // Local height should now be at the horizon sync height
        assert_eq!(alice_metadata.height_of_longest_chain.unwrap(), sync_height);

        // Synchronize full blocks
        let state_event = BestChainMetadataBlockSyncInfo
            .next_event(&mut alice_state_machine, &network_tip, &mut sync_peers)
            .await;
        assert_eq!(state_event, StateEvent::BlocksSynchronized);
        let alice_metadata = alice_db.get_metadata().unwrap();
        assert_eq!(alice_metadata, network_tip);

        // Check headers
        let network_tip_height = network_tip.height_of_longest_chain();
        let block_nums = (0..=network_tip_height).collect::<Vec<u64>>();
        let alice_headers = alice_db.fetch_headers(block_nums.clone()).unwrap();
        let bob_headers = bob_db.fetch_headers(block_nums).unwrap();
        assert_eq!(alice_headers, bob_headers);
        // Check Kernel MMR nodes
        let alice_num_kernels = alice_db
            .fetch_mmr_node_count(MmrTree::Kernel, network_tip_height)
            .unwrap();
        let bob_num_kernels = bob_db
            .fetch_mmr_node_count(MmrTree::Kernel, network_tip_height)
            .unwrap();
        assert_eq!(alice_num_kernels, bob_num_kernels);
        let alice_kernel_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, alice_num_kernels, Some(network_tip_height))
            .unwrap();
        let bob_kernel_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, bob_num_kernels, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_kernel_nodes, bob_kernel_nodes);
        // Check Kernels
        let alice_kernel_hashes = alice_kernel_nodes.iter().map(|n| n.0.clone()).collect::<Vec<_>>();
        let bob_kernels_hashes = bob_kernel_nodes.iter().map(|n| n.0.clone()).collect::<Vec<_>>();
        let alice_kernels = alice_db.fetch_kernels(alice_kernel_hashes).unwrap();
        let bob_kernels = bob_db.fetch_kernels(bob_kernels_hashes).unwrap();
        assert_eq!(alice_kernels, bob_kernels);
        // Check UTXO MMR nodes
        let alice_num_utxos = alice_db
            .fetch_mmr_node_count(MmrTree::Utxo, network_tip_height)
            .unwrap();
        let bob_num_utxos = bob_db.fetch_mmr_node_count(MmrTree::Utxo, network_tip_height).unwrap();
        assert_eq!(alice_num_utxos, bob_num_utxos);
        let alice_utxo_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::Utxo, 0, alice_num_utxos, Some(network_tip_height))
            .unwrap();
        let bob_utxo_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::Utxo, 0, bob_num_utxos, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_utxo_nodes, bob_utxo_nodes);
        // Check RangeProof MMR nodes
        let alice_num_rps = alice_db
            .fetch_mmr_node_count(MmrTree::RangeProof, network_tip_height)
            .unwrap();
        let bob_num_rps = bob_db
            .fetch_mmr_node_count(MmrTree::RangeProof, network_tip_height)
            .unwrap();
        assert_eq!(alice_num_rps, bob_num_rps);
        let alice_rps_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::RangeProof, 0, alice_num_rps, Some(network_tip_height))
            .unwrap();
        let bob_rps_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::RangeProof, 0, bob_num_rps, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_rps_nodes, bob_rps_nodes);
        // Check UTXOs
        let mut alice_utxos = Vec::<TransactionOutput>::new();
        for (hash, deleted) in alice_utxo_nodes {
            if !deleted {
                alice_utxos.push(alice_db.fetch_utxo(hash).unwrap());
            }
        }
        let mut bob_utxos = Vec::<TransactionOutput>::new();
        for (hash, deleted) in bob_utxo_nodes {
            if !deleted {
                bob_utxos.push(bob_db.fetch_utxo(hash).unwrap());
            }
        }
        assert_eq!(alice_utxos, bob_utxos);

        alice_node.comms.shutdown().await;
        bob_node.comms.shutdown().await;
    });
}

#[test]
fn test_pruned_mode_sync_with_spent_utxos() {
    env_logger::init();
    let mut runtime = Runtime::new().unwrap();
    let factories = CryptoFactories::default();
    let temp_dir = tempdir().unwrap();
    let network = Network::LocalNet;
    let consensus_constants = ConsensusConstantsBuilder::new(network)
        .with_emission_amounts(100_000_000.into(), 0.999, 100.into())
        .build();
    let (genesis_block, output) = create_genesis_block(&factories, &consensus_constants);
    let consensus_manager = ConsensusManagerBuilder::new(network)
        .with_consensus_constants(consensus_constants)
        .with_block(genesis_block.clone())
        .build();
    let blockchain_db_config = BlockchainDatabaseConfig {
        orphan_storage_capacity: 3,
        pruning_horizon: 4,
        pruned_mode_cleanup_interval: 2,
    };
    let (alice_node, bob_node, consensus_manager) = create_network_with_2_base_nodes_with_config(
        &mut runtime,
        blockchain_db_config,
        BaseNodeServiceConfig::default(),
        MmrCacheConfig::default(),
        MempoolServiceConfig::default(),
        LivenessConfig::default(),
        consensus_manager,
        temp_dir.path().to_str().unwrap(),
    );
    let mut horizon_sync_config = HorizonSyncConfig::default();
    horizon_sync_config.horizon_sync_height_offset = 2;
    let state_machine_config = BaseNodeStateMachineConfig {
        block_sync_config: BlockSyncConfig::default(),
        horizon_sync_config,
        sync_peer_config: SyncPeerConfig::default(),
    };
    let shutdown = Shutdown::new();
    let mut alice_state_machine = BaseNodeStateMachine::new(
        &alice_node.blockchain_db,
        &alice_node.local_nci,
        &alice_node.outbound_nci,
        alice_node.comms.peer_manager(),
        alice_node.comms.connectivity(),
        alice_node.chain_metadata_handle.get_event_stream(),
        state_machine_config,
        SyncValidators::new(
            MockValidator::new(true),
            // TODO: Need a test helper which adds the correct reward to a coinbase UTXO as per consensus to use the
            //       ChainBalanceValidator
            MockValidator::new(true),
        ),
        shutdown.to_signal(),
    );

    runtime.block_on(async {
        let mut outputs = vec![output];

        let mut prev_block = genesis_block;
        for _ in 1..10 {
            // Need coinbases for kernels and utxos
            let (block, coinbase) = append_block_with_coinbase(
                &factories,
                &bob_node.blockchain_db,
                &prev_block,
                vec![],
                &consensus_manager,
                1.into(),
            )
            .unwrap();
            prev_block = block;
            outputs.push(coinbase);
        }

        // let txs = vec![txn_schema!(
        //     from: vec![outputs[0][0].clone()],
        //     to: vec![10 * T, 10 * T, 10 * T, 10 * T]
        // )];
        // generate_new_block(
        //     &mut bob_node.blockchain_db,
        //     &mut blocks,
        //     &mut outputs,
        //     txs,
        //     &consensus_manager.consensus_constants(),
        // )
        // .unwrap();
        // // Block2
        // let txs = vec![txn_schema!(from: vec![outputs[1][3].clone()], to: vec![6 * T])];
        // generate_new_block(
        //     &mut bob_node.blockchain_db,
        //     &mut blocks,
        //     &mut outputs,
        //     txs,
        //     &consensus_manager.consensus_constants(),
        // )
        // .unwrap();
        // // Block3
        // let txs = vec![txn_schema!(from: vec![outputs[2][0].clone()], to: vec![2 * T])];
        // generate_new_block(
        //     &mut bob_node.blockchain_db,
        //     &mut blocks,
        //     &mut outputs,
        //     txs,
        //     &consensus_manager.consensus_constants(),
        // )
        // .unwrap();
        // // Block4
        // let txs = vec![txn_schema!(from: vec![outputs[1][0].clone()], to: vec![2 * T])];
        // generate_new_block(
        //     &mut bob_node.blockchain_db,
        //     &mut blocks,
        //     &mut outputs,
        //     txs,
        //     &consensus_manager.consensus_constants(),
        // )
        // .unwrap();

        // Both nodes are running in pruned mode and can not use block sync to synchronize state. Sync horizon state
        // from genesis block to horizon_sync_height and then block sync to the tip.
        let alice_db = &alice_node.blockchain_db;
        let bob_db = &bob_node.blockchain_db;
        let network_tip = bob_db.get_metadata().unwrap();
        assert_eq!(network_tip.height_of_longest_chain(), 4);
        let mut sync_peers = vec![bob_node.node_identity.node_id().clone()];
        let state_event = HeaderSync::new(network_tip.clone(), sync_peers.clone())
            .next_event(&mut alice_state_machine)
            .await;
        unpack_enum!(StateEvent::HeadersSynchronized(local_metadata, sync_height) = state_event);
        // network tip - pruning horizon + offset
        assert_eq!(sync_height, 4 - 4 + 2);
        let state_event = HorizonStateSync::new(local_metadata, network_tip.clone(), sync_peers.clone(), sync_height)
            .next_event(&mut alice_state_machine)
            .await;
        assert_eq!(state_event, StateEvent::HorizonStateSynchronized);
        let state_event = BestChainMetadataBlockSyncInfo
            .next_event(&mut alice_state_machine, &network_tip, &mut sync_peers)
            .await;
        assert_eq!(state_event, StateEvent::BlocksSynchronized);
        let alice_metadata = alice_db.get_metadata().unwrap();
        assert_eq!(alice_metadata, network_tip);

        // Check headers
        let network_tip_height = network_tip.height_of_longest_chain.unwrap_or(0);
        let block_nums = (0..=network_tip_height).collect::<Vec<u64>>();
        let alice_headers = alice_db.fetch_headers(block_nums.clone()).unwrap();
        let bob_headers = bob_db.fetch_headers(block_nums).unwrap();
        assert_eq!(alice_headers, bob_headers);
        // Check Kernel MMR nodes
        let alice_num_kernels = alice_db
            .fetch_mmr_node_count(MmrTree::Kernel, network_tip_height)
            .unwrap();
        let bob_num_kernels = bob_db
            .fetch_mmr_node_count(MmrTree::Kernel, network_tip_height)
            .unwrap();
        assert_eq!(alice_num_kernels, bob_num_kernels);
        let alice_kernel_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, alice_num_kernels, Some(network_tip_height))
            .unwrap();
        let bob_kernel_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, bob_num_kernels, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_kernel_nodes, bob_kernel_nodes);
        // Check Kernels
        let alice_kernel_hashes = alice_kernel_nodes.iter().map(|n| n.0.clone()).collect::<Vec<_>>();
        let bob_kernels_hashes = bob_kernel_nodes.iter().map(|n| n.0.clone()).collect::<Vec<_>>();
        let alice_kernels = alice_db.fetch_kernels(alice_kernel_hashes).unwrap();
        let bob_kernels = bob_db.fetch_kernels(bob_kernels_hashes).unwrap();
        assert_eq!(alice_kernels, bob_kernels);
        // Check UTXO MMR nodes
        let alice_num_utxos = alice_db
            .fetch_mmr_node_count(MmrTree::Utxo, network_tip_height)
            .unwrap();
        let bob_num_utxos = bob_db.fetch_mmr_node_count(MmrTree::Utxo, network_tip_height).unwrap();
        assert_eq!(alice_num_utxos, bob_num_utxos);
        let alice_utxo_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::Utxo, 0, alice_num_utxos, Some(network_tip_height))
            .unwrap();
        let bob_utxo_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::Utxo, 0, bob_num_utxos, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_utxo_nodes, bob_utxo_nodes);
        // Check RangeProof MMR nodes
        let alice_num_rps = alice_db
            .fetch_mmr_node_count(MmrTree::RangeProof, network_tip_height)
            .unwrap();
        let bob_num_rps = bob_db
            .fetch_mmr_node_count(MmrTree::RangeProof, network_tip_height)
            .unwrap();
        assert_eq!(alice_num_rps, bob_num_rps);
        let alice_rps_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::RangeProof, 0, alice_num_rps, Some(network_tip_height))
            .unwrap();
        let bob_rps_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::RangeProof, 0, bob_num_rps, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_rps_nodes, bob_rps_nodes);
        // Check UTXOs
        let mut alice_utxos = Vec::<TransactionOutput>::new();
        for (hash, deleted) in alice_utxo_nodes {
            if !deleted {
                alice_utxos.push(alice_db.fetch_utxo(hash).unwrap());
            }
        }
        let mut bob_utxos = Vec::<TransactionOutput>::new();
        for (hash, deleted) in bob_utxo_nodes {
            if !deleted {
                bob_utxos.push(bob_db.fetch_utxo(hash).unwrap());
            }
        }
        assert_eq!(alice_utxos, bob_utxos);

        alice_node.comms.shutdown().await;
        bob_node.comms.shutdown().await;
    });
}
#[test]
fn test_pruned_mode_sync_with_invalid_state() {
    let mut runtime = Runtime::new().unwrap();
    let factories = CryptoFactories::default();
    let temp_dir = tempdir().unwrap();
    let network = Network::LocalNet;
    let consensus_constants = ConsensusConstantsBuilder::new(network)
        .with_emission_amounts(100_000_000.into(), 0.999, 100.into())
        .build();
    let (block0, output) = create_genesis_block(&factories, &consensus_constants);
    let consensus_manager = ConsensusManagerBuilder::new(network)
        .with_consensus_constants(consensus_constants)
        .with_block(block0.clone())
        .build();
    let blockchain_db_config = BlockchainDatabaseConfig {
        orphan_storage_capacity: 3,
        pruning_horizon: 4,
        pruned_mode_cleanup_interval: 2,
    };
    let (alice_node, mut bob_node, consensus_manager) = create_network_with_2_base_nodes_with_config(
        &mut runtime,
        blockchain_db_config,
        BaseNodeServiceConfig::default(),
        MmrCacheConfig::default(),
        MempoolServiceConfig::default(),
        LivenessConfig::default(),
        consensus_manager,
        temp_dir.path().to_str().unwrap(),
    );
    let mut horizon_sync_config = HorizonSyncConfig::default();
    horizon_sync_config.horizon_sync_height_offset = 0;
    let state_machine_config = BaseNodeStateMachineConfig {
        block_sync_config: BlockSyncConfig::default(),
        horizon_sync_config,
        sync_peer_config: SyncPeerConfig::default(),
    };
    let shutdown = Shutdown::new();
    let mut alice_state_machine = BaseNodeStateMachine::new(
        &alice_node.blockchain_db,
        &alice_node.local_nci,
        &alice_node.outbound_nci,
        alice_node.comms.peer_manager(),
        alice_node.comms.connectivity(),
        alice_node.chain_metadata_handle.get_event_stream(),
        state_machine_config,
        SyncValidators::new(
            MockValidator::new(true),
            ChainBalanceValidator::new(
                alice_node.blockchain_db.clone(),
                consensus_manager.clone(),
                factories.clone(),
            ),
        ),
        shutdown.to_signal(),
    );

    runtime.block_on(async {
        let mut blocks = vec![block0];
        let mut outputs = vec![vec![output]];
        // Block1
        let txs = vec![txn_schema!(
            from: vec![outputs[0][0].clone()],
            to: vec![10 * T, 10 * T, 10 * T, 10 * T]
        )];
        generate_new_block(
            &mut bob_node.blockchain_db,
            &mut blocks,
            &mut outputs,
            txs,
            &consensus_manager.consensus_constants(),
        )
        .unwrap();
        // Block2
        let txs = vec![txn_schema!(from: vec![outputs[1][3].clone()], to: vec![6 * T])];
        generate_new_block(
            &mut bob_node.blockchain_db,
            &mut blocks,
            &mut outputs,
            txs,
            &consensus_manager.consensus_constants(),
        )
        .unwrap();
        // Block3
        let v = consensus_manager.emission_schedule().block_reward(3) + 1 * uT;
        let (_, _, output) = create_coinbase(&factories, v, 1);
        let txs = vec![];
        generate_new_block(
            &mut bob_node.blockchain_db,
            &mut blocks,
            &mut vec![vec![output]],
            txs,
            &consensus_manager.consensus_constants(),
        )
        .unwrap();

        // Both nodes are running in pruned mode and can not use block sync to synchronize state. Sync horizon state
        // from genesis block to horizon_sync_height and then block sync to the tip.
        let alice_db = &alice_node.blockchain_db;
        let bob_db = &bob_node.blockchain_db;
        let network_tip = bob_db.get_metadata().unwrap();
        let mut sync_peers = vec![bob_node.node_identity.node_id().clone()];
        let state_event = BestChainMetadataBlockSyncInfo {}
            .next_event(&mut alice_state_machine, &network_tip, &mut sync_peers)
            .await;
        assert_eq!(state_event, StateEvent::BlocksSynchronized);
        let alice_metadata = alice_db.get_metadata().unwrap();
        assert_eq!(alice_metadata, network_tip);

        // Check headers
        let network_tip_height = network_tip.height_of_longest_chain.unwrap_or(0);
        let block_nums = (0..=network_tip_height).collect::<Vec<u64>>();
        let alice_headers = alice_db.fetch_headers(block_nums.clone()).unwrap();
        let bob_headers = bob_db.fetch_headers(block_nums).unwrap();
        assert_eq!(alice_headers, bob_headers);
        // Check Kernel MMR nodes
        let alice_num_kernels = alice_db
            .fetch_mmr_node_count(MmrTree::Kernel, network_tip_height)
            .unwrap();
        let bob_num_kernels = bob_db
            .fetch_mmr_node_count(MmrTree::Kernel, network_tip_height)
            .unwrap();
        assert_eq!(alice_num_kernels, bob_num_kernels);
        let alice_kernel_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, alice_num_kernels, Some(network_tip_height))
            .unwrap();
        let bob_kernel_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::Kernel, 0, bob_num_kernels, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_kernel_nodes, bob_kernel_nodes);
        // Check Kernels
        let alice_kernel_hashes = alice_kernel_nodes.iter().map(|n| n.0.clone()).collect::<Vec<_>>();
        let bob_kernels_hashes = bob_kernel_nodes.iter().map(|n| n.0.clone()).collect::<Vec<_>>();
        let alice_kernels = alice_db.fetch_kernels(alice_kernel_hashes).unwrap();
        let bob_kernels = bob_db.fetch_kernels(bob_kernels_hashes).unwrap();
        assert_eq!(alice_kernels, bob_kernels);
        // Check UTXO MMR nodes
        let alice_num_utxos = alice_db
            .fetch_mmr_node_count(MmrTree::Utxo, network_tip_height)
            .unwrap();
        let bob_num_utxos = bob_db.fetch_mmr_node_count(MmrTree::Utxo, network_tip_height).unwrap();
        assert_eq!(alice_num_utxos, bob_num_utxos);
        let alice_utxo_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::Utxo, 0, alice_num_utxos, Some(network_tip_height))
            .unwrap();
        let bob_utxo_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::Utxo, 0, bob_num_utxos, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_utxo_nodes, bob_utxo_nodes);
        // Check RangeProof MMR nodes
        let alice_num_rps = alice_db
            .fetch_mmr_node_count(MmrTree::RangeProof, network_tip_height)
            .unwrap();
        let bob_num_rps = bob_db
            .fetch_mmr_node_count(MmrTree::RangeProof, network_tip_height)
            .unwrap();
        assert_eq!(alice_num_rps, bob_num_rps);
        let alice_rps_nodes = alice_db
            .fetch_mmr_nodes(MmrTree::RangeProof, 0, alice_num_rps, Some(network_tip_height))
            .unwrap();
        let bob_rps_nodes = bob_db
            .fetch_mmr_nodes(MmrTree::RangeProof, 0, bob_num_rps, Some(network_tip_height))
            .unwrap();
        assert_eq!(alice_rps_nodes, bob_rps_nodes);
        // Check UTXOs
        let mut alice_utxos = Vec::<TransactionOutput>::new();
        for (hash, deleted) in alice_utxo_nodes {
            if !deleted {
                alice_utxos.push(alice_db.fetch_utxo(hash).unwrap());
            }
        }
        let mut bob_utxos = Vec::<TransactionOutput>::new();
        for (hash, deleted) in bob_utxo_nodes {
            if !deleted {
                bob_utxos.push(bob_db.fetch_utxo(hash).unwrap());
            }
        }
        assert_eq!(alice_utxos, bob_utxos);

        alice_node.comms.shutdown().await;
        bob_node.comms.shutdown().await;
    });
}
