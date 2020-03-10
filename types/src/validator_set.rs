// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::{
    access_path::{AccessPath, Accesses},
    account_config,
    event::EventKey,
    identifier::{IdentStr, Identifier},
    language_storage::StructTag,
    validator_public_keys::ValidatorPublicKeys,
};
use anyhow::{Error, Result};
use libra_crypto::VerifyingKey;
use once_cell::sync::Lazy;
#[cfg(any(test, feature = "fuzzing"))]
use proptest_derive::Arbitrary;
use serde::{Deserialize, Serialize};
use std::{
    convert::{TryFrom, TryInto},
    fmt,
    iter::IntoIterator,
    ops::Deref,
    vec,
};

use std::collections::HashMap;
use crate::{account_address::AccountAddress};

static LIBRA_SYSTEM_MODULE_NAME: Lazy<Identifier> =
    Lazy::new(|| Identifier::new("LibraSystem").unwrap());
static VALIDATOR_SET_STRUCT_NAME: Lazy<Identifier> =
    Lazy::new(|| Identifier::new("ValidatorSet").unwrap());

pub fn validator_set_module_name() -> &'static IdentStr {
    &*LIBRA_SYSTEM_MODULE_NAME
}

pub fn validator_set_struct_name() -> &'static IdentStr {
    &*VALIDATOR_SET_STRUCT_NAME
}

pub fn validator_set_tag() -> StructTag {
    StructTag {
        name: validator_set_struct_name().to_owned(),
        address: account_config::CORE_CODE_ADDRESS,
        module: validator_set_module_name().to_owned(),
        type_params: vec![],
    }
}

pub(crate) fn validator_set_path() -> Vec<u8> {
    AccessPath::resource_access_vec(&validator_set_tag(), &Accesses::empty())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "fuzzing"), derive(Arbitrary))]
// Used in twins testing to specify leader(s) per round.
// Note: Round is u64
pub struct ValidatorSet<PublicKey>(Vec<ValidatorPublicKeys<PublicKey>>,
                                   // round_to_proposers: Used in twins testing to specify leader(s) per round.
                                   // Note: Round is u64
                                   // Option<HashMap<u64, Vec<AccountAddress>>>
                                   // Used in twins testing to specify how many twins
                                   // (needed so we can ignore twins in voting power calculations)
                                   Option<usize>,
                                   );

impl<PublicKey> fmt::Display for ValidatorSet<PublicKey> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[")?;
        for validator in &self.0 {
            write!(f, "{} ", validator)?;
        }
        write!(f, "]")
    }
}

impl<PublicKey: VerifyingKey> ValidatorSet<PublicKey> {
    /// Constructs a ValidatorSet resource.
    pub fn new(payload: Vec<ValidatorPublicKeys<PublicKey>>) -> Self {
        ValidatorSet(payload, None)
        //ValidatorSet(payload)
    }

    pub fn empty() -> Self {
        ValidatorSet::new(Vec::new())
    }

    pub fn change_event_key() -> EventKey {
        EventKey::new_from_address(&account_config::validator_set_address(), 2)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        lcs::from_bytes(bytes).map_err(Into::into)
    }


    /// Sets number of twins
    pub fn set_num_twins(&mut self, num_twins: usize) {
        self.1 = Some(num_twins);
    }

    /// Returns number of twins
    pub fn get_num_twins(&self) -> Option<usize> {
        self.1
    }


    /*
    /// Sets the `round_to_proposers' field of the struct ValidatorSet
    pub fn set_round_to_proposers(
        &mut self,
        round_to_proposers_map: HashMap<u64, Vec<AccountAddress>>,
    ) {
        self.1 = Some(round_to_proposers_map);
    }

    /// Returns round proposers
    pub fn get_round_proposers(&self) -> &Option<HashMap<u64, Vec<AccountAddress>>> {
        &self.1
    }
    */
}

impl<PublicKey> Deref for ValidatorSet<PublicKey> {
    type Target = [ValidatorPublicKeys<PublicKey>];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<PublicKey> IntoIterator for ValidatorSet<PublicKey> {
    type Item = ValidatorPublicKeys<PublicKey>;
    type IntoIter = vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<PublicKey: VerifyingKey> TryFrom<crate::proto::types::ValidatorSet>
    for ValidatorSet<PublicKey>
{
    type Error = Error;

    fn try_from(proto: crate::proto::types::ValidatorSet) -> Result<Self> {
        Ok(ValidatorSet::new(
            proto
                .validator_public_keys
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>>>()?,
        ))
    }
}

impl<PublicKey: VerifyingKey> From<ValidatorSet<PublicKey>> for crate::proto::types::ValidatorSet {
    fn from(set: ValidatorSet<PublicKey>) -> Self {
        Self {
            validator_public_keys: set.0.into_iter().map(Into::into).collect(),
        }
    }
}
