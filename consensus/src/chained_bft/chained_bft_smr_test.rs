// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use libra_types::account_address::ADDRESS_LENGTH;

use crate::{
    chained_bft::{
        block_storage::BlockReader,
        chained_bft_smr::ChainedBftSMR,
        network_interface::{ConsensusMsg, ConsensusNetworkEvents, ConsensusNetworkSender},
        network_tests::NetworkPlayground,
        test_utils::{
            consensus_runtime, MockSharedStorage, MockStateComputer, MockStorage,
            MockTransactionManager, TestPayload,
        },
    },
    consensus_provider::ConsensusProvider,
};

use channel::{self, libra_channel, message_queues::QueueStyle};
use consensus_types::vote_msg::VoteMsg;
use futures::{channel::mpsc, executor::block_on, stream::StreamExt};
use libra_config::{
    config::{
        ConsensusConfig,
        ConsensusProposerType::{self, FixedProposer, MultipleOrderedProposers, RotatingProposer},
        NodeConfig, SafetyRulesConfig,

// Bano: Check the imports below
use channel;
use consensus_types::{
    proposal_msg::{ProposalMsg, ProposalUncheckedSignatures},
    vote_msg::VoteMsg,
};
use futures::{channel::mpsc, executor::block_on, prelude::*};
use libra_config::config::{
    ConsensusProposerType::{self, FixedProposer, MultipleOrderedProposers, RotatingProposer},
    {SafetyRulesBackend, SafetyRulesConfig},
};
use libra_crypto::hash::CryptoHash;
use libra_types::account_address::AccountAddress;
use libra_types::{
    crypto_proxies::{
        random_validator_verifier, LedgerInfoWithSignatures, ValidatorSigner, ValidatorVerifier,
// end

    },
    generator::{self, ValidatorSwarm},
};

use libra_crypto::{hash::CryptoHash, HashValue};
use libra_mempool::mocks::MockSharedMempool;
use libra_types::crypto_proxies::{LedgerInfoWithSignatures, ValidatorSet, ValidatorVerifier};
use network::peer_manager::conn_status_channel;
use std::{num::NonZeroUsize, sync::Arc};

// Bano: Check if these are needed
use safety_rules::OnDiskStorage;
use std::{convert::TryFrom, path::PathBuf, sync::Arc, time::Duration};
use tempfile::NamedTempFile;
use tokio::runtime;

use libra_config::config::ConsensusProposerType::RoundProposers;
use libra_crypto::hash::HashValue;
use libra_logger::prelude::*;
use std::collections::HashMap;

use consensus_types::{common::Round, executed_block::ExecutedBlock};

use itertools::enumerate;
use itertools::Itertools;
use permutator::{Combination, XPermutationIterator};

use permutator::copy::Permutation;
use rand::Rng;
use std::{thread, time};

/// Auxiliary struct that is preparing SMR for the test
struct SMRNode {
    config: NodeConfig,
    smr_id: usize,
    smr: ChainedBftSMR<TestPayload>,
    commit_cb_receiver: mpsc::UnboundedReceiver<LedgerInfoWithSignatures>,
    storage: Arc<MockStorage<TestPayload>>,
    state_sync: mpsc::UnboundedReceiver<Vec<usize>>,
    shared_mempool: MockSharedMempool,
}

impl SMRNode {
    fn start(
        playground: &mut NetworkPlayground,
        config: NodeConfig,
        smr_id: usize,
        storage: Arc<MockStorage<TestPayload>>,
        executor_with_reconfig: Option<ValidatorSet>,
    ) -> Self {
        let author = config.validator_network.as_ref().unwrap().peer_id;

        let (network_reqs_tx, network_reqs_rx) =
            libra_channel::new(QueueStyle::FIFO, NonZeroUsize::new(8).unwrap(), None);
        let (consensus_tx, consensus_rx) =
            libra_channel::new(QueueStyle::FIFO, NonZeroUsize::new(8).unwrap(), None);
        let (conn_mgr_reqs_tx, conn_mgr_reqs_rx) = channel::new_test(8);
        let (_, conn_status_rx) = conn_status_channel::new();
        let network_sender = ConsensusNetworkSender::new(network_reqs_tx, conn_mgr_reqs_tx);
        let network_events = ConsensusNetworkEvents::new(consensus_rx, conn_status_rx);
        playground.add_node(author, consensus_tx, network_reqs_rx, conn_mgr_reqs_rx);
        let (state_sync_client, state_sync) = mpsc::unbounded();
        let (commit_cb_sender, commit_cb_receiver) = mpsc::unbounded::<LedgerInfoWithSignatures>();
        let shared_mempool = MockSharedMempool::new(None);
        let consensus_to_mempool_sender = shared_mempool.consensus_sender.clone();

        let mut smr = ChainedBftSMR::new(
            network_sender,
            network_events,
            &mut config.clone(),
            Arc::new(MockStateComputer::new(
                state_sync_client,
                commit_cb_sender,
                Arc::clone(&storage),
                executor_with_reconfig,
            )),
            storage.clone(),
            Box::new(MockTransactionManager::new(Some(
                consensus_to_mempool_sender,
            ))),
        );

        smr.start().expect("Failed to start SMR!");
        Self {
            config,
            smr_id,
            smr,
            commit_cb_receiver,
            storage,
            state_sync,
            shared_mempool,
        }
    }

    fn restart_with_empty_shared_storage(mut self, playground: &mut NetworkPlayground) -> Self {
        let validator_set = self.storage.shared_storage.validator_set.clone();
        let shared_storage = Arc::new(MockSharedStorage::new(validator_set));
        self.storage = Arc::new(MockStorage::new_with_ledger_info(
            shared_storage,
            self.storage.get_ledger_info(),
        ));
        self.restart(playground)
    }

    fn restart(mut self, playground: &mut NetworkPlayground) -> Self {
        self.smr.stop();
        Self::start(
            playground,
            self.config,
            self.smr_id + 10,
            self.storage,
            None,
        )
    }

    fn start_num_nodes(
        num_nodes: usize,
        playground: &mut NetworkPlayground,
        proposer_type: ConsensusProposerType,
        executor_with_reconfig: bool,
    ) -> Vec<Self> {

        let ValidatorSwarm {
            mut nodes,
            validator_set,
        } = generator::validator_swarm_for_testing(num_nodes);

        let executor_validator_set = if executor_with_reconfig {
            Some(validator_set.clone())
        } else {
            None
        };
        let validators = Arc::new(validator_verifier);
        let mut nodes = vec![];

        for smr_id in 0..num_nodes {
            let (storage, initial_data) = MockStorage::start_for_testing();
            let safety_rules_path = NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
            OnDiskStorage::default_storage(safety_rules_path.clone());
            nodes.push(Self::start(
                playground,
                signers.remove(0),
                Arc::clone(&validators),
                smr_id,
                storage,
                initial_data,
                proposer_type,
                validator_set.clone(),
                safety_rules_path,
            ));
        }

        nodes
    }

    // Starts num_nodes and target_nodes.len() twins
    // Returns:
    // (1) A vector of nodes created, and
    // (2) A mapping between node index and corresponding twin index
    //     (twins are included in the nodes vector)
    fn start_num_nodes_with_twins(
        num_nodes: usize,
        // Indices of nodes (i.e. target nodes) for which we will create twins
        target_nodes: &Vec<usize>,
        quorum_voting_power: u64,
        playground: &mut NetworkPlayground,
        proposer_type: ConsensusProposerType,
        executor_with_reconfig: bool,
        // Leaders per round
        twins_round_proposers_idx: HashMap<Round, Vec<usize>>,
        // The indices at which twins will be created
        node_to_twin: &HashMap<usize, usize>,
    ) -> Vec<Self> {
        // ======================================
        // Generate 'ValidatorSigner' and 'ValidatorVerifier'
        // ======================================

        // ValidatorSigner --> is a struct that has node's
        // account address, public and private keys. "Signers" is a
        // vector of "ValidatorSigner" for each node.
        // "Signers" is passed to "SMRNode::start()" to start SMR nodes
        // with certain account addresses and public and private keys
        //
        // ValidatorVerifier --> is a struct that includes total voting
        // power, quorum voting power, and a hashmap "address_to_validator_info"
        // that maps account addresses of all nodes to public key and
        // voting power (wrapped in a struct "ValidatorInfo").
        // "ValidatorVerifier" supports validation of signatures for
        // known authors with individual voting powers. This struct
        // can be used for all signature verification operations
        // including block and network signature verification.
        // "ValidatorVerifier" is also used by consensus to send
        // messages to nodes, (see "consensus/src/chained_bft/network.rs")
        let (mut signers, mut validator_verifier) =
            random_validator_verifier(num_nodes, Some(quorum_voting_power), true);

        // ======================================
        // Add twins to 'ValidatorSigner' and 'ValidatorVerifier'
        // ======================================

        // Vector of twins
        let mut twins: Vec<ValidatorSigner> = vec![];

        // Starting index for twin account addresses (this will appear in
        // logs). We choose 240 (hex: f0), so twins will appear as "f0"
        // onwards in logs
        let mut twin_account_index = 240;

        for ref_target_node in target_nodes {
            let target_node = *ref_target_node;

            // -----------------------------------------
            // Clone the target node and add to vector of twins
            // -----------------------------------------

            twins.push(signers[target_node].clone());

            // Index of the newly added twin
            let twins_top = twins.len() - 1;

            // The twin should be equal to the target node, at this point
            assert_eq!(twins[twins_top], signers[target_node]);

            // -----------------------------------------
            // Change the twin's account address to "twin_account_address"
            // -----------------------------------------

            // Explanation: At the consensus layer routing decisions are
            // made based on account addresses (see relevant functions in
            // "consensus/src/chained_bft/network.rs" such as "send_vote"
            // and "broadcast" -- they all use author, which is the same as an
            // account address, to identify the destination node of a message)

            let mut twin_address = [0; ADDRESS_LENGTH];
            // Usually account address is hash of node's public key, but for
            // testing we generate account addresses that are more readable.
            // So "twin_address" below will appear in the first byte of the
            // "AccountAddress" generated by "AccountAddress::try_from"
            twin_address[0] = twin_account_index;
            twin_account_index += 1;

            let twin_account_address = AccountAddress::try_from(&twin_address[..]).unwrap();
            ValidatorSigner::set_account_address(&mut twins[twins_top], twin_account_address);

            // The twin should be NOT equal to the target node, at this point
            assert_ne!(twins[twins_top], signers[target_node]);

            // -----------------------------------------
            // Update "signers", so the newly created twin is included
            // -----------------------------------------

            signers.push(twins[twins_top].clone());

            // -----------------------------------------
            // Also update "validator_verifier", i.e. add twin to its
            // "address_to_validator_info". Twin will have its own
            // 'twin_account_address', but keys of 'target_account_address'
            // -----------------------------------------

            let target_account_address = signers[target_node].author();
            ValidatorVerifier::add_to_address_to_validator_info(
                &mut validator_verifier,
                twin_account_address.clone(),
                &target_account_address,
            );
        }

        // ======================================
        // Set leaders per round
        // ======================================

        // A map that tells who is the proposer(s) per round
        // Note: If no proposer is defined for a round, we default to the first node
        let mut twins_round_proposers: HashMap<Round, Vec<AccountAddress>> = HashMap::new();

        for (round, vec_idx) in twins_round_proposers_idx.iter() {
            let mut idx_to_authors = Vec::new();
            for idx in vec_idx.iter() {
                idx_to_authors.push(signers[idx.to_owned()].author());
            }

            /*
            println!("======================");
            println!("Round: {0}, Leaders: [{1:?}]", round, idx_to_authors);
            println!("======================");
            */

            // Leader for round 1: node0 and twin_node0
            twins_round_proposers.insert(round.to_owned(), idx_to_authors);
        }

        ValidatorVerifier::set_round_to_proposers(&mut validator_verifier, twins_round_proposers);

        // =======================
        // Set validators
        // =======================

        let validator_set = if executor_with_reconfig {
            Some((&validator_verifier).into())
        } else {
            None
        };

        let validators = Arc::new(validator_verifier);

        // ====================
        // Start nodes
        // ====================

        let mut nodes = vec![];

        // Some tests make assumptions about the ordering of configs in relation
        // to the FixedProposer which should be the first proposer in lexical order.
        nodes.sort_by(|a, b| {
            let a_auth = a.validator_network.as_ref().unwrap().peer_id;
            let b_auth = b.validator_network.as_ref().unwrap().peer_id;
            a_auth.cmp(&b_auth)
        });

        let mut smr_nodes = vec![];
        for (smr_id, config) in nodes.iter().enumerate() {
            let mut node_config = config.clone();
            node_config.consensus.proposer_type = proposer_type;
            // Use in memory storage for testing
            node_config.consensus.safety_rules = SafetyRulesConfig::default();

            let (_, storage) = MockStorage::start_for_testing(validator_set.clone());
            smr_nodes.push(Self::start(
                playground,
                node_config,
                smr_id,
                storage,
                executor_validator_set.clone(),
            ));
        }

        // =================
        // Start twins
        // =================

        // Adding twins
        let count_twins = twins.len();

        for each in 0..count_twins {
            let (storage, initial_data) = MockStorage::start_for_testing();
            let safety_rules_path = NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
            OnDiskStorage::default_storage(safety_rules_path.clone());
            nodes.push(Self::start(
                playground,
                // We always remove at 0 because removing results in the
                // element being removed, and all the other elements being
                // moved to the 'left' (hence what used to be the element
                // at index 1 will come to index 0, after the original
                // element at index 0 is removed). So eventually on removing
                // the last element we're left with an empty vector.
                twins.remove(0),
                Arc::clone(&validators),
                num_nodes + each,
                storage,
                initial_data,
                proposer_type,
                validator_set.clone(),
                safety_rules_path,
            ));
        }

        nodes
    }
}


// =======================
// Twins tests
// =======================
>>>>>>> 2141ac72... Tidy up

#[test]
/// This test checks that when a node and its twin are both leaders for a round,
/// only one of the two proposals gets a QC
///
/// Setup:
///
/// Network of 2 nodes (n0, n1), and their twins (twin0, twin1)
/// Quorum voting power: 3
///
/// Test:
///
/// Let n0 and twin0 propose a block in round1
/// Pull out enough votes so a QC can be formed
/// Check that the QC of n0 and twin0 matches
///
fn twins_QC_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    let num_nodes = 2;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
    // Similarly we can access entries for twins in other collections like
    // 'signers' and 'validator_verifier' by using this map
    let mut node_to_twin: HashMap<usize, usize> = HashMap::new();
    for (each, target) in target_nodes.iter().enumerate() {
        let twin_index = num_nodes + each;
        node_to_twin.insert(*target, twin_index);
        debug!(
            "[Twins] Will create Twin for node {0} at index {1}",
            *target, twin_index
        );
    }

    // Specify round leaders here
    // Will default to the first node, if no leader specified for given round
    let mut twins_round_proposers_idx: HashMap<Round, Vec<usize>> = HashMap::new();
    // Leaders are n0 and twin1 for round 1..4
    for i in 1..5 {
        twins_round_proposers_idx.insert(i, vec![0, node_to_twin.get(&0).unwrap().to_owned()]);
    }

    // Start a network with 2 nodes and 2 twins for those nodes
    let nodes = SMRNode::start_num_nodes_with_twins(
        num_nodes,
        &mut target_nodes,
        3,
        &mut playground,
        RoundProposers,
        false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    let n0 = nodes[0].signer.author();
    let n1 = nodes[1].signer.author();
    let twin0 = nodes[node_to_twin.get(&0).unwrap().to_owned()]
        .signer
        .author();
    let twin1 = nodes[node_to_twin.get(&1).unwrap().to_owned()]
        .signer
        .author();

    block_on(async move {
        // Two proposals (by n0 and twin0)
        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only)
            .await;

        // Pull out enough votes so a QC can be formed
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(6, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();

        // Twin leader's HQC
        let hqc_twin0 = nodes[3].smr.block_store().unwrap().highest_quorum_cert();

        // Any other nodes' HQC
        let hqc_node0 = nodes[0].smr.block_store().unwrap().highest_quorum_cert();

        // Proposal from node0 and twin_node0 are going to race
        // but only one of them will form QC because quorum voting
        // power is set to 3, so ultimately they'll have the same QC
        assert_eq!(hqc_twin0, hqc_node0);
    });
}


#[test]
/// Checks that DropConfigRound (filter rules per round) works for twins
///
/// Setup:
/// Network with 2 nodes (n0, n1) and their twins (twin0, twin1)
/// Leader for each round is n0
/// Quorum voting power: 2
///
/// Test:
/// Round 1: Drop messages between n0 and n1. Check that the proposed
///          block is in everyone else's block store except n1
///
/// Round2: There is no filtering rule. Check that the proposed
///         block is in everyone else's block store
///
/// Round3: Drop messages between n0 and (n1, twin0, twin1). Check that
///         the proposed block is in no one's block store except n0
///
fn twins_drop_config_round_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    let num_nodes = 2;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
    // Similarly we can access entries for twins in other collections like
    // 'signers' and 'validator_verifier' by using this map
    let mut node_to_twin: HashMap<usize, usize> = HashMap::new();
    for (each, target) in target_nodes.iter().enumerate() {
        let twin_index = num_nodes + each;
        node_to_twin.insert(*target, twin_index);
        debug!(
            "[Twins] Will create Twin for node {0} at index {1}",
            *target, twin_index
        );
    }

    // Specify round leaders here
    // Will default to the first node, if no leader specified for given round
    let mut twins_round_proposers_idx: HashMap<Round, Vec<usize>> = HashMap::new();

    let nodes = SMRNode::start_num_nodes_with_twins(
        num_nodes,
        &mut target_nodes,
        2,
        &mut playground,
        RoundProposers,
        false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    let n0 = nodes[0].signer.author();
    let n1 = nodes[1].signer.author();
    let twin0 = nodes[node_to_twin.get(&0).unwrap().to_owned()]
        .signer
        .author();
    let twin1 = nodes[node_to_twin.get(&1).unwrap().to_owned()]
        .signer
        .author();

    // Filters

    playground.drop_message_for_round(n0, n1, 1);
    // No filtering rule in round 2
    playground.drop_message_for_round(n0, twin0, 3);
    playground.drop_message_for_round(n0, twin1, 3);

    block_on(async move {
        // ===== Round 1 ======

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 2 votes (from the twins)
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(2, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store
        // of n0, twin0 and twin1
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        assert!(nodes[2]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        assert!(nodes[3]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        // n1 does not have the block id because it did not receive the proposal from n0
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());

        // ========= Round 2 =========

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 3 votes (from n1, and the twins)
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(3, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store
        // of n0, n1, twin0 and twin1
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        assert!(nodes[2]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        assert!(nodes[3]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        // ===== Round 3 ======

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 1 vote from n1
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(1, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();

        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is present in the block store of n0
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        // Verify that the proposed block id is present in the block store of n1
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        // twin0 should not have the block id because it did not receive the proposal from n0
        assert!(nodes[2]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());

        // twin1 should not have the block id because it did not receive the proposal from n0
        assert!(nodes[3]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
    });
}

#[test]
/// Checks that split_network works for twins
///
/// Setup:
/// Creates a network of 2 nodes (node0 and node1) and 2 twins (twin0 and twin1).
/// Quorum voting power is 2
/// Each round has node0 as the leader.
/// Creates a network partition between (node0, node1) and (twin0 and twin1)
///
/// Test: node0 sends a proposal, which reaches node0 and node1.
/// Check that the block is present in the store of node0 and node1,
/// and should not be present in the store of twin0 and twin1.
fn twins_split_network_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    let num_nodes = 2;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
    // Similarly we can access entries for twins in other collections like
    // 'signers' and 'validator_verifier' by using this map
    let mut node_to_twin: HashMap<usize, usize> = HashMap::new();
    for (each, target) in target_nodes.iter().enumerate() {
        let twin_index = num_nodes + each;
        node_to_twin.insert(*target, twin_index);
        debug!(
            "[Twins] Will create Twin for node {0} at index {1}",
            *target, twin_index
        );
    }

    // Specify round leaders here
    // Will default to the first node, if no leader specified for given round
    let mut twins_round_proposers_idx: HashMap<Round, Vec<usize>> = HashMap::new();

    let nodes = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ num_nodes,
        &mut target_nodes,
        /* quorum_voting_power */ 2,
        &mut playground,
        RoundProposers, //FixedProposer,
        /* executor_with_reconfig */ false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    block_on(async move {
        let n0 = &nodes[0].signer.author();
        let n1 = &nodes[1].signer.author();
        let twin0 = &nodes[2].signer.author();
        let twin1 = &nodes[3].signer.author();

        // Proposals from node0 will never reach twin0 and twin1
        playground.split_network(vec![n0, n1], vec![twin0, twin1]);
        // playground.stop_split_network(vec![n0, n1], vec![twin0, twin1]);

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 2 votes from node0 and node1
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(2, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        let _proposals = playground
            .wait_for_messages(3, NetworkPlayground::proposals_only)
            .await;

        // Verify that the proposed block id is indeed present in the block store
        // of node0 and node1
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        // The proposed block id should not be present in the block store
        // of twin0 and twin1
        assert!(nodes[2]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
        assert!(nodes[3]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
    });
}

#[test]
/// This test demonstrates safety violation with f+1 twins
///
/// Setup:
///
/// 4 honest nodes (n0, n1, n2, n3), and 2 twins (twin0, twin1)
///
/// Leader: For each round n0 and its twin (twin0) are both leaders
///
/// Quorum voting power: 2
///
/// We split the network as follows:
///      partition 1: node 0, node 1, node 2
///      partition 2: twin 0, twin 1, node 3
///
/// Test:
///
/// The purpose of this test is to create a safety violation;
/// n2 commits the block proposed by node 0, and n3 commits
/// the block proposed by twin0.
///
/// Run the test:
/// cargo xtest -p consensus twins_safety_violation_test -- --nocapture
fn twins_safety_violation_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    let num_nodes = 4;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
    // Similarly we can access entries for twins in other collections like
    // 'signers' and 'validator_verifier' by using this map
    let mut node_to_twin: HashMap<usize, usize> = HashMap::new();
    for (each, target) in target_nodes.iter().enumerate() {
        let twin_index = num_nodes + each;
        node_to_twin.insert(*target, twin_index);
        debug!(
            "[Twins] Will create Twin for node {0} at index {1}",
            *target, twin_index
        );
    }

    // Specify round leaders here
    // Will default to the first node, if no leader specified for given round
    let mut twins_round_proposers_idx: HashMap<Round, Vec<usize>> = HashMap::new();

    // Make n0 and twin0 leaders for round 1..29
    for i in 1..30 {
        twins_round_proposers_idx.insert(i, vec![0, node_to_twin.get(&0).unwrap().to_owned()]);
    }

    let nodes = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ num_nodes,
        &mut target_nodes,
        /* quorum_voting_power */ 3,
        &mut playground,
        RoundProposers,
        /* executor_with_reconfig */ false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    // 4 honest nodes
    let n0 = nodes[0].signer.author();
    let n1 = nodes[1].signer.author();
    let n2 = nodes[2].signer.author();
    let n3 = nodes[3].signer.author();
    // twin of n0
    let twin0 = nodes[node_to_twin.get(&0).unwrap().to_owned()]
        .signer
        .author();
    // twin of n1
    let twin1 = nodes[node_to_twin.get(&1).unwrap().to_owned()]
        .signer
        .author();

    // Create static network partition
    playground.split_network(vec![&n0, &n1, &n2], vec![&twin0, &twin1, &n3]);

    block_on(async move {
        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only)
            .await;

        // Pull enough votes to get a commit on the first block)
        // The proposer's votes are implicit and do not go in the queue.
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(18, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();

        // =================
        // Check if the tree of commits across the two partitions matches
        // =================

        let mut all_branches: Vec<Vec<Arc<ExecutedBlock<TestPayload>>>> = Vec::new();

        for i in 0..nodes.len() {
            let branch_head = nodes[i]
                .smr
                .block_store()
                .unwrap()
                .highest_ledger_info()
                .commit_info()
                .id();

            let branch: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[i]
                .smr
                .block_store()
                .unwrap()
                .path_from_root(branch_head)
                .unwrap_or_else(Vec::new);

            all_branches.push(branch);
        }
        // Now check if the branches match at all heights
        assert!(!is_safe(all_branches));
    });
}

#[test]
/// This test is the same as 'twins_safety_violation_test' except
/// that we use scenario_executor, rather than doing it manually.
/// The test demonstrates safety violation with f+1 twins
///
/// Setup:
///
/// 4 honest nodes (n0, n1, n2, n3), and 2 twins (twin0, twin1)
///
/// Leader: For each round n0 and its twin (twin0) are both leaders
///
/// Quorum voting power: 2
///
/// We split the network as follows:
///      partition 1: node 0, node 1, node 2
///      partition 2: twin 0, twin 1, node 3
///
/// Test:
///
/// The purpose of this test is to create a safety violation;
/// n2 commits the block proposed by node 0, and n3 commits
/// the block proposed by twin0.
///
/// Run the test:
/// cargo xtest -p consensus twins_safety_violation_test -- --nocapture
fn twins_safety_violation_scenario_executor_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    let num_nodes = 4;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
    // Similarly we can access entries for twins in other collections like
    // 'signers' and 'validator_verifier' by using this map
    let mut node_to_twin: HashMap<usize, usize> = HashMap::new();
    for (each, target) in target_nodes.iter().enumerate() {
        let twin_index = num_nodes + each;
        node_to_twin.insert(*target, twin_index);
        debug!(
            "[Twins] Will create Twin for node {0} at index {1}",
            *target, twin_index
        );
    }

    // 4 honest nodes
    let n0 = 0;
    let n1 = 1;
    let n2 = 2;
    let n3 = 3;
    // twin of n0
    let twin0 = node_to_twin.get(&0).unwrap().to_owned();
    // twin of n1
    let twin1 = node_to_twin.get(&1).unwrap().to_owned();

    let quorum_voting_power = 3;

    // Specify round leaders here
    // Will default to the first node, if no leader specified for given round
    let mut twins_round_proposers_idx: HashMap<Round, Vec<usize>> = HashMap::new();

    // Make n0 and twin0 leaders for round 1..29
    for i in 1..30 {
        twins_round_proposers_idx.insert(i, vec![0, node_to_twin.get(&0).unwrap().to_owned()]);
    }

    // Create per round partitions

    let mut round_partitions_idx: HashMap<u64, Vec<Vec<usize>>> = HashMap::new();

    for round in 0..50 {
        round_partitions_idx.insert(
            /* round */ round,
            vec![vec![n0, n1, n2], vec![twin0, twin1, n3]],
        );
    }

    assert!(!execute_scenario(
        num_nodes,
        &target_nodes,
        &node_to_twin,
        round_partitions_idx,
        twins_round_proposers_idx,
        quorum_voting_power
    ));
}

fn create_partitions(
    playground: &mut NetworkPlayground,
    partitions: HashMap<u64, Vec<Vec<AccountAddress>>>,
) -> bool {
    playground.split_network_round(&partitions)
}

// Compares branches at each height to check if there's any conflict
fn is_safe(branches: Vec<Vec<Arc<ExecutedBlock<TestPayload>>>>) -> bool {
    compare_vectors(&branches)
}

// This function compares vectors (of possibly different lengths) at each index
//
// The vectors are equal if they match at each (available) index
//
// There is a conflict at index i if the value at index i is not uniform
//          (or absent) across all the vectors. Values at conflicting index
//          will be printed out

fn compare_vectors(vecs: &Vec<Vec<Arc<ExecutedBlock<TestPayload>>>>) -> bool {
    // how many vectors need to be compared
    let num_vecs = vecs.len();

    /*
    println!("Comparing {0} vectors", num_vecs);
    for (each, item) in vecs.iter().enumerate() {
        println!("====== Vector {0} =======", each);
        for (x, y) in item.iter().enumerate() {
            print!("Index{0}:{1}\t",x, y.id());
        }
        print!("\n");
    }
    */

    let (longest_idx, longest_len) = longest_vector(vecs);
    //println!("Longest vector is at idx {0} with len {1}: {2:?}", longest_idx, longest_len, vecs[longest_idx]);

    let mut is_conflict = false;

    // At each index position
    for i in 0..longest_len {
        let mut val: HashValue = HashValue::zero();

        let mut first_time = true;

        // in the vectors to be compared
        for vec in vecs.iter() {
            // Only compare if the index being compared exists
            // (because some vectors might be shorter than the longest)
            if i < vec.len() {
                // If this is the first time comparing
                if first_time {
                    val = vec[i].id();
                    first_time = false;
                }
                // If there's a different val at this index than the one
                // at the previous vector
                if !val.eq(&vec[i].id()) {
                    // then print the conflicting index at all vectors,
                    print_conflict(vecs, i);
                    is_conflict = true;
                    // and move on to the next index
                    break;
                }
            }
        }
    }

    // Return negation, because the function is called 'is_safe'
    !is_conflict
}

// This function takes a vector of vectors, and for each vector it prints
// out the values at the given conflicting index
fn print_conflict(vecs: &Vec<Vec<Arc<ExecutedBlock<TestPayload>>>>, index: usize) {
    println!("CONFLICT: Index {0} doesn't match, values are:", index);

    for vec in vecs.iter() {
        if index < vec.len() {
            println!("{0}", vec[index].id());
        } else {
            println!("NULL");
        }
    }
}

// This function takes a vector of vectors, and returns
// (1) the longest one, if the vectors are all of different lengths, OR
// (2) the first longest one, if multiple vectors are of that length
fn longest_vector(vecs: &Vec<Vec<Arc<ExecutedBlock<TestPayload>>>>) -> (usize, usize) {
    let mut max_len: usize = 0;
    let mut idx: usize = 0;

    for (i, vec) in vecs.iter().enumerate() {
        let vec_len = vec.len();

        if vec_len > max_len {
            max_len = vec_len;
            idx = i;
        }
    }

    (idx, max_len)
}

fn execute_scenario(
    num_nodes: usize,
    target_nodes: &Vec<usize>, // the nodes for which to create twins
    node_to_twin: &HashMap<usize, usize>,
    round_partitions_idx: HashMap<u64, Vec<Vec<usize>>>,
    twins_round_proposers_idx: HashMap<Round, Vec<usize>>,
    quorum_voting_power: u64,
) -> bool {
    //assert_eq!(partitions.len(), leaders.len());
    //let num_of_rounds = partitions.len().clone();

    let runtime = consensus_runtime();

    let mut playground = NetworkPlayground::new(runtime.executor());

    let nodes = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ num_nodes,
        &target_nodes,
        /* quorum_voting_power */ quorum_voting_power,
        &mut playground,
        RoundProposers,
        /* executor_with_reconfig */ false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    // Create partitions

    let mut round_partitions: HashMap<u64, Vec<Vec<AccountAddress>>> = HashMap::new();

    // The partitions have been provided in terms of node indices
    // Below we just transform those to AccountAddress, which is what
    // is expected  by 'create_partitions'
    for (round, partitions) in round_partitions_idx.iter() {
        let mut round_account_addrs: Vec<Vec<AccountAddress>> = Vec::new();

        for part in partitions.iter() {
            let mut account_addrs: Vec<AccountAddress> = Vec::new();

            for idx in part.iter() {
                account_addrs.push(nodes[idx.clone()].signer.author());
            }

            round_account_addrs.push(account_addrs);
        }

        round_partitions.insert(round.clone(), round_account_addrs);
    }

    // Create partitions
    create_partitions(&mut playground, round_partitions);
    // playground.print_drop_config_round();

    // Start sending messages

    let mut is_this_safe = true;

    block_on(async move {
        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only)
            .await;

        // Pull enough votes to get a commit on the first block)
        // The proposer's votes are implicit and do not go in the queue.
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(40, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();

        // =================
        // Check if the tree of commits across the two partitions matches
        // =================

        let mut all_branches: Vec<Vec<Arc<ExecutedBlock<TestPayload>>>> = Vec::new();

        for i in 0..nodes.len() {
            let branch_head = nodes[i]
                .smr
                .block_store()
                .unwrap()
                .highest_ledger_info()
                .commit_info()
                .id();

            let branch: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[i]
                .smr
                .block_store()
                .unwrap()
                .path_from_root(branch_head)
                .unwrap_or_else(Vec::new);

            all_branches.push(branch);
        }
        // Now check if the branches match at all heights
        is_this_safe = is_safe(all_branches);
    });

    is_this_safe
}

// A memory-inefficient implementation of all solutions of Stirling number
// of second kind (https://en.wikipedia.org/wiki/Stirling_numbers_of_the_second_kind).
// This will probably not work for n > 20.
fn stirling2(n: usize, k: usize) -> Vec<Vec<Vec<usize>>> {
    if k == 1 {
        return vec![vec![(0..n).collect()]];
    } else if n == k {
        let mut ret = vec![vec![]];
        for i in (0..n) {
            ret[0].push(vec![i])
        }
        return ret;
    } else {
        let mut s_n_1_k_1 = stirling2(n - 1, k - 1);
        for v in &mut s_n_1_k_1 {
            v.push(vec![n - 1]);
        }

        let mut k_s_n_1_k = Vec::new();
        let tmp = stirling2(n - 1, k);
        for i in (0..k) {
            k_s_n_1_k.extend(tmp.iter().cloned());
        }
        let size = k * tmp.len();
        for i in (0..size) {
            let j = i / tmp.len() as usize;
            k_s_n_1_k[i][j].push(n - 1);
        }

        s_n_1_k_1.extend(k_s_n_1_k.iter().cloned());
        return s_n_1_k_1;
    }
}

#[test]
/// cargo xtest -p consensus test_stirling2 -- --nocapture
fn test_stirling2() {
    let mut ret = stirling2(4, 3);
    assert_eq!(
        ret,
        vec![
            vec![vec![0, 1], vec![2], vec![3]],
            vec![vec![0, 2], vec![1], vec![3]],
            vec![vec![0], vec![1, 2], vec![3]],
            vec![vec![0, 3], vec![1], vec![2]],
            vec![vec![0], vec![1, 3], vec![2]],
            vec![vec![0], vec![1], vec![2, 3]],
        ]
    );

    ret = stirling2(5, 2);
    assert_eq!(
        ret,
        vec![
            vec![vec![0, 1, 2, 3], vec![4]],
            vec![vec![0, 1, 2, 4], vec![3]],
            vec![vec![0, 1, 3, 4], vec![2]],
            vec![vec![0, 2, 3, 4], vec![1]],
            vec![vec![0, 3, 4], vec![1, 2]],
            vec![vec![0, 1, 4], vec![2, 3]],
            vec![vec![0, 2, 4], vec![1, 3]],
            vec![vec![0, 4], vec![1, 2, 3]],
            vec![vec![0, 1, 2], vec![3, 4]],
            vec![vec![0, 1, 3], vec![2, 4]],
            vec![vec![0, 2, 3], vec![1, 4]],
            vec![vec![0, 3], vec![1, 2, 4]],
            vec![vec![0, 1], vec![2, 3, 4]],
            vec![vec![0, 2], vec![1, 3, 4]],
            vec![vec![0], vec![1, 2, 3, 4]],
        ]
    );

    ret = stirling2(10, 4);
    assert_eq!(ret.len(), 34105);

    ret = stirling2(7, 1);
    assert_eq!(ret.len(), 1);
}

// Filter the set of possible partitions by remove some that are unlikely
// to produce useful results.
fn filter_partitions(
    list_of_partitions: Vec<Vec<Vec<usize>>>,
    num_of_nodes: usize,
) -> Vec<Vec<Vec<usize>>> {
    // Find the index of bad and twin nodes.
    // By convention, the first f nodes are bad, and the last f are their twins.
    let f: usize = (num_of_nodes - 1) / 3;
    let bad_nodes: Vec<usize> = (0..f).collect();
    let twin_nodes: Vec<usize> = (num_of_nodes..num_of_nodes + f).collect();

    // Remove partitions.
    let mut filtered_list_of_partitions = Vec::new();
    for partitions in list_of_partitions {
        let mut to_remove = false;
        for partition in partitions.clone() {
            // Remove the partitions if the node and its twin are both
            // in the same partition.
            for (i, bad_node) in enumerate(bad_nodes.clone()) {
                let twin_node = twin_nodes[i];
                if partition.contains(&bad_node) && partition.contains(&twin_node) {
                    to_remove = true;
                }
            }
        }
        if !to_remove {
            filtered_list_of_partitions.push(partitions.clone());
        }
    }
    filtered_list_of_partitions
}

#[test]
/// cargo xtest -p consensus test_filter_partitions -- --nocapture
fn test_filter_partitions() {
    let num_of_nodes = 4;
    let f = 1;
    let list_of_partitions = stirling2(num_of_nodes + f, 2);
    let filtered_list_of_partitions = filter_partitions(list_of_partitions, num_of_nodes);
    assert_eq!(
        filtered_list_of_partitions,
        vec![
            vec![vec![0, 1, 2, 3], vec![4]],
            //vec![vec![0, 1, 2, 4], vec![3]],
            //vec![vec![0, 1, 3, 4], vec![2]],
            //vec![vec![0, 2, 3, 4], vec![1]],
            //vec![vec![0, 3, 4], vec![1, 2]],
            //vec![vec![0, 1, 4], vec![2, 3]],
            //vec![vec![0, 2, 4], vec![1, 3]],
            //vec![vec![0, 4], vec![1, 2, 3]],
            vec![vec![0, 1, 2], vec![3, 4]],
            vec![vec![0, 1, 3], vec![2, 4]],
            vec![vec![0, 2, 3], vec![1, 4]],
            vec![vec![0, 3], vec![1, 2, 4]],
            vec![vec![0, 1], vec![2, 3, 4]],
            vec![vec![0, 2], vec![1, 3, 4]],
            vec![vec![0], vec![1, 2, 3, 4]],
        ]
    );
}

fn filter_partitions_pick_n(list_of_partitions: &mut Vec<Vec<Vec<usize>>>, n: usize) {
    if n < list_of_partitions.len() {
        let mut rng = rand::thread_rng();
        let mut dice = rng.gen_range(0, list_of_partitions.len());
        let mut seen = Vec::new();
        for i in 0..n {
            list_of_partitions[i] = list_of_partitions[dice].clone();
            seen.push(dice);
            while seen.contains(&dice) {
                dice = rng.gen_range(0, list_of_partitions.len());
            }
        }
        list_of_partitions.truncate(n);
    }
}

#[test]
/// run:
/// cargo xtest -p consensus twins_test_safety_attack_generator -- --nocapture
fn twins_test_safety_attack_generator() {
    const NUM_OF_ROUNDS: usize = 3; // Play with this parameter
    const NUM_OF_NODES: usize = 10; // Play with this parameter
    const NUM_OF_PARTITIONS: usize = 2; // Play with this parameter

    let f = (NUM_OF_NODES - 1) / 3;
    let quorum_voting_power: u64 = (NUM_OF_NODES - f) as u64;

    // =============================================
    //
    // Generate indices of nodes. There are three kinds of nodes:
    // (1) Target nodes: Represented by 'f', these are the nodes for which
    //      we will create Twins, to emulate byzantine behavior.
    // (2) Honest nodes: These are the honest nodes
    // (3) Twin nodes: These are the twins of target nodes (see 1 above)
    //
    // Below, we generate indices using the ordering convention:
    //      target_nodes, honest_nodes, twin_nodes
    //  |----------------------------------------------------------------------|
    //  | 0 ... f-1 | f ... NUM_OF_NODES-1 | NUM_OF_NODES ... NUM_OF_NODES+f-1 |
    //  |++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++|
    //  | target_nodes |    honest_nodes      |            twin_nodes             |
    //  |----------------------------------------------------------------------|
    //
    // =============================================

    let mut nodes: Vec<usize> = Vec::new();

    // First fill the bad nodes. By convention, the first nodes are bad.
    let mut target_nodes: Vec<usize> = (0..f).collect();
    nodes.append(&mut target_nodes.clone());

    // Then, add the other (honest) nodes
    let mut honest_nodes: Vec<usize> = (f..NUM_OF_NODES).collect();
    nodes.append(&mut honest_nodes.clone());

    // Finally, add the twins of the target nodes; those created at last by
    // the function 'start_num_nodes_with_twins'.
    let mut twin_nodes: Vec<usize> = (NUM_OF_NODES..NUM_OF_NODES + f).collect();
    nodes.append(&mut twin_nodes.clone());

    // `node_to_twin` is required by `execute_scenario()` which we call
    //  at the end to execute generated scenarios.
    // `node_to_twin` maps `target_nodes` to twins indices in 'nodes'
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
    // Similarly we can access entries for twins in other collections like
    // 'signers' and 'validator_verifier' by using this map
    let mut node_to_twin: HashMap<usize, usize> = HashMap::new();

    for (each, target) in target_nodes.iter().enumerate() {
        let twin_index = NUM_OF_NODES + each;
        node_to_twin.insert(*target, twin_index);
    }

    // =============================================
    //
    // Find all possible ways in which N nodes can be partitioned into P partitions.
    // We call each possible way a partition scenario.
    //
    // E.g. for N={0,1,2} and P=2, possible partition scenarios are:
    // {    { {0,1}, {2} },
    //      { {0,2}, {1} },
    //      { {1,2}, {0} },
    //      { {2}, {0,1} }, etc.
    // }
    //
    // =============================================

    // This problems is known as "Stirling Number of the Second Kind".
    // "In combinatorics, the Stirling numbers of the second kind tell
    // us how many ways there are of dividing up a set of n objects
    // (all different, or at least all labeled) into k nonempty subsets."
    // https://www.statisticshowto.datasciencecentral.com/stirling-numbers-second-kind/
    //
    //
    // Note: Many sets of partitions will be useless for us, e.g. cases where the
    // target_node and its twin are in the same partition. We may want to prune some
    // of them from the list if the tests take too much time.

    let mut partition_scenarios = stirling2(nodes.len(), NUM_OF_PARTITIONS);
    println!(
        "There are {:?} ways to allocate {:?} nodes ({:?} honest nodes + {:?} node-twin pairs) into {:?} partitions.",
        partition_scenarios.len(), nodes.len(), NUM_OF_NODES-f, f, NUM_OF_PARTITIONS
    );
    let old_list_of_partition_length = partition_scenarios.len();

    // Filter the partitions that have both twins in the same partition.
    //partition_scenarios = filter_partitions(partition_scenarios, NUM_OF_NODES);

    // Choose only two partitions
    filter_partitions_pick_n(&mut partition_scenarios, 2);
    println!(
        "After filtering, we have {:?} partition scenarios (we filtered out {:?} scenarios).",
        partition_scenarios.len(),
        old_list_of_partition_length - partition_scenarios.len()
    );

    // =============================================
    //
    // Assign leaders to partition scenarios.
    //
    // We don't consider honest_nodes as leaders, only pairs of target_nodes and their twins.
    // Note: It would be nice to test scenarios where all nodes can be leaders,
    // but the problem would quickly become intractable.
    //
    // The set of leaders L looks like this:
    // { (target_node1, twin_node1), (target_node2, twin_node2), (target_node3, twin_node3), ..}
    //
    // =============================================

    // Find all combinations of leaders and partition scenarios.
    //
    // Recall the shape of list_of_partitions, e.g. for N={0,1,2} and P=2,
    //  possible partitions are:
    // {    { {0,1}, {2} },
    //      { {0,2}, {1} },
    //      { {1,2}, {0} },
    //      { {2}, {0,1} }, etc.
    // }
    //
    // The problem is as follows: for each partition scenario (e.g. { {0, 22, 1}, {2, 11} },
    // where 11 is the twin of 1 and 22 is the twin of 2), find combination of each pair
    // in the leader set L = { {2,22}, {1,11}} with the partition scenarios. Some possible
    // combinations are:
    // Notation: First vector is the leader pair, second vector is the partition scenario
    // {
    //  { {2,22}, {{0, 22, 1}, {2, 11}} },
    //  { {1,11}, {{0, 22, 1}, {2, 11}} }, } etc.
    //
    // Note: For cases where a leader is not part of the corresponding partition,
    //  e.g. {2, {0, 22, 1}}, the leader just ends up not being able to propose
    //  anything at all
    //

    // In `partition_scenarios_with_leaders` each element is a pair of leader and
    // partition scenario
    let mut partition_scenarios_with_leaders = Vec::new();

    // We only add target_nodes as leaders here, and the twin_node corresponding to target_nodes
    // will be added as a leader implicitly by the scenario executor
    for each_leader in &target_nodes {
        for each_scenario in &partition_scenarios {
            let pair = (each_leader, each_scenario.clone());
            partition_scenarios_with_leaders.push(pair);
        }
    }

    // Don't need this any more
    partition_scenarios.clear();
    println!(
        "After combining leaders with partition scenarios, we have {:?} scenario-leader combinations).",
        partition_scenarios_with_leaders
    );

    // =============================================
    // We now have all possible partition-leader scenarios. Next we
    // find how to arrange these scenarios across NUM_OF_ROUNDS rounds.
    // =============================================

    // Number of rounds should be less than scenarios, otherwise we need to repeat
    // the same scenarios for multiple rounds (which is not implemented).
    assert!(partition_scenarios_with_leaders.len() > NUM_OF_ROUNDS);

    let test_cases = partition_scenarios_with_leaders
        .iter()
        .permutations(NUM_OF_ROUNDS);

    // =============================================
    // Now we are ready to prepare and execute each scenario via the executor
    // =============================================

    let mut round = 1;
    for each_test in test_cases {
        let mut round_partitions_idx = HashMap::new();
        let mut twins_round_proposers_idx = HashMap::new();
        for round_scenario in each_test {
            let leader = round_scenario.0.clone();
            let scenario = round_scenario.1.clone();
            let mut leaders = Vec::new();
            leaders.push(leader.clone());
            // If a target node is leader, make its twin the leader too
            if target_nodes.contains(&leader) {
                let twin_node = node_to_twin.get(&leader).unwrap().clone();
                leaders.push(twin_node.clone());
            }
            twins_round_proposers_idx.insert(round, leaders);
            round_partitions_idx.insert(round, scenario);
            round += 1;
        }

        execute_scenario(
            NUM_OF_NODES,
            &target_nodes,
            &node_to_twin,
            round_partitions_idx,      // this changes for each test
            twins_round_proposers_idx, // this changes for each test
            quorum_voting_power
        );

        //thread::sleep(time::Duration::from_secs(1));
    }
}

// ===============================
// Regular (i.e. non-Twins) tests
// ===============================

fn verify_finality_proof(node: &SMRNode, ledger_info_with_sig: &LedgerInfoWithSignatures) {
    let validators = ValidatorVerifier::from(&node.storage.shared_storage.validator_set);
    let ledger_info_hash = ledger_info_with_sig.ledger_info().hash();
    for (author, signature) in ledger_info_with_sig.signatures() {
        assert_eq!(
            Ok(()),
            validators.verify_signature(*author, ledger_info_hash, &signature)
        );
    }
}

#[test]
/// Should receive a new proposal upon start
fn basic_start_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    let nodes = SMRNode::start_num_nodes(2, &mut playground, RotatingProposer, false);
    let genesis = nodes[0]
        .smr
        .block_store()
        .expect("No valid block store!")
        .root();
    block_on(async move {
        let msg = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only::<TestPayload>)
            .await;
        let first_proposal = match &msg[0].1 {
            ConsensusMsg::ProposalMsg(proposal) => proposal,
            _ => panic!("Unexpected message found"),
        };
        assert_eq!(first_proposal.proposal().parent_id(), genesis.id());
        assert_eq!(
            first_proposal
                .proposal()
                .quorum_cert()
                .certified_block()
                .id(),
            genesis.id()
        );
    });
}

#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the participants.
fn start_with_proposal_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    let nodes = SMRNode::start_num_nodes(2, &mut playground, RotatingProposer, false);

    block_on(async move {
        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only::<TestPayload>)
            .await;
        // Need to wait for 2 votes for the 2 replicas
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
            .await
            .into_iter()
            .map(|(_, msg)| match msg {
                ConsensusMsg::VoteMsg(vote_msg) => *vote_msg,
                _ => panic!("Unexpected message found"),
            })
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store.
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());
    });
}


fn basic_full_round(
    num_nodes: usize,
    quorum_voting_power: u64,
    proposer_type: ConsensusProposerType,
) {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    let _nodes = SMRNode::start_num_nodes(num_nodes, &mut playground, proposer_type, false);

    // In case we're using multi-proposer, every proposal and vote is sent to two participants.
    let num_messages_to_send = if proposer_type == MultipleOrderedProposers {
        2 * (num_nodes - 1)
    } else {
        num_nodes - 1
    };
    block_on(async move {
        let _broadcast_proposals_1 = playground
            .wait_for_messages(
                num_messages_to_send,
                NetworkPlayground::proposals_only::<TestPayload>,
            )
            .await;
        let _votes_1 = playground
            .wait_for_messages(
                num_messages_to_send,
                NetworkPlayground::votes_only::<TestPayload>,
            )
            .await;
        let broadcast_proposals_2 = playground
            .wait_for_messages(
                num_messages_to_send,
                NetworkPlayground::proposals_only::<TestPayload>,
            )
            .await;
        let msg = &broadcast_proposals_2;
        let next_proposal = match &msg[0].1 {
            ConsensusMsg::ProposalMsg(proposal) => proposal,
            _ => panic!("Unexpected message found"),
        };
        assert!(next_proposal.proposal().round() >= 2);
    });
}

#[test]
/// Upon startup, the first proposal is sent, voted by all the participants, QC is formed and
/// then the next proposal is sent.
fn basic_full_round_test() {
    basic_full_round(2, FixedProposer);
}

#[test]
/// Basic happy path with multiple proposers
fn happy_path_with_multi_proposer() {
    basic_full_round(2, MultipleOrderedProposers);
}

async fn basic_commit(
    playground: &mut NetworkPlayground,
    nodes: &mut Vec<SMRNode>,
    block_ids: &mut Vec<HashValue>,
) {
    let num_rounds = 10;

    for round in 0..num_rounds {
        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::exclude_timeout_msg::<TestPayload>)
            .await;

        // A proposal is carrying a QC that commits a block of round - 3.
        if round >= 3 {
            let block_id_to_commit = block_ids[round - 3];
            let commit_v1 = nodes[0].commit_cb_receiver.next().await.unwrap();
            let commit_v2 = nodes[1].commit_cb_receiver.next().await.unwrap();
            assert_eq!(
                commit_v1.ledger_info().consensus_block_id(),
                block_id_to_commit
            );
            verify_finality_proof(&nodes[0], &commit_v1);
            assert_eq!(
                commit_v2.ledger_info().consensus_block_id(),
                block_id_to_commit
            );
            verify_finality_proof(&nodes[1], &commit_v2);
        }

        // v1 and v2 send votes
        let votes = playground
            .wait_for_messages(1, NetworkPlayground::votes_only::<TestPayload>)
            .await;
        let vote_msg = match &votes[0].1 {
            ConsensusMsg::VoteMsg(vote_msg) => vote_msg,
            _ => panic!("Unexpected message found"),
        };
        block_ids.push(vote_msg.vote().vote_data().proposed().id());
    }

    assert!(
        nodes[0].smr.block_store().unwrap().root().round() >= 7,
        "round of node 0 is {}",
        nodes[0].smr.block_store().unwrap().root().round()
    );
    assert!(
        nodes[1].smr.block_store().unwrap().root().round() >= 7,
        "round of node 1 is {}",
        nodes[1].smr.block_store().unwrap().root().round()
    );

    // This message is for proposal with round 11 to delivery the QC, but not gather the QC
    // so after restart, proposer will propose round 11 again.
    playground
        .wait_for_messages(1, NetworkPlayground::exclude_timeout_msg::<TestPayload>)
        .await;
}

/// Verify the basic e2e flow: blocks are committed, txn manager is notified, block tree is
/// pruned, restart the node and we can still continue.
#[test]
fn basic_commit_and_restart() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    let mut nodes = SMRNode::start_num_nodes(2, &mut playground, RotatingProposer, false);
    let mut block_ids = vec![];

    block_on(basic_commit(&mut playground, &mut nodes, &mut block_ids));

    // create a new playground to avoid polling potential vote messages in previous one.
    playground = NetworkPlayground::new(runtime.handle().clone());
    nodes = nodes
        .into_iter()
        .map(|node| node.restart(&mut playground))
        .collect();

    block_on(async {
        let mut round = 0;

        while round < 10 {
            // The loop is to ensure that we collect a network vote(enough for QC with 2 nodes) then
            // move the round forward because there's a race that node1 may or may not
            // reject round 11 depends on whether it voted for before restart.
            loop {
                let msg = playground
                    .wait_for_messages(1, NetworkPlayground::exclude_timeout_msg)
                    .await;
                if let ConsensusMsg::<TestPayload>::VoteMsg(_) = msg[0].1 {
                    round += 1;
                    break;
                }
            }
        }

        // Because of the race, we can't assert the commit reliably, instead we assert
        // both nodes commit to at least round 17.
        // We cannot reliable wait for the event of "commit & prune": the only thing that we know is
        // that after receiving the vote for round 20, the root should be at least height 16.
        assert!(
            nodes[0].smr.block_store().unwrap().root().round() >= 17,
            "round of node 0 is {}",
            nodes[0].smr.block_store().unwrap().root().round()
        );
        assert!(
            nodes[1].smr.block_store().unwrap().root().round() >= 17,
            "round of node 1 is {}",
            nodes[1].smr.block_store().unwrap().root().round()
        );
    });
}

/// Test restart with an empty shared storage to simulate empty consensus db
#[test]
fn basic_commit_and_restart_from_clean_storage() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    let mut nodes = SMRNode::start_num_nodes(2, &mut playground, RotatingProposer, false);
    let mut block_ids = vec![];

    block_on(basic_commit(&mut playground, &mut nodes, &mut block_ids));

    // create a new playground to avoid polling potential vote messages in previous one.
    playground = NetworkPlayground::new(runtime.handle().clone());
    nodes = nodes
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            if index == 0 {
                node.restart_with_empty_shared_storage(&mut playground)
            } else {
                node.restart(&mut playground)
            }
        })
        .collect();

    block_on(async {
        let mut round = 0;

        while round < 10 {
            // The loop is to ensure that we collect a network vote(enough for QC with 2 nodes) then
            // move the round forward because there's a race that node1 may or may not
            // reject round 11 depends on whether it voted for before restart.
            loop {
                let msg = playground
                    .wait_for_messages(1, NetworkPlayground::exclude_timeout_msg)
                    .await;
                if let ConsensusMsg::<TestPayload>::VoteMsg(_) = msg[0].1 {
                    round += 1;
                    break;
                }
            }
        }

        // Because of the race, we can't assert the commit reliably, instead we assert
        // both nodes commit to at least round 17.
        // We cannot reliable wait for the event of "commit & prune": the only thing that we know is
        // that after receiving the vote for round 20, the root should be at least height 16.
        // Since we are starting from empty storage for node 0, we can't really get block_store
        // for it. But testing node 1's round is sufficient because we only have two nodes and
        // if node 0 never starts up successfully we can't possible advance
        assert!(
            nodes[1].smr.block_store().unwrap().root().round() >= 17,
            "round of node 1 is {}",
            nodes[1].smr.block_store().unwrap().root().round()
        );
    });
}

#[test]
fn basic_block_retrieval() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    // This test depends on the fixed proposer on nodes[0]
    let mut nodes = SMRNode::start_num_nodes(4, &mut playground, FixedProposer, false);
    block_on(async move {
        let mut first_proposals = vec![];
        // First three proposals are delivered just to nodes[0..2].
        playground.drop_message_for(&nodes[0].smr.author(), nodes[3].smr.author());
        for _ in 0..2 {
            playground
                .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
                .await;
            let votes = playground
                .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
                .await;
            let vote_msg = match &votes[0].1 {
                ConsensusMsg::VoteMsg(vote_msg) => vote_msg,
                _ => panic!("Unexpected message found"),
            };
            let proposal_id = vote_msg.vote().vote_data().proposed().id();
            first_proposals.push(proposal_id);
        }
        // The next proposal is delivered to all: as a result nodes[2] should retrieve the missing
        // blocks from nodes[0] and vote for the 3th proposal.
        playground.stop_drop_message_for(&nodes[0].smr.author(), &nodes[3].smr.author());
        // Drop nodes[1]'s vote to ensure nodes[3] contribute to the quorum
        playground.drop_message_for(&nodes[0].smr.author(), nodes[1].smr.author());

        playground
            .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
            .await;
        playground
            .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
            .await;
        // The first two proposals should be present at nodes[3] via block retrieval
        for block_id in &first_proposals {
            assert!(nodes[3]
                .smr
                .block_store()
                .unwrap()
                .get_block(*block_id)
                .is_some());
        }

        // 4th proposal will get quorum and verify that nodes[3] commits the first proposal.
        playground
            .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
            .await;
        playground
            .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
            .await;
        if let Some(commit_v3) = nodes[3].commit_cb_receiver.next().await {
            assert_eq!(
                commit_v3.ledger_info().consensus_block_id(),
                first_proposals[0],
            );
        }
    });
}

#[test]
fn block_retrieval_with_timeout() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    let nodes = SMRNode::start_num_nodes(4, &mut playground, FixedProposer, false);
    block_on(async move {
        let mut first_proposals = vec![];
        // First three proposals are delivered just to nodes[0..2].
        playground.drop_message_for(&nodes[0].smr.author(), nodes[3].smr.author());
        for _ in 0..2 {
            playground
                .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
                .await;
            let votes = playground
                .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
                .await;
            let vote_msg = match &votes[0].1 {
                ConsensusMsg::VoteMsg(vote_msg) => vote_msg,
                _ => panic!("Unexpected message found"),
            };
            let proposal_id = vote_msg.vote().vote_data().proposed().id();
            first_proposals.push(proposal_id);
        }
        // stop proposals from nodes[0]
        playground.drop_message_for(&nodes[0].smr.author(), nodes[1].smr.author());
        playground.drop_message_for(&nodes[0].smr.author(), nodes[2].smr.author());

        // Wait until {1, 2, 3} timeout to {0 , 1, 2, 3} excluding self messages
        playground
            .wait_for_messages(3 * 3, NetworkPlayground::timeout_votes_only::<TestPayload>)
            .await;

        // the first two proposals should be present at nodes[3]
        for block_id in &first_proposals {
            assert!(nodes[2]
                .smr
                .block_store()
                .unwrap()
                .get_block(*block_id)
                .is_some());
        }
    });
}

#[test]
/// Verify that a node that is lagging behind can catch up by state sync some blocks
/// have been pruned by the others.
fn basic_state_sync() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    // This test depends on the fixed proposer on nodes[0]
    let mut nodes = SMRNode::start_num_nodes(4, &mut playground, FixedProposer, false);
    block_on(async move {
        let mut proposals = vec![];
        // The first ten proposals are delivered just to nodes[0..2], which should commit
        // the first seven blocks.
        playground.drop_message_for(&nodes[0].smr.author(), nodes[3].smr.author());
        for _ in 0..10 {
            playground
                .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
                .await;
            let votes = playground
                .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
                .await;
            let vote_msg = match &votes[0].1 {
                ConsensusMsg::VoteMsg(vote_msg) => vote_msg,
                _ => panic!("Unexpected message found"),
            };
            let proposal_id = vote_msg.vote().vote_data().proposed().id();
            proposals.push(proposal_id);
        }

        let mut node0_commits = vec![];
        for i in 0..7 {
            node0_commits.push(
                nodes[0]
                    .commit_cb_receiver
                    .next()
                    .await
                    .unwrap()
                    .ledger_info()
                    .consensus_block_id(),
            );
            assert_eq!(node0_commits[i], proposals[i]);
        }

        // Next proposal is delivered to all: as a result nodes[3] should be able to retrieve the
        // missing blocks from nodes[0] and commit the first eight proposals as well.
        playground.stop_drop_message_for(&nodes[0].smr.author(), &nodes[3].smr.author());
        playground
            .wait_for_messages(3, NetworkPlayground::proposals_only::<TestPayload>)
            .await;
        let mut node3_commits = vec![];
        // The only notification we will receive is for the last (8th) proposal.
        node3_commits.push(
            nodes[3]
                .commit_cb_receiver
                .next()
                .await
                .unwrap()
                .ledger_info()
                .consensus_block_id(),
        );
        assert_eq!(node3_commits[0], proposals[7]);

        // wait for the vote from all including node3
        playground
            .wait_for_messages(3, NetworkPlayground::votes_only::<TestPayload>)
            .await;

        playground
            .wait_for_messages(3, NetworkPlayground::proposals_only::<TestPayload>)
            .await;

        let committed_txns = nodes[3]
            .state_sync
            .next()
            .await
            .expect("MockStateSync failed to be notified by a mempool committed txns");
        let max_block_size = ConsensusConfig::default().max_block_size as usize;
        assert_eq!(committed_txns.len(), max_block_size);
    });
}

#[test]
/// Verify that a node syncs up when receiving a timeout message with a relevant ledger info
fn state_sync_on_timeout() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    // This test depends on the fixed proposer on nodes[0]
    let mut nodes = SMRNode::start_num_nodes(4, &mut playground, FixedProposer, false);
    block_on(async move {
        // The first ten proposals are delivered just to nodes[0..2], which should commit
        // the first seven blocks.
        // nodes[2] should be fully disconnected from the others s.t. its timeouts would not trigger
        // SyncInfo delivery ahead of time.
        playground.drop_message_for(&nodes[0].smr.author(), nodes[3].smr.author());
        playground.drop_message_for(&nodes[1].smr.author(), nodes[3].smr.author());
        playground.drop_message_for(&nodes[2].smr.author(), nodes[3].smr.author());
        for _ in 0..10 {
            playground
                .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
                .await;
            playground
                .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
                .await;
        }

        // Stop dropping messages from node 1 to node 0: next time node 0 sends a timeout to node 1,
        // node 1 responds with a SyncInfo that carries a LedgerInfo for commit at round >= 7.
        playground.stop_drop_message_for(&nodes[1].smr.author(), &nodes[3].smr.author());
        // Wait for the sync info message from 1 to 3
        playground
            .wait_for_messages(1, NetworkPlayground::sync_info_only::<TestPayload>)
            .await;
        // In the end of the state synchronization node 3 should have commit at round >= 7.
        assert!(
            nodes[3]
                .commit_cb_receiver
                .next()
                .await
                .unwrap()
                .ledger_info()
                .round()
                >= 7
        );
    });
}

#[test]
/// Verify that in case a node receives timeout message from a remote peer that is lagging behind,
/// then this node sends a sync info, which helps the remote to properly catch up.
fn sync_info_sent_if_remote_stale() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());
    // This test depends on the fixed proposer on nodes[0]
    // We're going to drop messages from 0 to 2: as a result we expect node 2 to broadcast timeout
    // messages, for which node 1 should respond with sync_info, which should eventually
    // help node 2 to catch up.
    let mut nodes = SMRNode::start_num_nodes(4, &mut playground, FixedProposer, false);
    block_on(async move {
        playground.drop_message_for(&nodes[0].smr.author(), nodes[2].smr.author());
        // Don't want to receive timeout messages from 2 until 1 has some real stuff to contribute.
        playground.drop_message_for(&nodes[2].smr.author(), nodes[1].smr.author());
        for _ in 0..10 {
            playground
                .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
                .await;
            playground
                .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
                .await;
        }

        // Wait for some timeout message from 2 to {0, 1}.
        playground.stop_drop_message_for(&nodes[2].smr.author(), &nodes[1].smr.author());
        playground
            .wait_for_messages(3, NetworkPlayground::timeout_votes_only::<TestPayload>)
            .await;
        // Now wait for a sync info message from 1 to 2.
        playground
            .wait_for_messages(1, NetworkPlayground::sync_info_only::<TestPayload>)
            .await;

        let node2_commit = nodes[2]
            .commit_cb_receiver
            .next()
            .await
            .unwrap()
            .ledger_info()
            .consensus_block_id();

        // Close node 1 channel for new commit callbacks and iterate over all its commits: we should
        // find the node 2 commit there.
        let mut found = false;
        nodes[1].commit_cb_receiver.close();
        while let Ok(Some(node1_commit)) = nodes[1].commit_cb_receiver.try_next() {
            let node1_commit_id = node1_commit.ledger_info().consensus_block_id();
            if node1_commit_id == node2_commit {
                found = true;
                break;
            }
        }

        assert_eq!(found, true);
    });
}

#[test]
/// Verify that a QC can be formed by aggregating the votes piggybacked by TimeoutMsgs
fn aggregate_timeout_votes() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    // The proposer node[0] sends its proposal to nodes 1 and 2, which cannot respond back,
    // because their messages are dropped.
    // Upon timeout nodes 1 and 2 are sending timeout messages with attached votes for the original
    // proposal: both can then aggregate the QC for the first proposal.
    let nodes = SMRNode::start_num_nodes(3, &mut playground, FixedProposer, false);
    block_on(async move {
        // Nodes 1 and 2 cannot send messages to anyone
        playground.drop_message_for(&nodes[1].smr.author(), nodes[0].smr.author());
        playground.drop_message_for(&nodes[2].smr.author(), nodes[0].smr.author());
        playground.drop_message_for(&nodes[1].smr.author(), nodes[2].smr.author());
        playground.drop_message_for(&nodes[2].smr.author(), nodes[1].smr.author());

        // Node 0 sends proposals to nodes 1 and 2
        let msg = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
            .await;
        let first_proposal = match &msg[0].1 {
            ConsensusMsg::ProposalMsg(proposal) => proposal,
            _ => panic!("Unexpected message found"),
        };
        let proposal_id = first_proposal.proposal().id();
        // wait for node 0 send vote to 1 and 2
        playground
            .wait_for_messages(2, NetworkPlayground::votes_only::<TestPayload>)
            .await;
        playground.drop_message_for(&nodes[0].smr.author(), nodes[1].smr.author());
        playground.drop_message_for(&nodes[0].smr.author(), nodes[2].smr.author());

        // Now when the nodes 1 and 2 have the votes from 0, enable communication between them.
        // As a result they should get the votes from each other and thus be able to form a QC.
        playground.stop_drop_message_for(&nodes[2].smr.author(), &nodes[1].smr.author());
        playground.stop_drop_message_for(&nodes[1].smr.author(), &nodes[2].smr.author());

        // Wait for the timeout messages sent by 1 and 2 to each other
        playground
            .wait_for_messages(2, NetworkPlayground::timeout_votes_only::<TestPayload>)
            .await;

        // Node 0 cannot form a QC
        assert_eq!(
            nodes[0]
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .certified_block()
                .round(),
            0
        );
        // Nodes 1 and 2 form a QC and move to the next round.
        // Wait for the timeout messages from 1 and 2
        playground
            .wait_for_messages(2, NetworkPlayground::timeout_votes_only::<TestPayload>)
            .await;

        assert_eq!(
            nodes[1]
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .certified_block()
                .id(),
            proposal_id
        );
        assert_eq!(
            nodes[2]
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .certified_block()
                .id(),
            proposal_id
        );
    });
}

#[test]
/// Verify that the NIL blocks formed during timeouts can be used to form commit chains.
fn chain_with_nil_blocks() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    // The proposer node[0] sends 3 proposals, after that its proposals are dropped and it cannot
    // communicate with nodes 1, 2, 3. Nodes 1, 2, 3 should be able to commit the 3 proposal
    // via NIL blocks commit chain.
    let num_nodes = 4;
    let nodes = SMRNode::start_num_nodes(num_nodes, &mut playground, FixedProposer, false);
    let num_proposal = 3;
    block_on(async move {
        // Wait for the first 3 proposals (each one sent to two nodes).
        playground
            .wait_for_messages(
                (num_nodes - 1) * num_proposal,
                NetworkPlayground::proposals_only::<TestPayload>,
            )
            .await;
        playground.drop_message_for(&nodes[0].smr.author(), nodes[1].smr.author());
        playground.drop_message_for(&nodes[0].smr.author(), nodes[2].smr.author());
        playground.drop_message_for(&nodes[0].smr.author(), nodes[3].smr.author());

        // After the first timeout nodes 1, 2, 3 should have last_proposal votes and
        // they can generate its QC independently.
        // Upon the second timeout nodes 1, 2, 3 send NIL block_1 with a QC to last_proposal.
        // Upon the third timeout nodes 1, 2, 3 send NIL block_2 with a QC to NIL block_1.
        // G <- p1 <- p2 <- p3 <- NIL1 <- NIL2
        let num_timeout = 3;
        playground
            .wait_for_messages(
                // all-to-all broadcast except nodes 0's messages are dropped and self messages don't count
                (num_nodes - 1) * (num_nodes - 1) * num_timeout,
                NetworkPlayground::timeout_votes_only::<TestPayload>,
            )
            .await;
        // We can't guarantee the timing of the last timeout processing, the only thing we can
        // look at is that HQC round is at least 4.
        assert!(
            nodes[2]
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .certified_block()
                .round()
                >= 4
        );

        assert!(nodes[2].smr.block_store().unwrap().root().round() >= 1)
    });
}

#[test]
/// Test secondary proposal processing
fn secondary_proposers() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    let num_nodes = 4;
    let mut nodes =
        SMRNode::start_num_nodes(num_nodes, &mut playground, MultipleOrderedProposers, false);
    block_on(async move {
        // Node 0 is disconnected.
        playground.drop_message_for(&nodes[0].smr.author(), nodes[1].smr.author());
        playground.drop_message_for(&nodes[0].smr.author(), nodes[2].smr.author());
        playground.drop_message_for(&nodes[0].smr.author(), nodes[3].smr.author());
        // Run a system until node 0 is a designated primary proposer. In this round the
        // secondary proposal should be voted for and attached to the timeout message.
        let timeout_votes = playground
            .wait_for_messages(
                (num_nodes - 1) * (num_nodes - 1),
                NetworkPlayground::timeout_votes_only::<TestPayload>,
            )
            .await;
        let mut secondary_proposal_ids = vec![];
        for msg in timeout_votes {
            let vote_msg = match msg.1 {
                ConsensusMsg::VoteMsg(vote_msg) => vote_msg,
                _ => panic!("Unexpected message found"),
            };
            assert!(vote_msg.vote().is_timeout());
            secondary_proposal_ids.push(vote_msg.vote().vote_data().proposed().id());
        }
        assert_eq!(
            secondary_proposal_ids.len(),
            (num_nodes - 1) * (num_nodes - 1)
        );
        let secondary_proposal_id = secondary_proposal_ids[0];
        for id in secondary_proposal_ids {
            assert_eq!(secondary_proposal_id, id);
        }
        // The secondary proposal id should get committed at some point in the future:
        // 10 rounds should be more than enough. Note that it's hard to say what round is going to
        // have 2 proposals and what round is going to have just one proposal because we don't want
        // to predict the rounds with proposer 0 being a leader.
        for _ in 0..10 {
            playground
                .wait_for_messages(num_nodes - 1, NetworkPlayground::votes_only::<TestPayload>)
                .await;
            // Retrieve all the ids committed by the node to check whether secondary_proposal_id
            // has been committed.
            while let Ok(Some(li)) = nodes[1].commit_cb_receiver.try_next() {
                if li.ledger_info().consensus_block_id() == secondary_proposal_id {
                    return;
                }
            }
        }
        panic!("Did not commit the secondary proposal");
    });
}

#[test]
/// Test we can do reconfiguration if execution returns new validator set.
fn reconfiguration_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    // This quorum size needs to be 2f+1 because we derive the ValidatorVerifier from ValidatorSet at network.rs
    // which doesn't support specializing quorum power
    let _nodes = SMRNode::start_num_nodes(4, &mut playground, MultipleOrderedProposers, true);
    let target_epoch = 10;
    block_on(async move {
        // Test we can survive a few epochs
        loop {
            let mut msg = playground
                .wait_for_messages(1, NetworkPlayground::take_all)
                .await;
            let msg = msg.pop().unwrap().1;
            if let ConsensusMsg::<TestPayload>::ValidatorChangeProof(proof) = msg {
                if proof.epoch().unwrap() == target_epoch {
                    break;
                }
            }
        }
    });
}

#[test]
/// Setup:
/// Start a network with 3 nodes and quorum voting power of 2
/// The leader is node0 (FixedProposer)
/// Test:
/// Create a partition between node0 and node1
/// Let node0 send a proposal
/// Check that the proposed block is not available in the store of node1
/// Remove the previously created partition between node0 and node1
/// Let node0 send a proposal
/// Check that the proposed block is available in the store of node1
fn network_partition_start_stop_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());
    let nodes = SMRNode::start_num_nodes(3, 2, &mut playground, FixedProposer, false);

    block_on(async move {
        let n0 = &nodes[0].signer.author();
        let n1 = &nodes[1].signer.author();

        playground.split_network(vec![n0], vec![n1]);

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 2 votes for the 2 replicas (node0 and node2)
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(2, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is present in the block store of node0
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        // Verify that the proposed block id is NOT present in the block store of node1
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());

        // Undo the split
        playground.stop_split_network(vec![n0], vec![n1]);

        let _proposals2 = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 2 votes for the 2 replicas node0 and node1
        let votes2: Vec<VoteMsg> = playground
            .wait_for_messages(3, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id2 = votes2[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is present in the block store of node0
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id2)
            .is_some());

        // Verify that the proposed block id is present in the block store of node1
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id2)
            .is_some());
    });
}
