// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::chained_bft::liveness::{
    proposer_election::ProposerElection, round_proposers_election::RoundProposers,
};
use consensus_types::{block::block_test_utils::certificate_for_genesis, block::Block};
use libra_types::validator_signer::ValidatorSigner;

use consensus_types::common::Round;
use libra_types::account_address::AccountAddress;
use std::collections::HashMap;

#[test]
fn test_round_proposers() {
    let first_validator_signer = ValidatorSigner::random([0u8; 32]);
    let first_author = first_validator_signer.author();

    let second_validator_signer = ValidatorSigner::random([1u8; 32]);
    let second_author = second_validator_signer.author();

    let third_validator_signer = ValidatorSigner::random([2u8; 32]);
    let third_author = third_validator_signer.author();

    // A map that tells who is the proposer(s) per round
    let mut round_proposers: HashMap<Round, Vec<AccountAddress>> = HashMap::new();
    // Both first and second authors are leaders of round 1
    round_proposers.insert(1, vec![first_author, second_author]);
    // Only third author is leader of round 2
    round_proposers.insert(2, vec![third_author]);

    let mut pe: Box<dyn ProposerElection<u32>> =
        Box::new(RoundProposers::new(Some(round_proposers), first_author));

    // Send a proposal from both first author and second author, the only winning proposals
    // should follow the round-to-author mapping

    // Test genesis and the next block
    let quorum_cert = certificate_for_genesis();

    // The function new_proposal asks for the following parameters
    /*
        pub fn new_proposal(
        payload: T,
        round: Round,
        timestamp_usecs: u64,
        quorum_cert: QuorumCert,
        validator_signer: &ValidatorSigner,
    )
    */

    let good_proposal_round1_1 =
        Block::new_proposal(1, 1, 1, quorum_cert.clone(), &first_validator_signer);

    let good_proposal_round1_2 =
        Block::new_proposal(2, 1, 1, quorum_cert.clone(), &second_validator_signer);

    let bad_proposal_round1 =
        Block::new_proposal(2, 1, 2, quorum_cert.clone(), &third_validator_signer);

    let good_proposal_round2 =
        Block::new_proposal(3, 2, 2, quorum_cert.clone(), &third_validator_signer);

    // ==============
    // Testing "is_valid_proposer"
    // ==============

    assert_eq!(pe.is_valid_proposer(first_author, 1), Some(first_author));
    assert_eq!(pe.is_valid_proposer(second_author, 1), Some(second_author));
    assert_eq!(pe.is_valid_proposer(third_author, 1), None);

    assert_eq!(pe.is_valid_proposer(third_author, 2), Some(third_author));
    assert_eq!(pe.is_valid_proposer(second_author, 2), None);

    // ==============
    // Testing "get_valid_proposers"
    // ==============

    assert_eq!(pe.get_valid_proposers(1), vec![first_author, second_author]);

    assert_eq!(pe.get_valid_proposers(2), vec![third_author]);

    // ==============
    // Testing "process_proposal"
    // ==============

    assert_eq!(
        pe.process_proposal(good_proposal_round1_1.clone()),
        Some(good_proposal_round1_1)
    );

    assert_eq!(
        pe.process_proposal(good_proposal_round1_2.clone()),
        Some(good_proposal_round1_2)
    );

    assert_eq!(pe.process_proposal(bad_proposal_round1.clone()), None);

    assert_eq!(
        pe.process_proposal(good_proposal_round2.clone()),
        Some(good_proposal_round2)
    );
}
