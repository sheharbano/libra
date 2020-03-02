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
use libra_logger::prelude::*;
use std::collections::HashMap;
use libra_crypto::hash::HashValue;

use consensus_types::{
    common::Round, executed_block::ExecutedBlock
};

use permutator::{Combination, XPermutationIterator};
use itertools::Itertools;
use itertools::enumerate;

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

    // Returns a vector of nodes created, and a mapping between node index
    // and corresponding twin index (twins are included in the nodes vector)
    fn start_num_nodes_with_twins(
        num_nodes: usize,
        // Index #s of nodes (i.e. target nodes) for which we will create twins
        target_nodes: &Vec<usize>,
        quorum_voting_power: u64,
        playground: &mut NetworkPlayground,
        proposer_type: ConsensusProposerType,
        executor_with_reconfig: bool,
    ) -> (Vec<Self>, HashMap<usize, usize>) {
        let (mut signers, mut validator_verifier) =
            random_validator_verifier(num_nodes, Some(quorum_voting_power), true);

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

        // Vector of twins
        let mut twins: Vec<ValidatorSigner> = vec![];

        // Starting index for twin account addresses. This will
        // appear in logs. I chose 240 (hex: f0), so twins will appear as
        // "f0" onwards in logs
        let mut twin_account_index = 240;

        // Add twins to 'signers' and 'validator verifier'
        for ref_target_node in target_nodes {
            let target_node = *ref_target_node;

            // Clone the target node and add to vector of twins
            twins.push(signers[target_node].clone());

            // Index of the newly added twin
            let twins_top = twins.len() - 1;

            // The twin should be equal to the target node, at this point
            assert_eq!(twins[twins_top], signers[target_node]);

            // Change the twin's account address to "twin_account_address"
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

            // Update "signers", so the newly created twin is included
            // Explanation: "ValidatorSigner" is a struct that has node's
            // account address, public and private keys. "Signers" is a
            // vector of "ValidatorSigner" for each node.
            // "Signers" is passed to "SMRNode::start()" to start SMR nodes
            // with certain account addresses and public and private keys
            signers.push(twins[twins_top].clone());
            // Also update "validator_verifier", i.e. add twin to its
            // "address_to_validator_info"
            // Explanation: ValidatorVerifier is a struct that includes
            // total voting power, quorum voting power, and a hashmap
            // "address_to_validator_info" that maps account addresses
            // of all nodes to public key and voting power (wrapped in
            // a struct "ValidatorInfo").
            // "ValidatorVerifier" supports validation of signatures for
            // known authors with individual voting powers. This struct
            // can be used for all signature verification operations
            // including block and network signature verification.
            // "ValidatorVerifier" is also used by consensus to send
            // messages to nodes, (see "consensus/src/chained_bft/network.rs")
            let target_account_address = signers[target_node].author();
            ValidatorVerifier::add_to_address_to_validator_info(
                &mut validator_verifier,
                twin_account_address.clone(),
                &target_account_address
            );
        }

        // A map that tells who is the proposer(s) per round
        // Note: If no proposer is defined for a round, we default to the first node
        let mut twins_round_proposers: HashMap<Round, Vec<AccountAddress>> = HashMap::new();
        // TODO: Read this from an input file eventually
        // FIXME: Specify round proposers here

        /*
        twins_round_proposers.insert(
            1,
            vec![signers[0].author(),
                 signers[node_to_twin.get(&0).unwrap().to_owned()].author(),
            ]
        );

        twins_round_proposers.insert(
            1,
            vec![signers[0].author()]
             );
        */


        // Leader for round 1: node0 and twin_node0
        twins_round_proposers.insert(
            1,
            vec![
                signers[0].author(),
                signers[node_to_twin.get(&0).unwrap().to_owned()].author(),
            ],
        );
        // Leader for round 2: node0 and twin_node0
        twins_round_proposers.insert(
            2,
            vec![
                signers[0].author(),
                signers[node_to_twin.get(&0).unwrap().to_owned()].author(),
            ],
        );
        // Leader for round 3: node0 and twin_node0
        twins_round_proposers.insert(
            3,
            vec![
                signers[0].author(),
                signers[node_to_twin.get(&0).unwrap().to_owned()].author(),
            ],
        );
        // Leader for round 4: node0 and twin_node0
        twins_round_proposers.insert(
            4,
            vec![
                signers[0].author(),
                signers[node_to_twin.get(&0).unwrap().to_owned()].author(),
            ],
        );

        ValidatorVerifier::set_round_to_validators(&mut validator_verifier, twins_round_proposers);

        let validator_set = if executor_with_reconfig {
            Some((&validator_verifier).into())
        } else {
            None
        };

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

        (nodes, node_to_twin)
>>>>>>> 66928da8... Minor fix & run cargo fmt
    }
}

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

#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the nodes and twins.
fn twins_start_with_proposal_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // Start a network with 2 nodes and 2 twins for those nodes
    let (nodes, node_to_twin) = SMRNode::start_num_nodes_with_twins(
        2,
        &mut target_nodes,
        3,
        &mut playground,
        RoundProposers,
        false,
    );

    let n0 = nodes[0].signer.author();
    let n1 = nodes[1].signer.author();
    let twin0 = nodes[node_to_twin.get(&0).unwrap().to_owned()]
        .signer
        .author();
    let twin1 = nodes[node_to_twin.get(&1).unwrap().to_owned()]
        .signer
        .author();

    // Drop messages from node1 and twin_node1 to node0
    //playground.drop_message_for_round(n1,  n0, 1);
    //playground.drop_message_for_round(twin1,  n0, 1);

    block_on(async move {
        // Two proposals (by node0 and twin_node0)
        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 4 votes for the 2 replicas plus their twins
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(8, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        //let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Twin leader's HQC
        let hqc_twin0 = nodes[3]
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert();

        // Any other nodes' HQC
        let hqc_node1 = nodes[1]
            .smr
            .block_store()
            .unwrap()
            .highest_quorum_cert();


        // Proposal from node0 and twin_node0 are going to race
        // but only one of them will form QC because quorum voting
        // power is set to 3
        assert_eq!(hqc_twin0, hqc_node1);

    });
}


#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the nodes and twins
/// except node 01 because it has been added to drop_config for round 1 and does not hear
/// the proposal from node 0.
fn twins_drop_config_round_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    let (nodes, node_to_twin) = SMRNode::start_num_nodes_with_twins(
        2,
        &mut target_nodes,
        2,
        &mut playground,
        RoundProposers,
        false,
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
    playground.drop_message_for_round(n0, n1, 3);
    playground.drop_message_for_round(n0, twin0, 3);

    block_on(async move {
        // ===== Round 1 ======

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 3 votes (from the 1 of the 2 replicas, and twins)
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(3, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store.
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());
        // node 1 does not have the block id because it did not receive the proposal from node 0
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
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

        // ========= Round 2 =========
        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 4 votes (from the 2 replicas, and their twins)
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(4, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store.
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());
        // node 1 should now have the block id because it's not in drop_config in round 2
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

        // Need to wait for 1 vote
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(1, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store.
        assert!(nodes[0]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());
        // node 1 does not have the block id because it did not receive the proposal from node 0
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
        // twin 0 does not have the block id because it did not receive the proposal from node 0
        assert!(nodes[2]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
        // twin 1 does not have the block id because it did not receive the proposal from node 0
        assert!(nodes[3]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());
    });
}

#[test]
/// node 0 and its twin (twin 0) are both leader, and we split
/// the network as follows:
///      partition 1: node 0, node 1, node 2
///      partition 2: twin 0, twin 1, node 3
///
/// The purpose of this test is to create a safety violation;
/// n2 commits the block proposed by node 0, and node 3 commits
/// the block proposed by twin 0.
///
/// run the test:
/// cargo xtest -p consensus twins_test_simple_safety_attack -- --nocapture
fn twins_test_simple_safety_attack() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    let (nodes, node_to_twin) = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ 4,
        &mut target_nodes,
        /* quorum_voting_power */ 3,
        &mut playground,
        RoundProposers, //FixedProposer,
        /* executor_with_reconfig */ false,
    );

    let n0 = nodes[0].signer.author();
    let n1 = nodes[1].signer.author();
    let twin0 = nodes[node_to_twin.get(&0).unwrap().to_owned()]
        .signer
        .author();
    let twin1 = nodes[node_to_twin.get(&1).unwrap().to_owned()]
        .signer
        .author();
    let n2 = nodes[2].signer.author();
    let n3 = nodes[3].signer.author();

    // `Create static netwok partitions`
    playground.split_network(vec![&n0, &n1, &n2], vec![&twin0, &twin1, &n3]);

    block_on(async move {
        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Pull enough votes to get a commit on the first block)
        // The proposer's votes are implicit and do not go in the queue.
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(23, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        /*
        // Verify that the proposed block id is indeed present in the
        // block store of replicas and their twins.
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

        // Check that all nodes of partition 1 have the same QC.
        let commit_info_parition_1 = nodes[0]
            .smr
            .block_store()
            .unwrap()
            .highest_quorum_cert()
            .commit_info()
            .id();
        assert_eq!(
            nodes[1]
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .commit_info()
                .id(),
            commit_info_parition_1
        );
        assert_eq!(
            nodes[2]
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .commit_info()
                .id(),
            commit_info_parition_1
        );

        // Check that all nodes in partition 2 have the same QC
        let commit_info_parition_2 = nodes[3]
            .smr
            .block_store()
            .unwrap()
            .highest_quorum_cert()
            .commit_info()
            .id();
        assert_eq!(
            nodes[4] // twin 0
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .commit_info()
                .id(),
            commit_info_parition_2
        );
        assert_eq!(
            nodes[5] // twin 1
                .smr
                .block_store()
                .unwrap()
                .highest_quorum_cert()
                .commit_info()
                .id(),
            commit_info_parition_2
        );

        // Show that the QC of the nodes in parition 1 and partition 2 are different
        assert_ne!(commit_info_parition_1, commit_info_parition_2);

        // Show that the QC of all nades are on the same round
        let commit_round_partition_1 = nodes[0]
            .smr
            .block_store()
            .unwrap()
            .highest_quorum_cert()
            .certified_block()
            .round();
        let commit_round_partition_2 = nodes[3]
            .smr
            .block_store()
            .unwrap()
            .highest_quorum_cert()
            .certified_block()
            .round();
        assert_eq!(commit_round_partition_1, commit_round_partition_2);
        */

        let mut all_branches: Vec<Vec<Arc<ExecutedBlock<TestPayload>>>> = Vec::new();

        // =================
        // Branch for node 0
        // =================

        let branch_head0 = nodes[0]
            .smr
            .block_store()
            .unwrap()
            .highest_ledger_info()
            .commit_info()
            .id();

        let branch0: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[0]
            .smr
            .block_store()
            .unwrap()
            .path_from_root(branch_head0)
            .unwrap_or_else(Vec::new);

        all_branches.push(branch0);

        // =================
        // Branch for node 1
        // =================

        let branch_head1 = nodes[1]
            .smr
            .block_store()
            .unwrap()
            .highest_ledger_info()
            .commit_info()
            .id();

        let branch1: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[1]
            .smr
            .block_store()
            .unwrap()
            .path_from_root(branch_head1)
            .unwrap_or_else(Vec::new);

        all_branches.push(branch1);

        // =================
        // Branch for node 2
        // =================

        let branch_head2 = nodes[2]
            .smr
            .block_store()
            .unwrap()
            .highest_ledger_info()
            .commit_info()
            .id();

        let branch2: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[2]
            .smr
            .block_store()
            .unwrap()
            .path_from_root(branch_head2)
            .unwrap_or_else(Vec::new);

        all_branches.push(branch2);


        // =================
        // Branch for node 3
        // =================

        let branch_head3 = nodes[3]
            .smr
            .block_store()
            .unwrap()
            .highest_ledger_info()
            .commit_info()
            .id();

        let branch3: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[3]
            .smr
            .block_store()
            .unwrap()
            .path_from_root(branch_head3)
            .unwrap_or_else(Vec::new);

        all_branches.push(branch3);

        // =================
        // Branch for node 4
        // =================

        let branch_head4 = nodes[4]
            .smr
            .block_store()
            .unwrap()
            .highest_ledger_info()
            .commit_info()
            .id();

        let branch4: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[4]
            .smr
            .block_store()
            .unwrap()
            .path_from_root(branch_head4)
            .unwrap_or_else(Vec::new);

        all_branches.push(branch4);

        // =================
        // Branch for node 5
        // =================

        let branch_head5 = nodes[5]
            .smr
            .block_store()
            .unwrap()
            .highest_ledger_info()
            .commit_info()
            .id();

        let branch5: Vec<Arc<ExecutedBlock<TestPayload>>> = nodes[5]
            .smr
            .block_store()
            .unwrap()
            .path_from_root(branch_head5)
            .unwrap_or_else(Vec::new);

        all_branches.push(branch5);


        assert!(!is_safe(all_branches));
    });
}


fn create_partitions(
    playground: &mut NetworkPlayground,
    partitions: HashMap<u64,Vec<Vec<AccountAddress>>>
) {
    for round in partitions.keys() {
        // TODO: create partitions
        println!("{}", round);
    }
}

// Compares branches at each height to check if there's any conflict
fn is_safe(branches: Vec<Vec<Arc<ExecutedBlock<TestPayload>>>>) -> bool {
    compare_vectors(&branches)
}

fn compare_vectors(vecs: &Vec<Vec<Arc<ExecutedBlock<TestPayload>>>>) -> bool {

    // how many vectors need to be compared
    let num_vecs = vecs.len();

    println!("Comparing {0} vectors", num_vecs);
    for (each, item) in vecs.iter().enumerate() {
        println!("====== Vector {0} =======", each);
        for (x, y) in item.iter().enumerate() {
            print!("Index{0}:{1}\t",x, y.id());
        }
        print!("\n");
    }

    let (longest_idx,longest_len)=longest_vector(vecs);
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
                if first_time  {
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
    is_conflict
}


fn print_conflict(vecs: &Vec<Vec<Arc<ExecutedBlock<TestPayload>>>>, index: usize) {

    println!("CONFLICT: Index {0} doesn't match, values are:", index);

    for vec in vecs.iter() {
        if index < vec.len() {
            println!("{0}", vec[index].id());
        }
        else {
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

    (idx,max_len)
}





fn run_experiment(
    num_nodes: usize,
    partitions: HashMap<u64,Vec<Vec<AccountAddress>>>,
    leaders: HashMap<usize,usize>
) {
    assert_eq!(partitions.len(), leaders.len());
    let num_of_rounds = partitions.len().clone();
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    let f: usize = (num_nodes-1) / 3;
    for i in 0..f {
        target_nodes.push(i);
    }
    // TODO: 'leaders' should be provided to 'start_num_nodes_with_twins'
    // (but we first need to address the FIXME in 'start_num_nodes_with_twins').
    let (nodes, node_to_twin) = SMRNode::start_num_nodes_with_twins(
        num_nodes,
        &mut target_nodes,
        /* quorum_voting_power */ (num_nodes - f) as u64,
        &mut playground,
        RoundProposers,
        /* executor_with_reconfig */ false,
    );

    // Create partitions.
    create_partitions(&mut playground, partitions);

    block_on(async move {
        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // The proposer's votes are implicit and do not go in the queue.
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(num_nodes * num_of_rounds, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Check for safety violations
        //assert!(!is_safe());
    });
}

// Solves the 'stars and bars' problem for bars=2. Ideally, we want to implement the
// full recursive algorthm to accomodate any number of bars. Such algorithm woud work
// as follwos: 1) guess how many stars are within each bar; 2) assign them; and 3) recurse
// to a smaller problem with less stars and one bar less.
// Note however that the function below maintains orders of the nodes, on which our test
// unit 'twins_test_safety_attack_generator' rely. That is, it always try to put twins
// in different partitions.
fn fill_partitions(nodes: Vec<usize>, num_of_partitions: usize) -> Vec<Vec<Vec<usize>>> {
    assert!(num_of_partitions > 0);
    assert!(nodes.len() >= num_of_partitions);

    assert!(num_of_partitions <= 3);
    match num_of_partitions {
        1 => vec![vec![nodes.to_vec()]],
        2 => fill_two_partitions(nodes),
        _ => fill_three_partitions(nodes),
    }
}

fn fill_two_partitions(nodes: Vec<usize>) -> Vec<Vec<Vec<usize>>> {
    let p = 2; // number of partitions
    let l = nodes.len(); // number of nodes
    let mut list: Vec<Vec<Vec<usize>>> = Vec::new();

    let threshold = match l % p {
        0 => l/p,
        _ => l/p + 1,
    };
    let mut x = l-p+1;
    while x >= threshold {
        let partition_0: Vec<usize> = nodes[0..x].iter().cloned().collect();
        let partition_1: Vec<usize> = nodes[x..l].iter().cloned().collect();
        list.push(vec![partition_0, partition_1]);
        x -= 1;
    }
    list
}

// NOTE: Does not hande the corner case 3 nodes for 3 partitions.
fn fill_three_partitions(nodes: Vec<usize>) -> Vec<Vec<Vec<usize>>> {
    let p = 3; // number of partitions
    let l = nodes.len(); // number of nodes
    let mut list: Vec<Vec<Vec<usize>>> = Vec::new();

    let threshold = match l % p {
        0 => l/p,
        _ => l/p + 1,
    };
    let mut x = l-p+1;
    while x >= threshold {
        // call fill_two_partitions
        // this will create a few more test cases than what is required
        // (ie. some superflous cases)
        let list_two = fill_two_partitions(nodes[x..l].iter().cloned().collect());
        for item in list_two {
            let partition_0: Vec<usize> = nodes[0..x].iter().cloned().collect();
            let partition_1: Vec<usize> = item[0].to_vec();
            let partition_2: Vec<usize> = item[1].to_vec();
            list.push(vec![partition_0, partition_1, partition_2]);
        }
        x -= 1;
    }
    list
}

#[test]
/// cargo xtest -p consensus test_fill_partitions -- --nocapture
fn test_fill_partitions() {
    let mut list_2 = fill_partitions((0..4).collect(), 2);
    assert_eq!(
        list_2,
        vec![vec![vec![0, 1, 2], vec![3]], vec![vec![0, 1], vec![2, 3]]]
    );
    list_2 = fill_partitions((0..3).collect(), 2);
    assert_eq!(
        list_2,
        vec![vec![vec![0, 1], vec![2]]]
    );

    let mut list_3 = fill_partitions((0..4).collect(), 3);
    assert_eq!(
        list_3,
        vec![vec![vec![0, 1], vec![2], vec![3]]]
    );
    list_3 = fill_partitions((0..10).collect(), 3);
    for item in list_3 {
        assert_eq!(item[0].len()+item[1].len()+item[2].len(), 10);
    }
}

#[test]
/// run:
/// cargo xtest -p consensus twins_test_safety_attack_generator -- --nocapture
/// TODO: implement leaders
fn twins_test_safety_attack_generator() {
    const NUM_OF_ROUNDS: usize = 4; // Play with this parameter
    const NUM_OF_NODES: usize = 7; // Play with this parameter
    const NUM_OF_PARTITIONS: usize = 2; // Play with this parameter

    let f: usize = (NUM_OF_NODES-1) / 3;
    let mut partitions = HashMap::new();
    let mut leaders: HashMap<usize,usize> = HashMap::new();

    // First fill the bad ndoes. By convention, the first nodes are bad.
    let mut nodes:Vec<usize> = (0..f).collect();

    // Then, add the other (honest) nodes
    let mut honest_nodes:Vec<usize> = (f..NUM_OF_NODES).collect();
    nodes.append(&mut honest_nodes);

    // Finally, add the twins of the bad nodes; those created at last by
    // the function 'start_num_nodes_with_twins'.
    let mut twin_nodes:Vec<usize> = (NUM_OF_NODES..NUM_OF_NODES+f).collect();
    nodes.append(&mut twin_nodes);

    // Fill the partitions in any possible way.
    // This problems is knows as the 'stars and bars' problem because the (honest)
    // nodes are *indistinguishable* and the paritions are *distinguishable*.
    // If we want to consider that nodes are distinguishable as well (perhaps because
    // different validators may run on different hardware/software), then we need to use
    // Stirling numbers of the second kind (which makes the problem much more compicated).
    let list = fill_partitions(nodes, NUM_OF_PARTITIONS);
    // Note that for a k-out-of-n permutation, k > n; otherwise the output is empty.
    let permutations = (0..list.len()).permutations(NUM_OF_ROUNDS);
    let mut count = 0;
    for test in permutations {
        println!("HERE: {:?}", test);
        for (r, p) in enumerate(test) {
            partitions.insert(r, list[p].to_vec());
        }
        //run_experiment(num_nodes, round_partitions, leaders);
        count += 1;
    }
    println!("Number of test cases: {:?}", count);
}

#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the participants.
fn twinsless_partition_start_stop_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());
    let nodes = SMRNode::start_num_nodes(2, 2, &mut playground, FixedProposer, false);

    block_on(async move {
        let n0 = &nodes[0].signer.author();
        let n1 = &nodes[1].signer.author();

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        playground.split_network(vec![n0], vec![n1]);
        playground.stop_split_network(vec![n0], vec![n1]);

        // Need to wait for 2 votes for the 2 replicas
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(20, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
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

#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the participants.
fn twinsless_block_proposal_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());
    let nodes = SMRNode::start_num_nodes(2, 2, &mut playground, FixedProposer, false);

    block_on(async move {
        let n0 = &nodes[0].signer.author();
        let n1 = &nodes[1].signer.author();
        playground.split_network(vec![n0], vec![n1]);

        // The messages below are never delivered
        // Expected to hang

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 2 votes for the 2 replicas
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(20, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
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

#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the participants.
fn twinsless_block_vote_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());
    let nodes = SMRNode::start_num_nodes(2, 2, &mut playground, FixedProposer, false);

    block_on(async move {
        let n0 = &nodes[0].signer.author();
        let n1 = &nodes[1].signer.author();

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        playground.split_network(vec![n0], vec![n1]);

        // The messages below are never delivered
        // Expected to hang

        // Need to wait for 2 votes for the 2 replicas
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(20, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
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

#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the participants.
fn twins_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());
    let nodes = SMRNode::start_num_nodes(2, 2, &mut playground, FixedProposer, false);

    block_on(async move {
        let n0 = &nodes[0].signer.author();
        let n1 = &nodes[1].signer.author();
        let twin0 = &nodes[2].signer.author();
        let twin1 = &nodes[3].signer.author();
        playground.split_network(vec![n0, n1], vec![twin0, twin1]);
        playground.stop_split_network(vec![n0, n1], vec![twin0, twin1]);

        // The messages below are never delivered
        // Expected to hang

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 2 votes for the 2 replicas
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(4, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
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
    });
}

#[test]
/// Upon startup, the first proposal is sent, delivered and voted by all the participants.
fn twins_block_proposal_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.executor());
    let nodes = SMRNode::start_num_nodes(2, 2, &mut playground, FixedProposer, false);

    block_on(async move {
        let n0 = &nodes[0].signer.author();
        let n1 = &nodes[1].signer.author();
        let twin0 = &nodes[2].signer.author();
        let twin1 = &nodes[3].signer.author();
        playground.split_network(vec![n0, n1], vec![twin0, twin1]);

        // The messages below are never delivered
        // Expected to hang

        let _proposals = playground
            .wait_for_messages(1, NetworkPlayground::proposals_only)
            .await;

        // Need to wait for 2 votes for the 2 replicas
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(4, NetworkPlayground::votes_only)
            .await
            .into_iter()
            .map(|(_, msg)| VoteMsg::try_from(msg).unwrap())
            .collect();
        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store.
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
