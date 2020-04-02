// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

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
    },
    generator::{self, ValidatorSwarm},
};
use libra_crypto::{hash::CryptoHash, HashValue};
use libra_mempool::mocks::MockSharedMempool;
use libra_types::{
    ledger_info::LedgerInfoWithSignatures, validator_set::ValidatorSet,
    validator_verifier::ValidatorVerifier,
};
use network::peer_manager::{
    conn_status_channel, ConnectionRequestSender, PeerManagerRequestSender,
};
use std::{num::NonZeroUsize, sync::Arc};

use std::collections::HashMap;
use consensus_types::{common::Round, executed_block::ExecutedBlock};
use libra_types::account_address::AccountAddress;
use libra_config::config::ConsensusProposerType::RoundProposers;
use itertools::enumerate;
use itertools::Itertools;
use rand::Rng;
use libra_logger::prelude::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};

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
        let (connection_reqs_tx, _) =
            libra_channel::new(QueueStyle::FIFO, NonZeroUsize::new(8).unwrap(), None);
        let (consensus_tx, consensus_rx) =
            libra_channel::new(QueueStyle::FIFO, NonZeroUsize::new(8).unwrap(), None);
        let (conn_mgr_reqs_tx, conn_mgr_reqs_rx) = channel::new_test(8);
        let (_, conn_status_rx) = conn_status_channel::new();
        let network_sender = ConsensusNetworkSender::new(
            PeerManagerRequestSender::new(network_reqs_tx),
            ConnectionRequestSender::new(connection_reqs_tx),
            conn_mgr_reqs_tx,
        );
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

    fn stop(mut self) {
        self.smr.stop();
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
            // Set higher timeout value in test.
            node_config.consensus.pacemaker_initial_timeout_ms = 5000;

            let (_, storage) = MockStorage::start_for_testing(validator_set.clone());
            smr_nodes.push(Self::start(
                playground,
                node_config,
                smr_id,
                storage,
                executor_validator_set.clone(),
            ));
        }
        smr_nodes
    }


    // Starts num_nodes and target_nodes.len() twins
    // Returns a vector of nodes (including twins) created
    #[cfg(any(test, feature = "fuzzing"))]
    fn start_num_nodes_with_twins(
        num_nodes: usize,
        // Indices of nodes (i.e. target nodes) for which we will create twins
        target_nodes: &Vec<usize>,
        playground: &mut NetworkPlayground,
        proposer_type: ConsensusProposerType,
        executor_with_reconfig: bool,
        // Leaders per round
        twins_round_proposers_idx: HashMap<Round, Vec<usize>>,
        // The indices in the vector of nodes returned by this function
        // where twins will be created
        _node_to_twin: &HashMap<usize, usize>,
    ) -> Vec<Self> {

        // ======================================
        // Generate 'NodeConfig' and 'ValidatorSet'
        // ======================================

        // ValidatorSwarm is a struct containing Vec<NodeConfig>,
        // and ValidatorSet.
        //
        // NodeConfig --> is a struct with configs for each module, e.g.
        // AdmissionControlConfig, RpcConfig, BaseConfig, ConsensusConfig etc.
        // pulls in configuration information from the config file.
        // This is used to set up the nodes and configure various parameters.
        // The config file is broken up into sections for each module
        // so that only that module can be passed around
        //
        // ValidatorSet --> is actually Vec<ValidatorPublicKeys<PublicKey>>
        // ValidatorPublicKeys: is a struct that contains the validator's
        // account address, consensus public key, consensus voting power,
        // network signing key and network identity public key.
        //
        // After executing a special transaction ValidatorSet indicates
        // a change to the next epoch, consensus and networking get the new
        // list of validators, their keys, and their voting power.  Consensus
        // has a public key to validate signed messages and networking will
        // have public signing and identity keys for creating secure channels
        // of communication between validators.  The validators and their
        // public keys and voting power may or may not change between epochs.

        // Create a network of nodes, with twins created for target_nodes
        let ValidatorSwarm {
            mut nodes,
            validator_set,
        } = generator::validator_swarm_for_testing_twins(num_nodes, target_nodes.clone());

        let executor_validator_set = if executor_with_reconfig {
            Some(validator_set.clone())
        } else {
            None
        };

        // ======================================
        // Set leaders per round
        // ======================================

        if twins_round_proposers_idx.len() > 0 {
            // A map that tells who is the proposer(s) per round
            // Note: If no proposer is defined for a round, we default to the first node
            let mut twins_round_proposers: HashMap<Round, Vec<AccountAddress>> = HashMap::new();

            for (round, vec_idx) in twins_round_proposers_idx.iter() {
                let mut idx_to_authors = Vec::new();

                for idx in vec_idx.iter() {
                    let author = nodes[idx.to_owned()].validator_network.as_ref().unwrap().peer_id;
                    idx_to_authors.push(author);
                }

                twins_round_proposers.insert(round.to_owned(), idx_to_authors);
            }

            for each_node in &mut nodes {
                each_node.set_round_to_proposers(twins_round_proposers.clone());
            }
        }


        // ====================
        // Start nodes
        // ====================

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

        smr_nodes
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

fn basic_full_round(num_nodes: usize, proposer_type: ConsensusProposerType) {
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

// =======================
// Twins tests
// =======================



#[test]
/// This test checks that when a node and its twin are both leaders for a round,
/// only one of the two proposals gets a QC
///
/// Setup:
///
/// Network of 4 nodes (n0, n1, n2, n3), and twins (twin0, twin1)
///
/// Test:
///
/// Let n0 and twin0 propose a block in round1
/// Pull out enough votes so a QC can be formed
/// Check that the QC of n0 and twin0 matches
///
/// Run the test:
/// cargo xtest -p consensus twins_QC_test -- --nocapture
fn twins_qc_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    let num_nodes = 4;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
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

    // Leaders are n0 and twin0 for round 1..10
    for i in 1..10 {
        twins_round_proposers_idx.insert(i, vec![0, node_to_twin.get(&0).unwrap().to_owned()]);
        //twins_round_proposers_idx.insert(i, vec![0,1]);
    }

    // Start a network with 2 nodes and 2 twins for those nodes
    let nodes = SMRNode::start_num_nodes_with_twins(
        num_nodes,
        &mut target_nodes,
        &mut playground,
        //RotatingProposer,
        RoundProposers,
        false,
        twins_round_proposers_idx,
        &node_to_twin,
    );


    block_on(async move {
        // Two proposals (by n0 and twin0)
        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
            .await;

        // Pull out enough votes so a QC can be formed
        let _votes: Vec<VoteMsg> = playground
            .wait_for_messages(15, NetworkPlayground::votes_only::<TestPayload>)
            .await
            .into_iter()
            .map(|(_, msg)| match msg {
                ConsensusMsg::VoteMsg(vote_msg) => *vote_msg,
                _ => panic!("Unexpected message found"),
            })
            .collect();


        // Twin leader's HQC
        let hqc_twin0 = nodes[4].smr.block_store().unwrap().highest_quorum_cert();

        // Any other nodes' HQC
        let hqc_node0 = nodes[0].smr.block_store().unwrap().highest_quorum_cert();

        // Proposal from node0 and twin_node0 are going to race
        // but only one of them will form QC so ultimately they'll
        // have the same QC
        assert_eq!(hqc_twin0, hqc_node0);
    });
}


#[test]
/// Checks that split_network_round works for twins
///
/// Setup:
/// Creates a network of 4 nodes (n0, n1, n2, n3) and 2 twins (twin0 and twin1).
/// Each round has n0 as the leader.
/// Creates a network partition between (n0, n3, n2, twin1) and (n1 and twin0)
/// The first partition will be able to make commits, but the second partition
/// will not be able to make progress (due to quorum size)
///
/// Test: n0 sends a proposal, which reaches (n0, n1, n2, twin0).
/// Check that the block is present in the store of e.g. n0 and twin0,
/// and should not be present in the store of n3 and twin1.
///
/// Run the test:
/// cargo xtest -p consensus twins_split_network_test -- --nocapture
fn twins_split_network_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    let num_nodes = 4;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
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

    // Leader is n0 for round 1..20
    for i in 1..20 {
        //twins_round_proposers_idx.insert(i, vec![0, node_to_twin.get(&0).unwrap().to_owned()]);
        twins_round_proposers_idx.insert(i, vec![0]);
    }

    let nodes = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ num_nodes,
        &mut target_nodes,
        &mut playground,
        RoundProposers, //FixedProposer,
        /* executor_with_reconfig */ false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    block_on(async move {
        let n0 = &nodes[0].smr.author();
        let n1 = &nodes[1].smr.author();
        let n2 = &nodes[2].smr.author();
        let n3 = &nodes[3].smr.author();
        let twin0 = &nodes[4].smr.author();
        let twin1 = &nodes[5].smr.author();

        let mut round_partitions: HashMap<u64, Vec<Vec<AccountAddress>>> = HashMap::new();

        for round in 0..20 {
            round_partitions.insert(
                round,
                vec![
                    vec![twin1.clone(), n0.clone(), n2.clone(), n3.clone()],
                    vec![twin0.clone(), n1.clone()],
                ],
            );
        }

        // Proposals from node0 will never reach n3 and twin1
        playground.split_network_round(&round_partitions);

        //playground.print_drop_config_round();

        let _proposals = playground
            .wait_for_messages(5, NetworkPlayground::proposals_only::<TestPayload>)
            .await;

        // Pull out enough votes so a QC can be formed
        let votes: Vec<VoteMsg> = playground
            .wait_for_messages(30, NetworkPlayground::votes_only::<TestPayload>)
            .await
            .into_iter()
            .map(|(_, msg)| match msg {
                ConsensusMsg::VoteMsg(vote_msg) => *vote_msg,
                _ => panic!("Unexpected message found"),
            })
            .collect();

        let proposed_block_id = votes[0].vote().vote_data().proposed().id();

        // Verify that the proposed block id is indeed present in the block store
        // of node3
        assert!(nodes[3]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_some());

        // The proposed block id should not be present in the block store
        // of n1 and twin0
        assert!(nodes[1]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
        assert!(nodes[4]
            .smr
            .block_store()
            .unwrap()
            .get_block(proposed_block_id)
            .is_none());
    });

}


#[test]
/// This test checks that the vote of a node and its twin
/// should be counted as duplicate vote (because they have
/// the same public keys)
///
/// Setup:
///
/// 4 honest nodes (n0, n1, n2, n3), and 1 twin (twin0)
///
/// Leader: For each round n0 and n2 are leaders
///
///
/// We split the network as follows:
///      partition 1: n0, n1, twin0
///      partition 2: n2, n3
///
/// Test:
///
/// We need 3 nodes to form a quorum. None of the partitions
/// should be able to form quorum. Partition 1 has 3 nodes
/// but one of them is a twin, and its vote will be counted
/// as duplicate of n0.
///
/// Run the test:
/// cargo xtest -p consensus twins_vote_dedup_test -- --nocapture
#[cfg(test)]
fn twins_vote_dedup_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    let num_nodes = 4;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
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

    // Make n0 and n3 leaders for round 1..50
    for i in 1..50 {
        twins_round_proposers_idx.insert(i, vec![1,2]);
    }

    let mut nodes = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ num_nodes,
        &mut target_nodes,
        &mut playground,
        RoundProposers,
        /* executor_with_reconfig */ false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    // 4 honest nodes
    let n0 = &nodes[0].smr.author();
    let n1 = &nodes[1].smr.author();
    let n2 = &nodes[2].smr.author();
    let n3 = &nodes[3].smr.author();
    // twin of n0
    let twin0 = &nodes[node_to_twin.get(&0).unwrap().to_owned()]
        .smr.
        author();

    debug!("Created nodes:\
    \n n0 -> {:?},\
    \n n1 -> {:?},\
    \n n2 -> {:?},\
    \n n3 -> {:?},\
    \n twin0 -> {:?}", n0,n1,n2,n3,twin0);


    let mut round_partitions: HashMap<u64, Vec<Vec<AccountAddress>>> = HashMap::new();

    for round in 0..50 {
        round_partitions.insert(
            round,
            vec![
                vec![n0.clone(), n1.clone(), twin0.clone()],
                vec![n2.clone(), n3.clone()],
            ],
        );
    }

    playground.split_network_round(&round_partitions);

    block_on(async move {

        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
            .await;

        // Pull enough votes to get a commit on the first block)
        // The proposer's votes are implicit and do not go in the queue.
        let _votes: Vec<VoteMsg> = playground
            .wait_for_messages(50, NetworkPlayground::votes_only::<TestPayload>)
            .await
            .into_iter()
            .map(|(_, msg)| match msg {
                ConsensusMsg::VoteMsg(vote_msg) => *vote_msg,
                _ => panic!("Unexpected message found"),
            })
            .collect();

        // =================
        // Check that the commit logs for nodes in the two partitions
        // are empty
        // =================

        let mut commit_seen = false;

        for i in 0..nodes.len() {

            nodes[i].commit_cb_receiver.close();

            while let Ok(Some(node_commit)) = nodes[i].commit_cb_receiver.try_next() {
                commit_seen = true;
                break;
            }
        }

        assert!(!commit_seen);
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
///
/// We split the network as follows:
///      partition 1: node 0, node 1, node 2
///      partition 2: twin 0, twin 1, node 3
///
/// Test:
///
/// The purpose of this test is to create a safety violation;
/// n2 commits the block proposed by n0, and n3 commits
/// the block proposed by twin0.
///
/// Run the test:
/// cargo xtest -p consensus twins_safety_violation_test -- --nocapture
#[cfg(test)]
fn twins_safety_violation_test() {
    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    let num_nodes = 4;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
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

    let mut nodes = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ num_nodes,
        &mut target_nodes,
        &mut playground,
        RoundProposers,
        /* executor_with_reconfig */ false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    // 4 honest nodes
    let n0 = &nodes[0].smr.author();
    let n1 = &nodes[1].smr.author();
    let n2 = &nodes[2].smr.author();
    let n3 = &nodes[3].smr.author();
    // twin of n0
    let twin0 = &nodes[node_to_twin.get(&0).unwrap().to_owned()]
        .smr.
        author();
    // twin of n1
    let twin1 = nodes[node_to_twin.get(&1).unwrap().to_owned()]
        .smr
        .author();

    let mut round_partitions: HashMap<u64, Vec<Vec<AccountAddress>>> = HashMap::new();

    for round in 0..50 {
        round_partitions.insert(
            /* round */ round,
            vec![
                vec![n1.clone(), n0.clone(), n2.clone()],
                vec![twin0.clone(), twin1.clone(), n3.clone()],
            ],
        );
    }

    playground.split_network_round(&round_partitions);

    block_on(async move {

        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
            .await;

        // Pull enough votes to get a commit on the first block)
        // The proposer's votes are implicit and do not go in the queue.
        let _votes: Vec<VoteMsg> = playground
            .wait_for_messages(100, NetworkPlayground::votes_only::<TestPayload>)
            .await
            .into_iter()
            .map(|(_, msg)| match msg {
                ConsensusMsg::VoteMsg(vote_msg) => *vote_msg,
                _ => panic!("Unexpected message found"),
            })
            .collect();

        // =================
        // Check for all nodes if there are any conflicts in the commit logs
        // =================

        debug!(">>>>> Getting node commit trees for safety check\n");

        let mut all_branches = vec![];

        for i in 0..nodes.len() {

            nodes[i].commit_cb_receiver.close();

            // Node's commited blocks, in order of round numbers
            let mut node_commits = vec![];

            let mut map_node_commits = HashMap::new();
            // Add (round, commited_block) to map. This will also take care of
            // redundancy as some committed blocks appear multiple times in
            // 'commit_cb_receiver',
            let mut highest_round = 0;
            while let Ok(Some(node_commit)) = nodes[i].commit_cb_receiver.try_next() {
                let node_commit_id = node_commit.ledger_info().consensus_block_id();
                let node_commit_round = node_commit.ledger_info().commit_info().round();

                if node_commit_round > highest_round {
                    highest_round = node_commit_round;
                }
                map_node_commits.insert(node_commit_round, node_commit_id);
            }

            // Intialize node_commits with all zeroes.
            // Each index represents (round-1). This is
            // because round 0 is genesis and won't appear in
            // commit_cb_receiver
            for i in (0..highest_round){
                node_commits.push(HashValue::zero());
            }

            for round in map_node_commits.keys().sorted() {
                let idx = round.clone() as usize;
                node_commits[idx - 1] = *map_node_commits.get(round).unwrap();
            }

            all_branches.push(node_commits);
        }

        // Now check if the branches match at all heights
        assert!(is_safe(all_branches));

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
///
/// We split the network as follows:
///      partition 1: n0, n1, n2
///      partition 2: twin 0, twin 1, n3
///
/// Test:
///
/// The purpose of this test is to create a safety violation;
/// n2 commits the block proposed by n0, and n3 commits
/// the block proposed by twin0.
///
/// Run the test:
/// cargo xtest -p consensus twins_safety_violation_scenario_executor_test -- --nocapture
fn twins_safety_violation_scenario_executor_test() {

    let num_nodes = 4;

    // Index #s of nodes (i.e. target nodes) for which we will create twins
    let mut target_nodes = vec![];
    target_nodes.push(0);
    target_nodes.push(1);

    // This helps us map target nodes (for which we will create twins)
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
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

    execute_scenario(
        num_nodes,
        &target_nodes,
        &node_to_twin,
        round_partitions_idx,
        twins_round_proposers_idx,
        true,
    );
}

fn create_partitions(
    playground: &mut NetworkPlayground,
    partitions: HashMap<u64, Vec<Vec<AccountAddress>>>,
) -> bool {
    playground.split_network_round(&partitions)
}



// Compares branches at each height to check if there's any conflict
fn is_safe(branches: Vec<Vec<HashValue>>) -> bool {
    compare_vectors(&branches)
}

// This function compares vectors (of possibly different lengths) at each index
//
// The vectors are equal if they match at each *available* (i.e. non-zero) index
//
// There is a conflict at index i if the value at index i is different
//          at any of the the vectors. Values at conflicting index
//          will be printed out
fn compare_vectors(vecs: &Vec<Vec<HashValue>>) -> bool {

    let (_longest_idx, longest_len) = longest_vector(vecs);

    let mut is_conflict = false;

    // At each index position
    for i in 0..longest_len {
        let mut val: HashValue = HashValue::zero();

        let mut first_time = true;

        // in the vectors to be compared
        for vec in vecs.iter() {
            // Only compare if the index being compared exists
            // (because some vectors might be shorter than the longest),
            if i < vec.len() {
                // and has a non-zero value at the index
                if !vec[i].eq(&HashValue::zero()) {
                    // If this is the first time comparing
                    if first_time {
                        val = vec[i];
                        first_time = false;
                    }
                    // If there's a different val at this index than the one
                    // at the previous vector
                    if !val.eq(&vec[i]) {
                        // then print the conflicting index at all vectors,
                        print_conflict(vecs, i);
                        is_conflict = true;
                        // and move on to the next index
                        break;
                    }
                }
            }
        }
    }

    // Return negation, because the calling function is called 'is_safe'
    let is_safe = !is_conflict;

    is_safe
}

// This function takes a vector of vectors, and for each vector it prints
// out the values at the given conflicting index
fn print_conflict(vecs: &Vec<Vec<HashValue>>, index: usize) {
    debug!("CONFLICT: Index {0} doesn't match, values are:", index);

    for vec in vecs.iter() {
        if index < vec.len() {
            //debug!("{0}", vec[index].id());
            debug!("{0}", vec[index]);
        } else {
            debug!("NULL");
        }
    }
}

// This function takes a vector of vectors, and returns
// (1) the longest one, if the vectors are all of different lengths, OR
// (2) the first longest one, if multiple vectors are of that length
fn longest_vector(vecs: &Vec<Vec<HashValue>>) -> (usize, usize) {
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



/// This is the test executor. It gets as input a scenario description
/// consisting of a node-set, a subset of which are marked target; and
/// a round-by-round message delivery schedule and leaders. The executor
/// sets up a network of nodes with the given number of target nodes
/// (representing byzantine nodes) and per round partitions and leaders.
/// The target nodes correspond to the nodes for which the executor will
/// create twins (i.e. identical instances with the same credentials and
/// signing keys), thereby emulating mis-behavior. The executor runs the
/// BFT protocol among nodes for a pre-specified number of rounds, at
/// the end of which, the executor checks for violations.
fn execute_scenario(
    num_nodes: usize,
    target_nodes: &Vec<usize>, // the nodes for which to create twins
    node_to_twin: &HashMap<usize, usize>,
    round_partitions_idx: HashMap<u64, Vec<Vec<usize>>>,
    twins_round_proposers_idx: HashMap<Round, Vec<usize>>,
    enable_safety_assertion: bool,
) {
    debug!(">>>>> Executing scenario\n");

    debug!(">>>>> Setting up configuration\n");

    let runtime = consensus_runtime();
    let mut playground = NetworkPlayground::new(runtime.handle().clone());

    debug!("Starting nodes and twins");
    let mut nodes = SMRNode::start_num_nodes_with_twins(
        /* num_nodes */ num_nodes,
        &target_nodes,
        &mut playground,
        RoundProposers,
        /* executor_with_reconfig */ false,
        twins_round_proposers_idx,
        &node_to_twin,
    );

    let num_of_rounds = round_partitions_idx.len();

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
                account_addrs.push(nodes[idx.clone()].smr.author());
            }

            round_account_addrs.push(account_addrs);
        }

        round_partitions.insert(round.clone(), round_account_addrs);
    }

    // Create partitions
    create_partitions(&mut playground, round_partitions);

    debug!(">>>>> Starting protocol execution\n");

    // Start sending messages
    block_on(async move {

        let _proposals = playground
            .wait_for_messages(2, NetworkPlayground::proposals_only::<TestPayload>)
            .await;

        // Pull enough votes to get a commit on the first block)
        // The proposer's votes are implicit and do not go in the queue.
        let _votes: Vec<VoteMsg> = playground
            .wait_for_messages(num_nodes * num_of_rounds, NetworkPlayground::votes_only::<TestPayload>)
            .await
            .into_iter()
            .map(|(_, msg)| match msg {
                ConsensusMsg::VoteMsg(vote_msg) => *vote_msg,
                _ => panic!("Unexpected message found"),
            })
            .collect();

    });

    // =================
    // Check for all nodes if there are any conflicts in the commit logs
    // =================

    debug!(">>>>> Getting node commit trees for safety check\n");

    let mut all_branches = vec![];

    for i in 0..nodes.len() {

        nodes[i].commit_cb_receiver.close();

        // Node's commited blocks, in order of round numbers
        let mut node_commits = vec![];

        let mut map_node_commits = HashMap::new();
        // Add (round, commited_block) to map. This will also take care of
        // redundancy as some committed blocks appear multiple times in
        // 'commit_cb_receiver',
        let mut highest_round = 0;
        while let Ok(Some(node_commit)) = nodes[i].commit_cb_receiver.try_next() {
            let node_commit_id = node_commit.ledger_info().consensus_block_id();
            let node_commit_round = node_commit.ledger_info().commit_info().round();

            if node_commit_round > highest_round {
                highest_round = node_commit_round;
            }
            map_node_commits.insert(node_commit_round, node_commit_id);
        }

        // Intialize node_commits with all zeroes.
        // Each index represents (round-1). This is
        // because round 0 is genesis and won't appear in
        // commit_cb_receiver
        for i in (0..highest_round){
            node_commits.push(HashValue::zero());
        }

        for round in map_node_commits.keys().sorted() {
            let idx = round.clone() as usize;
            node_commits[idx - 1] = *map_node_commits.get(round).unwrap();
        }

        all_branches.push(node_commits);
    }


    debug!(">>>>> Performing safety check\n");

    // Now check if the branches match at all heights
    if enable_safety_assertion {
        assert!(is_safe(all_branches));
    } else {
        is_safe(all_branches);
    }

    debug!(">>>>> Stopping nodes\n");

    // Stop all the nodes
    for each_node in nodes {
        each_node.stop();
    }

    // TODO: We should probably also shut down the network playground here,
    // but there is no existing function to help with that. If memory usage is
    // high with repeated calls to this function and some resources / processes
    // outliving this function, we will have to implement this.
    // We tried shutting down runtime---it only adds the 'timeout'
    // overhead without any real benefits in terms of test execution
    //runtime.shutdown_timeout(Duration::from_millis(100));

    debug!(">>>>> Finished test!\n");

}



// A memory-inefficient implementation of all solutions of Stirling number
// of second kind (https://en.wikipedia.org/wiki/Stirling_numbers_of_the_second_kind).
// This will probably not work for n > 20.
fn stirling2(n: usize, k: usize) -> Vec<Vec<Vec<usize>>> {
    if k == 1 {
        return vec![vec![(0..n).collect()]];
    } else if n == k {
        let mut ret = vec![vec![]];
        for i in 0..n {
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
        for _i in 0..k {
            k_s_n_1_k.extend(tmp.iter().cloned());
        }
        let size = k * tmp.len();
        for i in 0..size {
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


/// This function filters an input vector down to 'n' elements. How these 'n' elements
/// are selected  depends on the value of 'parameter':
///     0: Pick the first n elements (deterministic)
///     1: Randomly pick n elements without repetitions
///     2: Randomly pick n elements with repetition
fn filter_n_elements<T: Clone> (
    list: &mut Vec<T>,
    n: usize,
    parameter: usize
){
    match parameter {
        // deterministically truncate (if necessary)
        0 => {
            if n < list.len() {
                list.truncate(n)
            }
        },
        // pick without replacement
        1  => {
            if n >= list.len() {
                return;
            }
            let original_list = list.clone();
            let mut rng = rand::thread_rng();
            let mut dice = rng.gen_range(0, original_list.len());
            let mut seen = Vec::new();
            // Pick n random indices, copy their value to beginning of list
            // and truncate rest of list
            for i in 0..n {
                list[i] = original_list[dice].clone();
                seen.push(dice);
                // Repeatedly roll the dice until we get an unseen value
                while seen.contains(&dice) {
                    dice = rng.gen_range(0, original_list.len());
                }
            }
            list.truncate(n);
        },
        // pick with replacement
        2 => {
            let original_list = list.clone();
            // extend the list if necessary
            while list.len() < n {
                list.push(list[0].clone());
            }
            let mut rng = rand::thread_rng();
            let mut dice = rng.gen_range(0, original_list.len());
            // Pick n random indices, copy their value to beginning of list
            // and truncate rest of list
            for i in 0..n {
                list[i] = original_list[dice].clone();
                dice = rng.gen_range(0, original_list.len());
            }
            list.truncate(n);
        }
        _ => assert!(false)
    }
}


#[test]
/// This function is the test generator. It produces various test cases to be
/// fed into the test executor (execute_scenario()). Each test case represents
/// a unique instance of executor configuration parameters, i.e. the target
/// nodes and per round network partitions and leaders. The generator iterates
/// over the generated test cases linearly, and invokes the executor for each
/// test case.
///
/// run:
/// cargo xtest -p consensus twins_test_safety_attack_generator -- --nocapture
///
/// To compute the time it take to run the measurements, run:
/// time cargo xtest -p consensus twins_test_safety_attack_generator
///
fn twins_test_safety_attack_generator() {
    const NUM_OF_ROUNDS: usize = 4; // FIXME: Tweak this parameter
    const NUM_OF_NODES: usize = 4; // FIXME: Tweak this parameter
    const NUM_OF_PARTITIONS: usize = 2; // FIXME: Tweak this parameter

    // If true will not execute scenarios, just print stats
    const IS_DRY_RUN: bool = false; // FIXME: Tweak this parameter


    // The parameters FILTER_X_PARTITIONS and OPTION_FILTER_X_PARTITIONS let
    // us select X partition scenarios from all possible scenarios (i.e. ways
    // in which N nodes can be partitioned into P partitions).
    //
    //
    // FILTER_X_PARTITIONS: Indicates how many partition scenarios to pick;
    // setting this parameter to 0 disables this filter.
    //
    // OPTION_FILTER_X_PARTITIONS: Indicates how to select those X samples;
    // it can take the following values:
    //     0: To simply select the first X partition scenarios (deterministic)
    //     1: To randomly pick *without* replacement X partition scenarios (probabilistic)
    //     2: To randomly pick *with* replacement X partition scenarios (probabilistic)
    const FILTER_X_PARTITIONS: usize = 0; // FIXME: Tweak this parameter
    const OPTION_FILTER_X_PARTITIONS: usize = 0; // FIXME: Tweak this parameter


    // The parameters FILTER_Y_PARTITIONS_WITH_LEADERS and
    // OPTION_FILTER_Y_PARTITIONS_WITH_LEADERS let us select Y testcases after
    // we have combined partition scenarios with all possible leaders.
    //
    // FILTER_Y_PARTITIONS_WITH_LEADERS: Indicates how many 'scenario-leader'
    // combinations to pick; setting this parameter to 0 disables this filter.
    //
    // OPTION_FILTER_Y_PARTITIONS_WITH_LEADERS: Indicates how to select those Y
    // samples; it can take the following values:
    //     0: To simply select the first Y testcases (deterministic)
    //     1: To randomly pick *without replacement* Y testcases (probabilistic)
    //     2: To randomly pick *with replacement* Y testcases (probabilistic)
    const FILTER_Y_PARTITIONS_WITH_LEADERS: usize = 2; // FIXME: Tweak this parameter
    const OPTION_FILTER_Y_PARTITIONS_WITH_LEADERS: usize = 0; // FIXME: Tweak this parameter


    // OPTION_TESTCASE_GENERATOR lets us choose how to distribute 'scenario-leaders'
    // combinations over R rounds.
    //
    // It can take the following values:
    //     0: Permute *without replacement* 'scenario-leaders' combinations over
    //     R rounds. This requires that there are equal or more scenario-leaders
    //     than the number of rounds.
    //
    //     1: Permute *with replacement* 'scenario-leaders' over R rounds.
    //     Note that this creates 2^n testcases, so it is advised to filter
    //     'scenario-leaders' using FILTER_Y_PARTITIONS_WITH_LEADERS
    //
    //     2: This mode generates 1,000 random testcases. It attributes to each
    //     round a 'scenario-leader' combination that is randomly picked *with
    //     replacement* from all possible 'scenario-leaders'; it keeps picking them
    //     until it generated a total of 1,000 testcases.
    const OPTION_TESTCASE_GENERATOR: usize = 1; // FIXME: Tweak this parameter



    // The parameters FILTER_Z_TEST_CASES and OPTION_FILTER_Z_TEST_CASES let
    // us choose Z testcases from all testcases right before starting execution;
    // i.e., after combining the 'scenario-leaders' with the rounds.
    //
    // FILTER_Z_TESTCASES: Indicates how many testcases to pick; setting this
    // parameter to 0 disables this filter.
    //
    // OPTION_FILTER_Z_TESTCASES: Indicates how to select those Z samples;
    // it can take the following values:
    //     0: To simply select the first Z testcases (deterministic)
    //     1: To randomly pick *without replacement* Z testcases (probabilistic)
    //     2: To randomly pick *with replacement* Z testcases (probabilistic)
    const FILTER_Z_TESTCASES: usize = 0; // FIXME: Tweak this parameter
    const OPTION_FILTER_Z_TESTCASES: usize = 0; // FIXME: Tweak this parameter


    let start = Instant::now();

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
    //  |-------------------------------------------------------------------------|
    //  | 0 ... f-1    | f ... NUM_OF_NODES-1 | NUM_OF_NODES ... NUM_OF_NODES+f-1 |
    //  |+++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++|
    //  | target_nodes |    honest_nodes      |            twin_nodes             |
    //  |-------------------------------------------------------------------------|
    //
    // =============================================

    let mut old_list_length; // used later as a temporary variable
    let f = (NUM_OF_NODES - 1) / 3;
    let mut nodes: Vec<usize> = Vec::new();

    // First fill the target nodes. By convention, the first nodes are the target nodes.
    let target_nodes: Vec<usize> = (0..f).collect();
    nodes.append(&mut target_nodes.clone());

    // Then, add the other (honest) nodes
    let honest_nodes: Vec<usize> = (f..NUM_OF_NODES).collect();
    nodes.append(&mut honest_nodes.clone());

    // Finally, add the twins of the target nodes
    let twin_nodes: Vec<usize> = (NUM_OF_NODES..NUM_OF_NODES + f).collect();
    nodes.append(&mut twin_nodes.clone());

    // This data structure is required by `execute_scenario()` which we
    // call at the end to execute generated tests.
    // node_to_twin` maps `target_nodes` to twins indices in 'nodes'
    // to corresponding twin indices in 'nodes'. For example, we get
    // twin for nodes[1] as follows:  nodes[node_to_twin.get(1)]
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
        "There are {:?} ways to allocate {:?} nodes ({:?} honest nodes + {:?} twins) \
        into {:?} partitions.",
        partition_scenarios.len(), nodes.len(), NUM_OF_NODES-f, f, NUM_OF_PARTITIONS
    );

    if FILTER_X_PARTITIONS != 0 {
        assert!(FILTER_X_PARTITIONS > 0);
        old_list_length =  partition_scenarios.len();
        filter_n_elements(
            &mut partition_scenarios,
            FILTER_X_PARTITIONS,
            OPTION_FILTER_X_PARTITIONS
        );
        println!(
            "After filtering we have {:?} partition scenarios: \
            we filtered out {:?} scenarios.",
            partition_scenarios.len(),
            old_list_length - partition_scenarios.len()
        );
    }

    // =============================================
    //
    // Assign leaders to partition scenarios.
    //
    // We don't consider honest_nodes as leaders, only target_nodes and their twins.
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
    // Notation: First vector is the leader pair (target node and its twin),
    // second vector is the partition scenario
    // {
    //  { {2,22}, {{0, 22, 1}, {2, 11}} },
    //  { {1,11}, {{0, 22, 1}, {2, 11}} }, } etc.
    //
    // Note: For cases where a leader is not part of the corresponding partition,
    //  e.g. {2, {0, 22, 1}}, the leader just ends up not being able to propose
    //  anything at all
    //

    // In `partition_scenarios_with_leaders` each element is a leader pair of
    // (target node and its twin) and partition scenario
    let mut partition_scenarios_with_leaders = Vec::new();

    // Note: We only add target_nodes as leaders here, and the twin_node corresponding
    // to target_nodes will be added as a leader implicitly by the scenario executor
    for each_leader in &target_nodes {
        for each_scenario in &partition_scenarios {
            let pair = (each_leader, each_scenario.clone());
            partition_scenarios_with_leaders.push(pair);
        }
    }

    // Don't need 'partition_scenarios' any more
    partition_scenarios.clear();
    println!(
        "After combining leaders with partition scenarios, we have {:?} scenario-leader \
        combinations.",
        partition_scenarios_with_leaders.len()
    );

    // =============================================
    // We now have all possible partition-leader scenarios. Next we
    // find how to arrange these scenarios across NUM_OF_ROUNDS rounds.
    // =============================================

    if FILTER_Y_PARTITIONS_WITH_LEADERS != 0 {
        assert!(FILTER_Y_PARTITIONS_WITH_LEADERS > 0);
        old_list_length = partition_scenarios_with_leaders.len();
        filter_n_elements(
            &mut partition_scenarios_with_leaders,
            FILTER_Y_PARTITIONS_WITH_LEADERS,
            OPTION_FILTER_Y_PARTITIONS_WITH_LEADERS
        );
        println!(
            "After filtering we have {:?} scenario-leader combinations: \
            we filtered out {:?} scenarios.",
            partition_scenarios_with_leaders.len(),
            old_list_length - partition_scenarios_with_leaders.len()
        );
    }

    // Permutation of n objects into r places, P(n,r) requires that n >= r.
    // Informally, number of rounds should be less than scenarios, otherwise
    // we need to repeat the same scenarios for multiple rounds which is
    // "permutations with replacement" (OPTION_TESTCASE_GENERATOR = 2) .
    let mut test_cases = Vec::new();
    if OPTION_TESTCASE_GENERATOR == 0 {
        assert!(partition_scenarios_with_leaders.len() >= NUM_OF_ROUNDS);
        println!(
            "Now generating test cases by permuting (without replacement) {:?} \
            scenario-leader combinations over {:?} rounds.",
            partition_scenarios_with_leaders.len(),
            NUM_OF_ROUNDS
        );

        let permutations = partition_scenarios_with_leaders
            .iter()
            .permutations(NUM_OF_ROUNDS);
        for perm in permutations {
            test_cases.push(perm)
        }
    }
    else if OPTION_TESTCASE_GENERATOR == 1 {
        assert!(partition_scenarios_with_leaders.len() > 0);
        println!(
            "Now generating test cases by permuting (with replacement) {:?} \
            scenario-leader combinations over {:?} rounds",
            partition_scenarios_with_leaders.len(),
            NUM_OF_ROUNDS
        );

        let length = partition_scenarios_with_leaders.len();

        if NUM_OF_ROUNDS == 4 {
            for (i1, i2, i3, i4) in iproduct!(0..length, 0..length, 0..length, 0..length) {
               let mut each_test = Vec::new();
               each_test.push(&partition_scenarios_with_leaders[i1]);
               each_test.push(&partition_scenarios_with_leaders[i2]);
               each_test.push(&partition_scenarios_with_leaders[i3]);
               each_test.push(&partition_scenarios_with_leaders[i4]);
               test_cases.push(each_test);
            }
        }
        else if NUM_OF_ROUNDS == 7 {
            for (i1, i2, i3, i4, i5, i6, i7) in iproduct!(
                0..length, 0..length, 0..length, 0..length, 0..length, 0..length, 0..length
            ) {
               let mut each_test = Vec::new();
               each_test.push(&partition_scenarios_with_leaders[i1]);
               each_test.push(&partition_scenarios_with_leaders[i2]);
               each_test.push(&partition_scenarios_with_leaders[i3]);
               each_test.push(&partition_scenarios_with_leaders[i4]);
               each_test.push(&partition_scenarios_with_leaders[i5]);
               each_test.push(&partition_scenarios_with_leaders[i6]);
               each_test.push(&partition_scenarios_with_leaders[i7]);
               test_cases.push(each_test);
            }
        }
        else {
            assert!(false);
        }

        // Make sure we got the right number of test cases
        assert_eq!(
            test_cases.len(),
            partition_scenarios_with_leaders.len().checked_pow(NUM_OF_ROUNDS as u32).unwrap()
        );
    }
    else if OPTION_TESTCASE_GENERATOR == 2 {
        const MAX_TESTCASES: usize = 1_000;
        println!(
            "Now generating {:?} test cases by picking (with replacement) {:?} \
            scenario-leader combinations ({:?} rounds x {:?} test cases \
            = {:?})",
            MAX_TESTCASES,
            NUM_OF_ROUNDS*MAX_TESTCASES,
            NUM_OF_ROUNDS,
            MAX_TESTCASES,
            NUM_OF_ROUNDS*MAX_TESTCASES
        );

        let mut rng = rand::thread_rng();
        for i in 0..MAX_TESTCASES {
            let mut each_test = Vec::new();
            for j in 0..NUM_OF_ROUNDS {
                let mut dice = rng.gen_range(0, partition_scenarios_with_leaders.len());
                each_test.push(&partition_scenarios_with_leaders[dice]);
            }
            test_cases.push(each_test);
        }
    }
    else {
        println!("{:?} is an invalid OPTION_TESTCASE_GENERATOR", OPTION_TESTCASE_GENERATOR);
        assert!(false)
    }

    println!("We have generated {:?} testcases.", test_cases.len());
    if FILTER_Z_TESTCASES != 0 {
        assert!(FILTER_Z_TESTCASES > 0);
        old_list_length = test_cases.len();
        filter_n_elements(
            &mut test_cases,
            FILTER_Z_TESTCASES,
            OPTION_FILTER_Z_TESTCASES
        );
        println!(
            "After filtering, we have {:?} testcases: we filtered out {:?} testcases.",
            test_cases.len(),
            old_list_length - test_cases.len()
        );
    }


    // =============================================
    // Now we are ready to prepare and execute each scenario via the executor
    // =============================================

    let mut round = 1;
    let mut num_test_cases = 1;

    for each_test in test_cases {

        if num_test_cases > 0 {
            if (!IS_DRY_RUN) {
                println!("=====================================");
                println!("TEST CASE {:?}:  {:?}", num_test_cases, &each_test);
                println!("=====================================");
            }

            // Creating data structures that specify round-by-round network partitions
            // and leaders
            //
            // Maps round to partitions (nodes are expressed in terms of their indices)
            let mut round_partitions_idx = HashMap::new();
            // Maps round to leaders (nodes are expressed in terms of their indices)
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

            if (!IS_DRY_RUN) {
                let start_execution = Instant::now();
                execute_scenario(
                    NUM_OF_NODES,
                    &target_nodes,
                    &node_to_twin,
                    round_partitions_idx,      // this changes for each test
                    twins_round_proposers_idx, // this changes for each test
                    true,
                );
                let execution_duration = start_execution.elapsed();
                println!(
                    "Time elapsed for execution only is: {:?} ms",
                    execution_duration.as_millis()
                );
            }
            num_test_cases += 1;
        }
    }

    println!(
        "\nFinished running total {:?} test cases for {:?} nodes, {:?} twins, \
         {:?} rounds and {:?} partitions\n",
        num_test_cases-1, NUM_OF_NODES, f, NUM_OF_ROUNDS, NUM_OF_PARTITIONS
    );
}
