// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::config::SafetyRulesConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use std::collections::HashMap;
use libra_types::account_address::AccountAddress;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConsensusConfig {
    pub max_block_size: u64,
    pub proposer_type: ConsensusProposerType,
    pub contiguous_rounds: u32,
    pub max_pruned_blocks_in_mem: usize,
    pub pacemaker_initial_timeout_ms: u64,
    pub safety_rules: SafetyRulesConfig,
    // round_to_proposers: Used to specify leader(s) per round.
    // Motivated by Twins, but can be used generally.
    // Note: u64 represents the round.
    pub round_to_proposers: Option<HashMap<u64, Vec<AccountAddress>>>
}

impl Default for ConsensusConfig {
    fn default() -> ConsensusConfig {
        ConsensusConfig {
            max_block_size: 1000,
            proposer_type: ConsensusProposerType::MultipleOrderedProposers,
            contiguous_rounds: 2,
            max_pruned_blocks_in_mem: 10000,
            // Bano: Change timeout to 100 ms
            //pacemaker_initial_timeout_ms: 1000,
            pacemaker_initial_timeout_ms: 100,
            safety_rules: SafetyRulesConfig::default(),
            round_to_proposers: None::<HashMap<u64, Vec<AccountAddress>>>
        }
    }
}

impl ConsensusConfig {
    pub fn set_data_dir(&mut self, data_dir: PathBuf) {
        self.safety_rules.set_data_dir(data_dir);
    }

    /// Sets the `round_to_proposers' field of the struct ConsensusConfig
    pub fn set_round_to_proposers(
        &mut self,
        round_to_proposers_map: HashMap<u64, Vec<AccountAddress>>,
    ) {
        self.round_to_proposers = Some(round_to_proposers_map);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsensusProposerType {
    // Choose the smallest PeerId as the proposer
    FixedProposer,
    // Round robin rotation of proposers
    RotatingProposer,
    // Multiple ordered proposers per round (primary, secondary, etc.)
    MultipleOrderedProposers,
    // Pre-specified mapping between rounds and proposers
    // (defaults to the first validator if there's no proposer for a round)
    RoundProposers,
}
