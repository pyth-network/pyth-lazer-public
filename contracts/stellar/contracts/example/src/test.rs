extern crate alloc;

use soroban_sdk::{
    testutils::Address as _, testutils::Ledger, Address, Bytes, BytesN, Env, IntoVal,
};

use pyth_lazer_stellar::{PythLazerContract, PythLazerContractClient};

use crate::{Error, PythLazerExample, PythLazerExampleClient, StoredPrice};

// BTC/USD feed in the shared test vector.
const BTC_FEED_ID: u32 = 1;
// Values the parser yields for the BTC feed of `test_lazer_update_bytes`.
const BTC_PRICE: i64 = 6_828_284_601_313;
const BTC_EXPONENT: i32 = -8;
const FEED_TS_US: u64 = 1_771_252_161_800_000;
// A ledger time (unix seconds) 200ms after the feed timestamp.
const FRESH_LEDGER_SECS: u64 = 1_771_252_162;

/// Full signed Lazer update envelope from the Sui/Stellar test suite.
fn test_lazer_update_bytes(env: &Env) -> Bytes {
    Bytes::from_slice(
        env,
        &hex_literal::hex!(
            "e4bd474d73a7e70a8e2b8de236b55dcc6a771b4a8a1533fe"
            "492f424fae162369fa14103e04c1c93302cef8a052110a95"
            "0da031f9dc5eade9e6099e95668aff2592ec1f7900fe0075"
            "d3c7934067e9c7f14a06000303010000000b00e1637ad535"
            "060000015a2507d335060000027f8bfdf53506000004f8ff"
            "0600070008000900000a601299cd3e0600000bc07595c73e"
            "0600000c014067e9c7f14a0600020000000b00971b209c2d"
            "0000000144056b9b2d0000000298fb6b9c2d00000004f8ff"
            "0600070008000900000a284444f92d0000000b480c07f92d"
            "0000000c014067e9c7f14a0600700000000b0020d85dd2d7"
            "8df30001000000000000000002000000000000000004f4ff"
            "060130f80bfeffffffff0701b8ab7057ec4a060008010020"
            "9db4060000000900000a00000000000000000b0000000000"
            "0000000c014067e9c7f14a0600"
        ),
    )
}

/// Trusted signer compressed public key from the Sui test suite.
fn test_trusted_signer_pubkey(env: &Env) -> BytesN<33> {
    BytesN::from_array(
        env,
        &hex_literal::hex!("03a4380f01136eb2640f90c17e1e319e02bbafbeef2e6e67dc48af53f9827e155b"),
    )
}

/// Deploy a real verifier with the test trusted signer and return its address.
fn deploy_verifier(env: &Env) -> Address {
    let executor = Address::generate(env);
    let initial_signer: Option<BytesN<33>> = None;
    let initial_signer_expires_at: Option<u64> = None;
    let verifier_id = env.register(
        PythLazerContract,
        (executor.clone(), initial_signer, initial_signer_expires_at),
    );
    let client = PythLazerContractClient::new(env, &verifier_id);

    let pubkey = test_trusted_signer_pubkey(env);
    let expires_at = 9_999_999_999u64; // Past the fixture's ledger time.
    client
        .mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &executor,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "update_trusted_signer",
                args: (pubkey.clone(), expires_at).into_val(env),
                sub_invokes: &[],
            },
        }])
        .update_trusted_signer(&pubkey, &expires_at);

    verifier_id
}

/// Deploy the example contract pointing at `verifier`.
fn deploy_example<'a>(
    env: &Env,
    verifier: &Address,
    feed_id: u32,
    freshness_threshold_us: u64,
) -> PythLazerExampleClient<'a> {
    let contract_id = env.register(
        PythLazerExample,
        (verifier.clone(), feed_id, freshness_threshold_us),
    );
    PythLazerExampleClient::new(env, &contract_id)
}

#[test]
fn test_update_price_success() {
    let env = Env::default();
    let verifier = deploy_verifier(&env);
    let example = deploy_example(&env, &verifier, BTC_FEED_ID, 60_000_000);

    env.ledger().set_timestamp(FRESH_LEDGER_SECS);

    let payload = test_lazer_update_bytes(&env);
    example.update_price(&payload);

    assert_eq!(
        example.get_price(),
        StoredPrice {
            price: BTC_PRICE,
            exponent: BTC_EXPONENT,
            timestamp_us: FEED_TS_US,
        }
    );
}

#[test]
fn test_update_price_stale() {
    let env = Env::default();
    let verifier = deploy_verifier(&env);
    // Threshold of 100ms; the ledger sits ~1.2s past the feed timestamp.
    let example = deploy_example(&env, &verifier, BTC_FEED_ID, 100_000);

    env.ledger().set_timestamp(FRESH_LEDGER_SECS + 1);

    let payload = test_lazer_update_bytes(&env);
    let result = example.try_update_price(&payload);
    assert_eq!(result.err().unwrap(), Ok(Error::PriceStale));
}

#[test]
fn test_update_price_feed_missing() {
    let env = Env::default();
    let verifier = deploy_verifier(&env);
    // Feed id not present in the payload.
    let example = deploy_example(&env, &verifier, 9_999, 60_000_000);

    env.ledger().set_timestamp(FRESH_LEDGER_SECS);

    let payload = test_lazer_update_bytes(&env);
    let result = example.try_update_price(&payload);
    assert_eq!(result.err().unwrap(), Ok(Error::FeedMissing));
}

#[test]
fn test_get_price_uninitialized() {
    let env = Env::default();
    let verifier = deploy_verifier(&env);
    let example = deploy_example(&env, &verifier, BTC_FEED_ID, 60_000_000);

    let result = example.try_get_price();
    assert_eq!(result.err().unwrap(), Ok(Error::PriceNotInitialized));
}

#[test]
fn test_update_price_overwrites() {
    let env = Env::default();
    let verifier = deploy_verifier(&env);
    let example = deploy_example(&env, &verifier, BTC_FEED_ID, 60_000_000);

    let payload = test_lazer_update_bytes(&env);

    env.ledger().set_timestamp(FRESH_LEDGER_SECS);
    example.update_price(&payload);

    // A second successful call (at a later, still-fresh ledger time) overwrites
    // the stored price. Re-signing a timestamp-shifted payload isn't possible
    // without the signer's secret key, so the same fixture is reused.
    env.ledger().set_timestamp(FRESH_LEDGER_SECS + 10);
    example.update_price(&payload);

    assert_eq!(
        example.get_price(),
        StoredPrice {
            price: BTC_PRICE,
            exponent: BTC_EXPONENT,
            timestamp_us: FEED_TS_US,
        }
    );
}
