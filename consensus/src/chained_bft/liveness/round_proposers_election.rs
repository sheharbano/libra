// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::chained_bft::liveness::proposer_election::ProposerElection;
use consensus_types::{
    block::Block,
    common::{Author, Payload, Round},
};

use libra_logger::prelude::*;
use std::collections::HashMap;

/// The round proposer maps a round to author(s)
pub struct RoundProposers {
    // This is a pre-defined map, specified in
    // 'consensus/src/chained_bft/chained_bft_smr_test.rs'
    proposers: Option<HashMap<Round, Vec<Author>>>,
    // Default leader to use if proposers is None
    // The leader election then becomes FixedProposer in practice.
    // We hardcode this to the first validator (smallest index = 0)
    default_proposer: Author,
}

impl RoundProposers {
    /// With only one proposer in the vector, it behaves the same as a fixed proposer strategy.
    pub fn new(proposers: Option<HashMap<Round, Vec<Author>>>, default_proposer: Author) -> Self {
        Self {
            proposers,
            // We hardcode this to the first validator (smallest index = 0)
            default_proposer,
        }
    }

    fn get_proposers(&self, round: Round) -> Vec<Author> {
        match &self.proposers {
            None => vec![self.default_proposer],
            Some(map) => {
                // If the round has not been specified in the map then
                // return default proposer, else return the corresponding
                // proposer(s)
                match map.get(&round) {
                    None => vec![self.default_proposer],
                    Some(round_proposers) => round_proposers.to_vec(),
                }
            }
        }
    }
}

impl<T: Payload> ProposerElection<T> for RoundProposers {
    fn is_valid_proposer(&self, author: Author, round: Round) -> Option<Author> {
        if self.get_proposers(round).contains(&author) {
            Some(author)
        } else {
            None
        }
    }

    fn get_valid_proposers(&self, round: Round) -> Vec<Author> {
        self.get_proposers(round)
    }

    fn process_proposal(&mut self, proposal: Block<T>) -> Option<Block<T>> {
        let author = proposal.author()?;
        let round = proposal.round();
        let proposers = self.get_proposers(round);
        for proposer in proposers {
            if author == proposer {
                debug!(
                    "Primary proposal {0} by {1}: going to process it right now.",
                    proposal, author
                );
                return Some(proposal);
            }
        }
        warn!(
            "Proposal {} does not match any candidate for round {}, ignore.",
            proposal, round
        );

        None
    }

    fn take_backup_proposal(&mut self, _round: Round) -> Option<Block<T>> {
        None
    }
}
