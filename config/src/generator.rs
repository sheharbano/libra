// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

//! Convenience structs and functions for generating a random set of Libra ndoes without the
//! genesis.blob.

use crate::{
    config::{
        NodeConfig, OnDiskStorageConfig, SafetyRulesBackend, SafetyRulesService, SeedPeersConfig,
        TestConfig,
    },
    utils,
};
use libra_types::{validator_info::ValidatorInfo, validator_set::ValidatorSet};
use rand::{rngs::StdRng, SeedableRng};
use libra_types::account_address::AccountAddress;
use std::{convert::TryFrom};

pub struct ValidatorSwarm {
    pub nodes: Vec<NodeConfig>,
    pub validator_set: ValidatorSet,
}

pub fn validator_swarm(
    template: &NodeConfig,
    count: usize,
    seed: [u8; 32],
    randomize_ports: bool,
) -> ValidatorSwarm {
    let mut rng = StdRng::from_seed(seed);
    let mut validator_keys = Vec::new();
    let mut nodes = Vec::new();

    for _index in 0..count {
        let mut node = NodeConfig::random_with_template(template, &mut rng);
        if randomize_ports {
            node.randomize_ports();
        }

        let mut storage_config = OnDiskStorageConfig::default();
        storage_config.default = true;
        node.consensus.safety_rules.service = SafetyRulesService::Thread;
        node.consensus.safety_rules.backend = SafetyRulesBackend::OnDiskStorage(storage_config);

        let network = node.validator_network.as_mut().unwrap();
        network.listen_address = utils::get_available_port_in_multiaddr(true);
        network.advertised_address = network.listen_address.clone();

        let test = node.test.as_ref().unwrap();
        let consensus_pubkey = test.consensus_keypair.as_ref().unwrap().public().clone();
        let network_keypairs = network
            .network_keypairs
            .as_ref()
            .expect("Network keypairs are not defined");

        validator_keys.push(ValidatorInfo::new(
            network.peer_id,
            consensus_pubkey,
            1, // @TODO: Add support for dynamic weights
            network_keypairs.signing_keys.public().clone(),
            network_keypairs.identity_keys.public().clone(),
        ));

        nodes.push(node);
    }

    let mut seed_peers = SeedPeersConfig::default();
    let network = nodes[0].validator_network.as_ref().unwrap();
    seed_peers
        .seed_peers
        .insert(network.peer_id, vec![network.listen_address.clone()]);

    for node in &mut nodes {
        let network = node.validator_network.as_mut().unwrap();
        network.seed_peers = seed_peers.clone();
    }

    validator_keys.sort_by(|k1, k2| k1.account_address().cmp(k2.account_address()));
    ValidatorSwarm {
        nodes,
        validator_set: ValidatorSet::new(validator_keys),
    }
}

pub fn validator_swarm_for_testing(nodes: usize) -> ValidatorSwarm {
    let mut config = NodeConfig::default();
    config.test = Some(TestConfig::open_module());
    validator_swarm(&NodeConfig::default(), nodes, [1u8; 32], true)
}

#[cfg(any(test, feature = "fuzzing"))]
/// Creates a network of nodes, with twins created for target_nodes
pub fn validator_swarm_twins(
    template: &NodeConfig,
    count: usize,
    seed: [u8; 32],
    randomize_ports: bool,
    target_nodes: Vec<usize>,
) -> ValidatorSwarm {
    let mut rng = StdRng::from_seed(seed);
    let mut validator_keys = Vec::new();
    let mut nodes = Vec::new();

    let validator_keys_twins = Vec::new();
    let mut nodes_twins = Vec::new();
    // Starting index for twin account addresses (this will appear in
    // logs). We choose 240 (hex: f0), so twins logs will be prefixed by
    // f0 (and onwards)
    let mut twin_account_index = 240;

    // =================
    // Creating nodes
    // =================

    for _index in 0..count {
        let mut node = NodeConfig::random_with_template(template, &mut rng);
        if randomize_ports {
            node.randomize_ports();
        }

        let mut storage_config = OnDiskStorageConfig::default();
        storage_config.default = true;
        node.consensus.safety_rules.backend = SafetyRulesBackend::OnDiskStorage(storage_config);

        let network = node.validator_network.as_mut().unwrap();
        network.listen_address = utils::get_available_port_in_multiaddr(true);
        network.advertised_address = network.listen_address.clone();

        let test = node.test.as_ref().unwrap();
        let consensus_pubkey = test.consensus_keypair.as_ref().unwrap().public().clone();
        let network_keypairs = network
            .network_keypairs
            .as_ref()
            .expect("Network keypairs are not defined");

        validator_keys.push(ValidatorInfo::new(
            network.peer_id,
            consensus_pubkey,
            1, // @TODO: Add support for dynamic weights
            network_keypairs.signing_keys.public().clone(),
            network_keypairs.identity_keys.public().clone(),
        ));

        // We will use this to instantiate twins below
        //let node_copy = node.clone_everything();
        let node_copy = node.clone();

        let test_original = node_copy.test.unwrap();

        nodes.push(node);


        // ==============
        // To be executed if the node is a target node for which we'll create twin
        // ==============

        // For twin, we will copy everything same as target node, except networking
        // info and account address.
        if target_nodes.contains(&_index) {

            // --------------------------------
            // Set the twin's account address
            //
            // Explanation: At the consensus layer routing decisions are
            // made based on account addresses (see relevant functions in
            // "consensus/src/chained_bft/network.rs" such as "send_vote"
            // and "broadcast"---they all use author, which is the same as an
            // account address, to identify the destination node of a message).
            // We want to give the twin its own unique account address so we
            // can decide which messages to send to the twin vs the target node
            // --------------------------------

            let mut twin_address = [0; AccountAddress::LENGTH];
            // Usually account address is hash of node's public key, but for
            // testing we generate account addresses that are more readable.
            // So "twin_address" below will appear in the first byte of the
            // "AccountAddress" generated by "AccountAddress::try_from"
            twin_address[0] = twin_account_index;
            twin_account_index += 1;

            let twin_account_address = AccountAddress::try_from(&twin_address[..]).unwrap();

            // ----------------------
            // Create the twin node
            // ----------------------

            // Get a 'NodeConfig' with the credentials of the target node and
            // the given twin account address; other info (such as networking)
            // to be generated randomly as usual (i.e. as defined in 'template')
            let mut node_twin = NodeConfig::random_with_test_and_account(template, &mut rng, test_original, twin_account_address);

            if randomize_ports {
                node_twin.randomize_ports();
            }

            let mut storage_config = OnDiskStorageConfig::default();
            storage_config.default = true;
            node_twin.consensus.safety_rules.backend = SafetyRulesBackend::OnDiskStorage(storage_config);

            let network = node_twin.validator_network.as_mut().unwrap();
            network.listen_address = utils::get_available_port_in_multiaddr(true);
            network.advertised_address = network.listen_address.clone();

            let test = node_twin.test.as_ref().unwrap();
            let consensus_pubkey = test.consensus_keypair.as_ref().unwrap().public().clone();
            let network_keypairs = network
                .network_keypairs
                .as_ref()
                .expect("Network keypairs are not defined");

            validator_keys.push(ValidatorInfo::new(
                twin_account_address,
                consensus_pubkey,
                1,
                network_keypairs.signing_keys.public().clone(),
                network_keypairs.identity_keys.public().clone(),
            ));

            nodes_twins.push(node_twin);
        }
    }

    // Some tests make assumptions about the ordering of configs in relation
    // to the FixedProposer which should be the first proposer in lexical order.
    // We apply the expected ordering only to non-twin nodes, because in spirit
    // the protocol is oblivious to their existence
    nodes.sort_by(|a, b| {
        let a_auth = a.validator_network.as_ref().unwrap().peer_id;
        let b_auth = b.validator_network.as_ref().unwrap().peer_id;
        a_auth.cmp(&b_auth)
    });

    validator_keys.sort_by(|k1, k2| k1.account_address().cmp(k2.account_address()));


    // ==============
    // Now add twins
    // ==============

    for each in validator_keys_twins {
        validator_keys.push(each);
    }

    for each in nodes_twins {
        nodes.push(each);
    }

    // ==========
    // Set seed peers
    // ===========

    let mut seed_peers = SeedPeersConfig::default();
    let network = nodes[0].validator_network.as_ref().unwrap();
    seed_peers
        .seed_peers
        .insert(network.peer_id, vec![network.listen_address.clone()]);

    for node in &mut nodes {
        let network = node.validator_network.as_mut().unwrap();
        network.seed_peers = seed_peers.clone();
    }


    let mut validator_set = ValidatorSet::new(validator_keys);
    // Tell ValidatorSet about the twins, so they can be ignored in
    // calculation of quorum voting power.
    //validator_set.set_num_twins(target_nodes.len());

    ValidatorSwarm {
        nodes,
        validator_set,
    }
}

#[cfg(any(test, feature = "fuzzing"))]
/// Creates a network of nodes, with twins created for target_nodes
pub fn validator_swarm_for_testing_twins(nodes: usize, target_nodes: Vec<usize>) -> ValidatorSwarm {
    let mut config = NodeConfig::default();
    config.test = Some(TestConfig::open_module());
    validator_swarm_twins(&NodeConfig::default(), nodes, [1u8; 32], true, target_nodes)
}
