//! new-transaction
//! sender: blessed
script {
use 0x0::Libra;
use 0x0::Coin1;
use 0x0::Coin2;
use 0x0::Signer;
use 0x0::Transaction;

// Make sure we can mint and burn
fun main(account: &signer) {
    let sender = Signer::address_of(account);
    let coin1_coins = Libra::mint<Coin1::T>(account, 10);
    let coin2_coins = Libra::mint<Coin2::T>(account, 10);
    let pre_coin1 = Libra::new_preburn<Coin1::T>();
    let pre_coin2 = Libra::new_preburn<Coin2::T>();
    Libra::publish_preburn(pre_coin1);
    Libra::publish_preburn(pre_coin2);
    Transaction::assert(Libra::market_cap<Coin1::T>() == 10, 7);
    Transaction::assert(Libra::market_cap<Coin2::T>() == 10, 8);
    Libra::preburn_to_sender(coin1_coins);
    Libra::preburn_to_sender(coin2_coins);
    Libra::burn<Coin1::T>(sender);
    Libra::burn<Coin2::T>(sender);
    Transaction::assert(Libra::market_cap<Coin1::T>() == 0, 9);
    Transaction::assert(Libra::market_cap<Coin2::T>() == 0, 10);

    let coin1_coins = Libra::mint<Coin1::T>(account, 10);
    let coin2_coins = Libra::mint<Coin2::T>(account, 10);

    Libra::update_minting_ability<Coin1::T>(account, false);
    Libra::preburn_to_sender(coin1_coins);
    Libra::preburn_to_sender(coin2_coins);
    Libra::burn<Coin1::T>(sender);
    Libra::burn<Coin2::T>(sender);
    Transaction::assert(Libra::market_cap<Coin1::T>() == 0, 11);
    Transaction::assert(Libra::market_cap<Coin2::T>() == 0, 12);
    Libra::preburn_to_sender(Libra::mint<Coin2::T>(account, 10));
    Libra::preburn_to_sender(Libra::mint<Coin1::T>(account, 10))
}
}
// check: ABORTED
// check: 4
