//! account: validatorvivian, 10000000, 0, validator
//! account: bob, 100000000, 0, unhosted
//! account: alice, 100000000, 0, unhosted

module Holder {
    resource struct Hold<T> { x: T }
    public fun hold<T>(x: T) {
        move_to_sender(Hold<T>{x})
    }
}

//! new-transaction
//! sender: association
script {
    use 0x0::Testnet;
    // Unset testnet
    fun main() {
        Testnet::remove_testnet()
    }
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::LibraAccount;
    use 0x0::LBR;
    fun main() {
        LibraAccount::pay_from_sender<LBR::T>({{alice}}, 1);
    }
}
// TODO: fix
// chec: ABORTED
// chec: 10048

//! new-transaction
//! sender: association
script {
use 0x0::LibraAccount;
fun main(account: &signer) {
    LibraAccount::mint_lbr_to_address(account, {{bob}}, 1);
}
}
// TODO: fix
// chec: ABORTED
// chec: 10047

//! new-transaction
//! sender: association
script {
    use 0x0::Testnet;
    // Reset testnet
    fun main() {
        Testnet::initialize()
    }
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::AccountLimits;
    fun main(account: &signer) {
        AccountLimits::publish_unrestricted_limits(account)
    }
}
// check: EXECUTED

//! new-transaction
//! sender: association
script {
    use 0x0::AccountLimits;
    fun main(account: &signer) {
        AccountLimits::certify_limits_definition(account, {{bob}});
    }
}
// check: EXECUTED

//! new-transaction
script {
    use 0x0::AccountLimits;
    fun main(account: &signer) {
        AccountLimits::decertify_limits_definition(account, {{bob}});
    }
}
// check: ABORTED
// check: 1002

//! new-transaction
//! sender: association
script {
    use 0x0::AccountLimits;
    fun main(account: &signer) {
        AccountLimits::decertify_limits_definition(account, {{bob}});
    }
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::AccountLimits;
    fun main(account: &signer) {
        AccountLimits::unpublish_limits_definition(account);
    }
}
// check: EXECUTED

//! new-transaction
script {
    use 0x0::AccountLimits;
    use 0x0::Transaction;
    fun main() {
        Transaction::assert(AccountLimits::default_limits_addr() == {{association}}, 0);
    }
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::LibraAccount;
    use 0x0::LBR;
    // Since we directly wrote into this account using fake data store, we
    // don't actually know that the balance is greater than 0 in the
    // account limits code, but it is.
    fun main() {
        LibraAccount::pay_from_sender<LBR::T>({{alice}}, 1);
    }
}
// check: EXECUTED

//! new-transaction
//! sender: association
script {
use 0x0::LibraAccount;
fun main(account: &signer) {
    LibraAccount::mint_lbr_to_address(account, {{bob}}, 2);
}
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::LibraAccount;
    use 0x0::LBR;
    fun main() {
        LibraAccount::pay_from_sender<LBR::T>({{alice}}, 1);
    }
}
// check: EXECUTED

//! new-transaction
//! sender: association
script {
    use 0x0::AccountLimits;
    // Publish our own limits definition for testing! Make sure we are
    // exercising the unrestricted limits check.
    fun main(account: &signer) {
        AccountLimits::unpublish_limits_definition(account);
        AccountLimits::publish_unrestricted_limits(account);
        AccountLimits::certify_limits_definition(account, {{association}});
    }
}
// check: EXECUTED

//! new-transaction
//! sender: association
script {
use 0x0::LibraAccount;
fun main(account: &signer) {
    LibraAccount::mint_lbr_to_address(account, {{bob}}, 2);
}
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::LibraAccount;
    use 0x0::LBR;
    fun main() {
        LibraAccount::pay_from_sender<LBR::T>({{alice}}, 1);
    }
}
// check: EXECUTED

//! new-transaction
//! sender: association
script {
    use 0x0::AccountLimits;
    // Publish our own limits definition for testing! Make sure we are
    // exercising the unrestricted limits check.
    fun main(account: &signer) {
        AccountLimits::decertify_limits_definition(account, {{association}});
    }
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::LibraAccount;
    use 0x0::LBR;
    fun main() {
        LibraAccount::pay_from_sender<LBR::T>({{alice}}, 1);
    }
}
// TODO: fix
// chec: ABORTED
// chec: 1

//! new-transaction
//! sender: association
script {
    use 0x0::AccountLimits;
    // Publish our own limits definition for testing!
    fun main(account: &signer) {
        AccountLimits::unpublish_limits_definition(account);
        AccountLimits::publish_limits_definition(
            account,
            100,
            100,
            200,
            40000,
        );
        AccountLimits::certify_limits_definition(account, {{association}});
    }
}
// check: EXECUTED

//! block-prologue
//! proposer: validatorvivian
//! block-time: 40001

//! new-transaction
//! sender: association
script {
    use 0x0::LibraAccount;
    fun main(account: &signer) {
        LibraAccount::mint_lbr_to_address(account, {{bob}}, 100);
    }
}
// check: EXECUTED

//! new-transaction
//! sender: association
script {
    use 0x0::LibraAccount;
    fun main(account: &signer) {
        LibraAccount::mint_lbr_to_address(account, {{bob}}, 1);
    }
}
// TODO: fix
// chec: ABORTED
// chec: 9

//! new-transaction
//! sender: bob
script {
    use 0x0::LibraAccount;
    use 0x0::LBR;
    fun main() {
        LibraAccount::pay_from_sender<LBR::T>({{alice}}, 101);
    }
}
// chec: ABORTED
// chec: 11

//! new-transaction
//! sender: association
script {
    use 0x0::AccountLimits;
    // Publish our own limits definition for testing! Make sure we are
    // exercising the unrestricted limits check.
    fun main(account: &signer) {
        AccountLimits::decertify_limits_definition(account, {{association}});
    }
}
// check: EXECUTED

//! new-transaction
//! sender: bob
script {
    use 0x0::LibraAccount;
    use 0x0::LBR;
    fun main() {
        LibraAccount::pay_from_sender<LBR::T>({{alice}}, 1);
    }
}
// TODO: fix
// chec: ABORTED
// chec: 1

//! new-transaction
//! sender: association
script {
    use 0x0::LibraAccount;
    fun main(account: &signer) {
        LibraAccount::mint_lbr_to_address(account, {{bob}}, 1);
    }
}
// TODO: fix
// chec: ABORTED
// chec: 1
