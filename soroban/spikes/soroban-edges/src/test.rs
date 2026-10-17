// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{
        storage::{Persistent as _, Temporary as _},
        Address as _, Events as _, IssuerFlags, Ledger as _,
    },
    token::{StellarAssetClient, TokenClient},
    vec,
    xdr::{
        self, AccountEntry, AccountEntryExt, AccountId, LedgerEntry, LedgerEntryData,
        LedgerEntryExt, LedgerKey, LedgerKeyAccount, LedgerKeyTrustLine, PublicKey, ScAddress,
        SequenceNumber, Thresholds, TrustLineAsset, TrustLineEntry, TrustLineEntryExt, Uint256,
    },
    Env, TryFromVal,
};
use std::rc::Rc;

mod child_wasm {
    soroban_sdk::contractimport!(file = "fixtures/soroban_edges_child.wasm");
}

struct Funded {
    env: Env,
    token: Address,
    asset: xdr::Asset,
    sac: soroban_sdk::testutils::StellarAssetContract,
    payer: Address,
}

fn funded(balance: i128) -> Funded {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(admin);
    let token = sac.address();
    let payer = env.register(Payer, ());
    StellarAssetClient::new(&env, &token).mint(&payer, &balance);
    Funded {
        asset: sac.asset(),
        env,
        token,
        sac,
        payer,
    }
}

fn account_address(env: &Env, seed: u8) -> Address {
    let id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([seed; 32])));
    Address::try_from_val(env, &ScAddress::Account(id)).unwrap()
}

fn account_id(address: &Address) -> AccountId {
    match ScAddress::from(address) {
        ScAddress::Account(id) => id,
        other => panic!("not an account address: {other:?}"),
    }
}

fn add_entry(env: &Env, key: LedgerKey, data: LedgerEntryData) {
    let entry = LedgerEntry {
        data,
        last_modified_ledger_seq: 0,
        ext: LedgerEntryExt::V0,
    };
    env.host()
        .add_ledger_entry(&Rc::new(key), &Rc::new(entry), None)
        .unwrap();
}

/// A classic account with no trustlines, like a fresh Stellar wallet.
fn create_account(env: &Env, address: &Address) {
    let account_id = account_id(address);
    add_entry(
        env,
        LedgerKey::Account(LedgerKeyAccount {
            account_id: account_id.clone(),
        }),
        LedgerEntryData::Account(AccountEntry {
            account_id,
            balance: 10_000_000,
            flags: 0,
            home_domain: Default::default(),
            inflation_dest: None,
            num_sub_entries: 0,
            seq_num: SequenceNumber(0),
            thresholds: Thresholds([1; 4]),
            signers: Default::default(),
            ext: AccountEntryExt::V0,
        }),
    );
}

fn create_trustline(env: &Env, address: &Address, asset: &xdr::Asset, flags: u32) {
    let account_id = account_id(address);
    let asset = match asset.clone() {
        xdr::Asset::Native => TrustLineAsset::Native,
        xdr::Asset::CreditAlphanum4(a) => TrustLineAsset::CreditAlphanum4(a),
        xdr::Asset::CreditAlphanum12(a) => TrustLineAsset::CreditAlphanum12(a),
    };
    add_entry(
        env,
        LedgerKey::Trustline(LedgerKeyTrustLine {
            account_id: account_id.clone(),
            asset: asset.clone(),
        }),
        LedgerEntryData::Trustline(TrustLineEntry {
            account_id,
            asset,
            balance: 0,
            limit: i64::MAX,
            flags,
            ext: TrustLineEntryExt::V0,
        }),
    );
}

// SAC (Stellar Asset Contract) error codes, as the host reports them.
const SAC_BALANCE_ERROR: u32 = 10;
const SAC_BALANCE_DEAUTHORIZED: u32 = 11;
const SAC_TRUSTLINE_MISSING: u32 = 13;
const CONTRACT: u32 = ScErrorType::Contract as u32;

fn leg(to: &Address, amount: i128) -> Leg {
    Leg {
        to: to.clone(),
        amount,
    }
}

/// A classic account that holds an authorized trustline, so it can receive.
fn receiver(f: &Funded, seed: u8) -> Address {
    let a = account_address(&f.env, seed);
    create_account(&f.env, &a);
    create_trustline(
        &f.env,
        &a,
        &f.asset,
        xdr::TrustLineFlags::AuthorizedFlag as u32,
    );
    a
}

/// What every failed transfer must look like from outside the mandate: our own
/// error, nothing moved, the caller's write gone, and no events left behind.
fn assert_mapped_and_rolled_back(f: &Funded, to: &Address, amount: i128, balance: i128) {
    let payer = PayerClient::new(&f.env, &f.payer);
    let token = TokenClient::new(&f.env, &f.token);
    let res = payer.try_pay(&f.token, &vec![&f.env, leg(to, amount)]);
    assert_eq!(res, Err(Ok(ProbeError::TransferFailed)));
    assert_eq!(f.env.events().all().events().len(), 0);
    assert_eq!(payer.marker(), None);
    assert_eq!(token.balance(&f.payer), balance);
}

/// Revoke shape: the failure is caught, the call succeeds, and the caller's own
/// write stays. Returns what the SAC reported.
fn caught_and_kept(f: &Funded, to: &Address, amount: i128) -> Option<Caught> {
    let payer = PayerClient::new(&f.env, &f.payer);
    let got = payer.pay_or_keep(&f.token, to, &amount);
    // pay_or_keep emits one event of its own; a successful SAC transfer would
    // have added a second one, from the token.
    let events = f.env.events().all();
    assert_eq!(events.filter_by_contract(&f.payer).events().len(), 1);
    if got.is_some() {
        assert_eq!(events.filter_by_contract(&f.token).events().len(), 0);
    } else {
        assert_eq!(events.filter_by_contract(&f.token).events().len(), 1);
    }
    assert_eq!(payer.marker(), Some(amount));
    got
}

#[test]
fn transfer_failure_no_trustline() {
    let f = funded(1_000);

    // A classic account that exists but holds no trustline for the asset.
    let no_trustline = account_address(&f.env, 1);
    create_account(&f.env, &no_trustline);
    assert_mapped_and_rolled_back(&f, &no_trustline, 100, 1_000);

    // An account that does not exist at all fails the same way: the SAC looks
    // for the trustline first and reports it missing.
    let no_account = account_address(&f.env, 2);
    assert_mapped_and_rolled_back(&f, &no_account, 100, 1_000);

    let want = Some(Caught {
        error_type: CONTRACT,
        code: SAC_TRUSTLINE_MISSING,
    });
    assert_eq!(caught_and_kept(&f, &no_trustline, 100), want);
    assert_eq!(caught_and_kept(&f, &no_account, 100), want);
    assert_eq!(TokenClient::new(&f.env, &f.token).balance(&f.payer), 1_000);
}

#[test]
fn transfer_failure_deauthorized() {
    let f = funded(1_000);
    let want = Some(Caught {
        error_type: CONTRACT,
        code: SAC_BALANCE_DEAUTHORIZED,
    });

    // Recipient side: a trustline the issuer has not authorized.
    let unauthorized = account_address(&f.env, 3);
    create_account(&f.env, &unauthorized);
    create_trustline(&f.env, &unauthorized, &f.asset, 0);
    assert_mapped_and_rolled_back(&f, &unauthorized, 100, 1_000);
    assert_eq!(caught_and_kept(&f, &unauthorized, 100), want);

    // Sender side: the issuer freezes the contract's own balance. This needs
    // AUTH_REVOCABLE on the issuer, which Circle's USDC issuer has.
    let good = receiver(&f, 4);
    f.sac.issuer().set_flag(IssuerFlags::RevocableFlag);
    StellarAssetClient::new(&f.env, &f.token).set_authorized(&f.payer, &false);
    let payer = PayerClient::new(&f.env, &f.payer);
    // Clear the marker left by the previous case so the rollback check means
    // something.
    f.env.as_contract(&f.payer, || {
        f.env.storage().persistent().remove(&ProbeKey::Marker)
    });
    assert_eq!(payer.marker(), None);
    assert_mapped_and_rolled_back(&f, &good, 100, 1_000);
    assert_eq!(caught_and_kept(&f, &good, 100), want);
    assert_eq!(TokenClient::new(&f.env, &f.token).balance(&good), 0);
}

#[test]
fn transfer_failure_insufficient_balance() {
    let f = funded(1_000);
    let good = receiver(&f, 5);
    assert_mapped_and_rolled_back(&f, &good, 1_001, 1_000);
    assert_eq!(
        caught_and_kept(&f, &good, 1_001),
        Some(Caught {
            error_type: CONTRACT,
            code: SAC_BALANCE_ERROR,
        })
    );
    // The exact balance still goes through, so the failure above was the
    // amount and nothing else.
    assert_eq!(caught_and_kept(&f, &good, 1_000), None);
    assert_eq!(TokenClient::new(&f.env, &f.token).balance(&good), 1_000);
}

#[test]
fn transfer_failure_rolls_back_earlier_legs() {
    let f = funded(1_000);
    let good = receiver(&f, 6);
    let no_trustline = account_address(&f.env, 7);
    create_account(&f.env, &no_trustline);
    let payer = PayerClient::new(&f.env, &f.payer);
    let token = TokenClient::new(&f.env, &f.token);

    // The first leg succeeds inside the call, the second fails. Returning our
    // error undoes the first leg too.
    let res = payer.try_pay(
        &f.token,
        &vec![&f.env, leg(&good, 300), leg(&no_trustline, 200)],
    );
    assert_eq!(res, Err(Ok(ProbeError::TransferFailed)));
    assert_eq!(token.balance(&good), 0);
    assert_eq!(token.balance(&f.payer), 1_000);
    assert_eq!(payer.marker(), None);

    // Same legs in the other order, and a fully valid pair, for contrast.
    let res = payer.try_pay(
        &f.token,
        &vec![&f.env, leg(&no_trustline, 200), leg(&good, 300)],
    );
    assert_eq!(res, Err(Ok(ProbeError::TransferFailed)));
    assert_eq!(token.balance(&good), 0);
    payer.pay(&f.token, &vec![&f.env, leg(&good, 300), leg(&good, 200)]);
    assert_eq!(token.balance(&good), 500);
    assert_eq!(payer.marker(), Some(500));
}

fn instance_entry_exists(env: &Env, contract: &Address) -> bool {
    let key = LedgerKey::ContractData(xdr::LedgerKeyContractData {
        contract: ScAddress::from(contract),
        key: xdr::ScVal::LedgerKeyContractInstance,
        durability: xdr::ContractDataDurability::Persistent,
    });
    env.host()
        .get_ledger_entry(&Rc::new(key))
        .unwrap()
        .is_some()
}

#[test]
fn constructor_error_through_deployer() {
    let env = Env::default();
    let hash = env.deployer().upload_contract_wasm(child_wasm::WASM);
    let factory = env.register(Factory, ());
    let client = FactoryClient::new(&env, &factory);
    let salt = BytesN::from_array(&env, &[7; 32]);
    let predicted = client.predicted(&salt);

    // The constructor returns ConfigInvalid (34). The factory caller does not
    // see 34: the host rewrites any recoverable constructor error to a generic
    // context error so it cannot leak into the caller. The code is only in the
    // diagnostic events.
    let res = client.try_create(&hash, &salt, &7, &true);
    assert_eq!(
        res,
        Err(Ok(Error::from_type_and_code(
            ScErrorType::Context,
            xdr::ScErrorCode::InvalidAction
        )))
    );
    assert_ne!(
        res,
        Err(Ok(Error::from_contract_error(
            child_wasm::ChildError::ConfigInvalid as u32
        )))
    );
    assert_eq!(env.events().all().events().len(), 0);

    // Nothing is left at the deterministic address: no instance entry, and the
    // same salt deploys cleanly once the arguments are good.
    assert!(!instance_entry_exists(&env, &predicted));
    let addr = client.create(&hash, &salt, &7, &false);
    assert_eq!(addr, predicted);
    assert!(instance_entry_exists(&env, &addr));
    assert_eq!(child_wasm::Client::new(&env, &addr).value(), Some(7));

    // A second deploy at a used address fails, which is why the clean retry
    // above proves the failed deploy left nothing. A native test contract
    // reports the generic context error here; the same factory running as
    // wasm on a network reports Error(Storage, ExistingValue), so host error
    // codes seen through native test contracts are not the ones a chain
    // reports.
    assert_eq!(
        client.try_create(&hash, &salt, &8, &false),
        Err(Ok(Error::from_type_and_code(
            ScErrorType::Context,
            xdr::ScErrorCode::InvalidAction
        )))
    );
    assert_eq!(child_wasm::Client::new(&env, &addr).value(), Some(7));
}

#[test]
fn constructor_error_checked_in_factory() {
    let env = Env::default();
    let hash = env.deployer().upload_contract_wasm(child_wasm::WASM);
    let factory = env.register(Factory, ());
    let client = FactoryClient::new(&env, &factory);
    let salt = BytesN::from_array(&env, &[9; 32]);

    // Validating in the factory before deploying is the only way the caller
    // gets the mandate's own code back.
    assert_eq!(
        client.try_create_checked(&hash, &salt, &7, &true),
        Err(Ok(ProbeError::ConfigInvalid))
    );
    assert!(!instance_entry_exists(&env, &client.predicted(&salt)));
    let addr = client.create_checked(&hash, &salt, &7, &false);
    assert_eq!(child_wasm::Client::new(&env, &addr).value(), Some(7));
}

fn live_until(
    env: &Env,
    contract: &Address,
    key: &ProbeKey,
    durability: xdr::ContractDataDurability,
) -> Option<u32> {
    let key = LedgerKey::ContractData(xdr::LedgerKeyContractData {
        contract: ScAddress::from(contract),
        key: xdr::ScVal::try_from_val(env, &IntoVal::<Env, soroban_sdk::Val>::into_val(key, env))
            .unwrap(),
        durability,
    });
    env.host()
        .get_ledger_entry(&Rc::new(key))
        .unwrap()
        .and_then(|(_, live_until)| live_until)
}

#[test]
fn archived_entry_read() {
    let env = Env::default();
    let store = env.register(Store, ());
    let client = StoreClient::new(&env, &store);
    let min_persistent = env.ledger().get().min_persistent_entry_ttl;
    assert_eq!(min_persistent, 4_096);

    let start = env.ledger().sequence();
    client.put(&1, &42, &10_000);
    let key = ProbeKey::Record(1);
    let ttl = env.as_contract(&store, || env.storage().persistent().get_ttl(&key));
    assert_eq!(ttl, 10_000);
    assert_eq!(
        live_until(&env, &store, &key, xdr::ContractDataDurability::Persistent),
        Some(start + 10_000)
    );

    // One ledger past the last live ledger the entry is archived. The test
    // env, like a protocol 23+ network with a simulated footprint, restores it
    // inside the call instead of failing, so the uniqueness check still says
    // "used" and the value is intact. It never reads as absent.
    env.ledger().set_sequence_number(start + 10_001);
    assert!(client.has(&1));
    let resources = env.cost_estimate().resources();
    // The record and the contract instance were both archived, so both were
    // written back.
    assert_eq!(resources.write_entries, 2);
    assert_eq!(resources.persistent_entry_rent_bumps, 2);
    assert_eq!(client.get(&1), Some(42));
    // A restored entry gets the minimum TTL, counting the restoring ledger.
    let restored_ttl = env.as_contract(&store, || env.storage().persistent().get_ttl(&key));
    assert_eq!(restored_ttl, min_persistent - 1);

    // An ID that was never written is still reported unused.
    assert!(!client.has(&2));
}

#[test]
fn temporary_entry_lapses_to_absent() {
    let env = Env::default();
    let store = env.register(Store, ());
    let client = StoreClient::new(&env, &store);
    let start = env.ledger().sequence();
    client.put_temp(&1, &5, &100);
    let key = ProbeKey::TempRecord(1);
    let ttl = env.as_contract(&store, || env.storage().temporary().get_ttl(&key));
    assert_eq!(ttl, 100);
    assert!(client.has_temp(&1));

    // Temporary storage has no archive: once the TTL runs out the entry is
    // simply gone and reads as never written. A uniqueness record kept here
    // would let an ID be used twice.
    assert_eq!(
        live_until(&env, &store, &key, xdr::ContractDataDurability::Temporary),
        Some(start + 100)
    );
    env.ledger().set_sequence_number(start + 101);
    assert!(!client.has_temp(&1));
    assert_eq!(
        live_until(&env, &store, &key, xdr::ContractDataDurability::Temporary),
        None
    );
}

#[test]
fn transfer_failure_rolled_back_write_is_still_charged() {
    let f = funded(1_000);
    let payer = PayerClient::new(&f.env, &f.payer);
    let good = receiver(&f, 8);
    let no_trustline = account_address(&f.env, 9);
    create_account(&f.env, &no_trustline);
    // Prime the marker so every call below rewrites the same entry.
    payer.pay_or_keep(&f.token, &good, &1);

    // A sender-side failure stops the SAC before it writes anything, so only
    // our own marker is written.
    assert!(payer.pay_or_keep(&f.token, &good, &5_000).is_some());
    let sender_side = f.env.cost_estimate().resources();
    assert_eq!(sender_side.write_entries, 1);
    assert_eq!(sender_side.write_bytes, 104);

    // A recipient-side failure comes after the SAC has debited the sender.
    // That write is rolled back, yet it still counts against the write
    // budget. RPC simulation (stellar-rpc 29.0.1) leaves it out and declares
    // 104 bytes, so the submitted transaction fails with ResourceLimitExceeded
    // unless the submitter pads the write bytes.
    assert!(payer.pay_or_keep(&f.token, &no_trustline, &1).is_some());
    let recipient_side = f.env.cost_estimate().resources();
    assert_eq!(recipient_side.write_entries, 2);
    assert_eq!(recipient_side.write_bytes, 328);
    assert_eq!(TokenClient::new(&f.env, &f.token).balance(&f.payer), 999);
}

#[test]
fn transfer_failure_budget_exhaustion_is_not_caught() {
    // The env writes a snapshot when it drops, and that draws on the same
    // exhausted budget, so this test skips it.
    let env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    // The test env panics as soon as a call goes over mainnet limits, which
    // hides what the host returns. Turn that off and give the call a plain
    // CPU budget instead.
    env.cost_estimate().disable_resource_limits();
    env.cost_estimate()
        .budget()
        .reset_limits(20_000_000, 200_000_000);
    // A stored value of 0 makes the child's transfer grow a vector forever.
    let burner = env.register(child_wasm::WASM, (0_u32, false));
    let payer = env.register(Payer, ());
    let client = PayerClient::new(&env, &payer);
    let to = Address::generate(&env);

    // pay_or_keep catches every failure it can and then succeeds. Budget
    // exhaustion is not one of them: try_invoke_contract never returns, and
    // the whole invocation fails with the host's budget error.
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.try_pay_or_keep(&burner, &to, &1)
    }));
    env.cost_estimate().budget().reset_unlimited();
    let events = std::format!("{:?}", env.host().get_diagnostic_events().unwrap());
    match got {
        Ok(Ok(Ok(caught))) => panic!("budget exhaustion was caught: {caught:?}"),
        Ok(Err(Ok(e))) => assert_eq!(
            e,
            Error::from_type_and_code(ScErrorType::Budget, xdr::ScErrorCode::ExceededLimit)
        ),
        // The native caller frame can only escalate the error by panicking.
        other => assert!(other.is_err(), "unexpected result {other:?}"),
    }
    // The token's own frame ran out of budget, try_call handed the same error
    // back up instead of a value the payer could inspect, and the payer
    // frame could only escalate it.
    assert!(
        events.contains("Error(Budget(ExceededLimit))"),
        "no budget error logged"
    );
    assert!(
        events.contains("contract try_call failed"),
        "the failure did not come through try_call"
    );
    assert!(events.contains("escalating error to panic"));
    assert_eq!(client.marker(), None);
}

#[test]
fn transfer_failure_host_error_is_narrowed() {
    let f = funded(1_000);
    let payer = PayerClient::new(&f.env, &f.payer);
    // No contract lives at this address, so the host fails the call itself.
    let nowhere = Address::generate(&f.env);
    let to = Address::generate(&f.env);
    assert_eq!(
        payer.pay_or_keep(&nowhere, &to, &1),
        Some(Caught {
            error_type: ScErrorType::Context as u32,
            code: xdr::ScErrorCode::InvalidAction as u32,
        })
    );
    // The granular cause is only in the diagnostic log.
    let events = std::format!("{:?}", f.env.host().get_diagnostic_events().unwrap());
    assert!(events.contains("contract try_call failed"));
    assert_eq!(payer.marker(), Some(1));
}

#[test]
fn transfer_returning_a_value_is_a_failure() {
    let env = Env::default();
    // Any non-zero stored value is what the child's transfer returns.
    let odd_token = env.register(child_wasm::WASM, (7_u32, false));
    let payer = env.register(Payer, ());
    let client = PayerClient::new(&env, &payer);
    let to = Address::generate(&env);
    assert_eq!(
        client.pay_or_keep(&odd_token, &to, &1),
        Some(Caught {
            error_type: ScErrorType::Value as u32,
            code: u32::MAX,
        })
    );
    assert_eq!(
        client.try_pay(&odd_token, &vec![&env, leg(&to, 1)]),
        Err(Ok(ProbeError::TransferFailed))
    );
}

#[test]
fn constructor_error_code_is_only_in_diagnostics() {
    let env = Env::default();
    let hash = env.deployer().upload_contract_wasm(child_wasm::WASM);
    let factory = env.register(Factory, ());
    let client = FactoryClient::new(&env, &factory);
    let salt = BytesN::from_array(&env, &[5; 32]);
    assert!(client.try_create(&hash, &salt, &7, &true).is_err());
    let events = std::format!("{:?}", env.host().get_diagnostic_events().unwrap());
    assert!(events.contains("constructor invocation has failed with error"));
    assert!(events.contains("Error(Contract(34))"));
}
