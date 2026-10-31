// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The factory's constructor, its checks in the order they run, the
//! deterministic address, the creation events and the fixed asset list and
//! fee address.

extern crate std;

use super::{account, contract, executable_of, register_factory, Fx};
use crate::MandateCreated;
use mandate_config::{MandateConfig, SPEC_VERSION};
use mandate_errors::MandateError;
use soroban_sdk::{
    testutils::{AuthorizedFunction, AuthorizedInvocation, Events as _, MockAuth, MockAuthInvoke},
    xdr::{self, ContractEventBody, ScSpecEntry, ScVal},
    Address, BytesN, Event, IntoVal, Symbol, TryFromVal, Vec,
};
use std::collections::BTreeSet;
use std::string::{String, ToString};
use ujira_mandate::events::MandateInitialized;
use ujira_mandate::UjiraMandateClient;

const CONFIG_INVALID: MandateError = MandateError::ConfigInvalid;
const ROLE_CONFLICT: MandateError = MandateError::RoleConflict;
const ROLE_SET_FULL: MandateError = MandateError::RoleSetFull;

/// A change to the base configuration.
type Edit = fn(&Fx, &mut MandateConfig);

/// What a failed `create` looked like to its caller.
#[derive(Debug, Eq, PartialEq)]
enum Refusal {
    /// A contract error code the factory returned.
    Code(MandateError),
    /// A host failure, such as a missing authorization or a host error from
    /// the deploy.
    Host,
}

fn try_create(fx: &Fx, config: &MandateConfig, salt: &BytesN<32>) -> Result<Address, Refusal> {
    match fx.client().try_create(config, salt) {
        Ok(Ok(addr)) => Ok(addr),
        Ok(Err(e)) => panic!("create returned something that is not an address: {e:?}"),
        Err(Ok(code)) => Err(Refusal::Code(code)),
        Err(Err(_)) => Err(Refusal::Host),
    }
}

/// Creates with every authorization mocked, so only the factory's own
/// checks can refuse.
fn create(fx: &Fx, config: &MandateConfig, salt: &BytesN<32>) -> Result<Address, Refusal> {
    fx.env.mock_all_auths();
    try_create(fx, config, salt)
}

fn salt(fx: &Fx, byte: u8) -> BytesN<32> {
    fx.b32(byte)
}

/// A failed create must leave nothing: no events, and no contract at the
/// address it would have deployed to.
fn assert_nothing_left(fx: &Fx, principal: &Address, salt: &BytesN<32>) {
    assert_eq!(fx.env.events().all().events().len(), 0);
    assert_eq!(executable_of(&fx.env, &fx.predicted(principal, salt)), None);
}

fn accounts(fx: &Fx, first: u8, n: u8) -> Vec<Address> {
    let mut out = Vec::new(&fx.env);
    for i in 0..n {
        out.push_back(fx.account(first + i));
    }
    out
}

// The constructor.

#[test]
fn constructor_stores_what_the_views_answer() {
    let fx = Fx::new();
    let c = fx.client();
    assert_eq!(
        c.supported_assets(),
        fx.list(&[&fx.asset, &fx.second_asset])
    );
    assert_eq!(c.mandate_wasm_hash(), fx.wasm_hash);
    assert_eq!(c.fee_address(), fx.fee_address);
    assert_eq!(c.version(), SPEC_VERSION);
    assert_eq!(c.version(), 1);
}

#[test]
fn constructor_refuses_an_empty_or_repeated_asset_list() {
    let fx = Fx::new();
    let refuse = |assets: Vec<Address>| {
        let got = register_factory(&fx.env, None, &fx.wasm_hash, &assets, &fx.fee_address);
        assert_eq!(got, Err(Some(CONFIG_INVALID)), "{assets:?}");
    };
    refuse(Vec::new(&fx.env));
    refuse(fx.list(&[&fx.asset, &fx.asset]));
    refuse(fx.list(&[&fx.asset, &fx.second_asset, &fx.asset]));
}

#[test]
fn constructor_refuses_a_fee_address_nobody_can_spend_from() {
    let fx = Fx::new();
    let assets = fx.list(&[&fx.asset]);
    let refuse = |at: Option<&Address>, fee: &Address| {
        let got = register_factory(&fx.env, at, &fx.wasm_hash, &assets, fee);
        assert_eq!(got, Err(Some(CONFIG_INVALID)), "{fee:?}");
    };
    // The all-zero account and contract, Stellar's nearest thing to a zero
    // address.
    refuse(None, &account(&fx.env, 0));
    refuse(None, &contract(&fx.env, 0));
    // A supported asset's own contract.
    refuse(None, &fx.asset);
    // The factory itself, which has no way to pay anything out.
    let at = contract(&fx.env, 0x5a);
    refuse(Some(&at), &at);

    // A fee address with a single zero byte is an ordinary address.
    let mut key = [7u8; 32];
    key[0] = 0;
    let fee = Address::try_from_val(
        &fx.env,
        &xdr::ScAddress::Account(xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(
            xdr::Uint256(key),
        ))),
    )
    .unwrap();
    assert!(register_factory(&fx.env, None, &fx.wasm_hash, &assets, &fee).is_ok());
    // So is a contract, such as a multisig wallet.
    let wallet = contract(&fx.env, 0x33);
    assert!(register_factory(&fx.env, None, &fx.wasm_hash, &assets, &wallet).is_ok());
}

// The asset list and the code hash never change.

#[test]
fn no_mutating_entrypoints() {
    // Read from the compiled wasm, which is what gets deployed: the
    // constructor, create and the four views, and nothing that could
    // change the asset list, the code hash or the fee address.
    let entries = soroban_spec::read::from_wasm(super::factory_wasm()).unwrap();
    let functions: BTreeSet<String> = entries
        .iter()
        .filter_map(|e| match e {
            ScSpecEntry::FunctionV0(f) => Some(f.name.0.to_utf8_string_lossy()),
            _ => None,
        })
        .collect();
    let want: BTreeSet<String> = [
        "__constructor",
        "create",
        "fee_address",
        "mandate_wasm_hash",
        "supported_assets",
        "version",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(functions, want);
}

#[test]
fn supported_assets_are_fixed_at_construction() {
    let fx = Fx::new();
    let before = fx.client().supported_assets();

    // Both supported assets create a mandate; any other token never does,
    // however many mandates come before it.
    let mut config = fx.config();
    assert!(create(&fx, &config, &salt(&fx, 1)).is_ok());
    config.asset = fx.second_asset.clone();
    assert!(create(&fx, &config, &salt(&fx, 2)).is_ok());
    config.asset = fx.other_token.clone();
    assert_eq!(
        create(&fx, &config, &salt(&fx, 3)),
        Err(Refusal::Code(MandateError::AssetNotSupported))
    );

    assert_eq!(fx.client().supported_assets(), before);
    assert_eq!(fx.client().mandate_wasm_hash(), fx.wasm_hash);
}

#[test]
fn create_rejects_unsupported_asset() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.asset = fx.other_token.clone();
    let s = salt(&fx, 1);
    assert_eq!(
        create(&fx, &config, &s),
        Err(Refusal::Code(MandateError::AssetNotSupported))
    );
    assert_nothing_left(&fx, &fx.principal, &s);

    // An address that is not a token at all is refused the same way.
    config.asset = contract(&fx.env, 0x44);
    assert_eq!(
        create(&fx, &config, &s),
        Err(Refusal::Code(MandateError::AssetNotSupported))
    );
}

// Authorization.

/// Lets `signer` authorize exactly `create(config, salt)` on the factory,
/// and nothing else. The test host can only stand in for a signature from a
/// contract address, so these tests use a contract wallet as the principal;
/// `require_auth` treats an account the same way, and the shared vectors
/// sign as accounts with real keys.
fn sign_create(fx: &Fx, signer: &Address, config: &MandateConfig, salt: &BytesN<32>) {
    fx.env.mock_auths(&[MockAuth {
        address: signer,
        invoke: &MockAuthInvoke {
            contract: &fx.factory,
            fn_name: "create",
            args: (config.clone(), salt.clone()).into_val(&fx.env),
            sub_invokes: &[],
        },
    }]);
}

/// The base configuration with a contract wallet as its principal.
fn wallet_config(fx: &Fx) -> MandateConfig {
    let mut config = fx.config();
    config.principal = contract(&fx.env, 0x61);
    config
}

#[test]
fn create_requires_principal_auth() {
    let fx = Fx::new();
    let config = wallet_config(&fx);
    let principal = config.principal.clone();
    let s = salt(&fx, 1);

    // Nobody signed.
    assert_eq!(try_create(&fx, &config, &s), Err(Refusal::Host));
    assert_nothing_left(&fx, &principal, &s);

    // Someone other than the principal signed this exact call.
    sign_create(&fx, &contract(&fx.env, 0x62), &config, &s);
    assert_eq!(try_create(&fx, &config, &s), Err(Refusal::Host));
    assert_nothing_left(&fx, &principal, &s);

    // The fee address cannot create a mandate for the principal either.
    let fee = contract(&fx.env, 0x63);
    let assets = fx.list(&[&fx.asset]);
    let factory = register_factory(&fx.env, None, &fx.wasm_hash, &assets, &fee).unwrap();
    fx.env.mock_auths(&[MockAuth {
        address: &fee,
        invoke: &MockAuthInvoke {
            contract: &factory,
            fn_name: "create",
            args: (config.clone(), s.clone()).into_val(&fx.env),
            sub_invokes: &[],
        },
    }]);
    let got = crate::UjiraFactoryClient::new(&fx.env, &factory).try_create(&config, &s);
    assert!(matches!(got, Err(Err(_))), "{got:?}");

    // The principal's signature over this exact call is enough.
    sign_create(&fx, &principal, &config, &s);
    assert_eq!(
        try_create(&fx, &config, &s),
        Ok(fx.predicted(&principal, &s))
    );
}

#[test]
fn create_asks_the_principal_for_exactly_this_call_and_nothing_else() {
    let fx = Fx::new();
    let config = fx.config();
    let s = salt(&fx, 1);
    create(&fx, &config, &s).unwrap();
    assert_eq!(
        fx.env.auths(),
        std::vec![(
            fx.principal.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    fx.factory.clone(),
                    Symbol::new(&fx.env, "create"),
                    (config, s).into_val(&fx.env),
                )),
                sub_invocations: std::vec![],
            }
        )]
    );
}

#[test]
fn a_signature_for_another_salt_does_not_authorize() {
    let fx = Fx::new();
    let config = wallet_config(&fx);
    let used = salt(&fx, 2);
    sign_create(&fx, &config.principal, &config, &salt(&fx, 1));
    assert_eq!(try_create(&fx, &config, &used), Err(Refusal::Host));
    assert_nothing_left(&fx, &config.principal, &used);

    // Nor does one for another configuration.
    let mut signed = config.clone();
    signed.limits.total_budget = 1_000_000;
    sign_create(&fx, &config.principal, &signed, &used);
    assert_eq!(try_create(&fx, &config, &used), Err(Refusal::Host));
    assert_nothing_left(&fx, &config.principal, &used);
}

// The order of the checks: authorization, the asset, then the configuration,
// and only then the deploy.

#[test]
fn authorization_is_checked_before_the_asset() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.asset = fx.other_token.clone();
    assert_eq!(try_create(&fx, &config, &salt(&fx, 1)), Err(Refusal::Host));
}

#[test]
fn the_asset_is_checked_before_the_configuration() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.asset = fx.other_token.clone();
    config.limits.total_budget = 0;
    config.agents.push_back(fx.principal.clone());
    assert_eq!(
        create(&fx, &config, &salt(&fx, 1)),
        Err(Refusal::Code(MandateError::AssetNotSupported))
    );
}

#[test]
fn the_configuration_keeps_the_shared_validation_order() {
    // Where a configuration breaks several rules, the factory returns the
    // same first error as the shared validation, which the constructor
    // runs too.
    let fx = Fx::new();
    let cases: [(MandateError, Edit); 4] = [
        // A field domain comes before a role conflict and a full set.
        (CONFIG_INVALID, |fx, c| {
            c.limits.total_budget = 0;
            c.agents = accounts(fx, 100, 9);
            c.agents.push_back(fx.approver.clone());
        }),
        // A role conflict comes before a full set.
        (ROLE_CONFLICT, |fx, c| {
            c.agents = accounts(fx, 100, 9);
            c.agents.push_back(fx.approver.clone());
        }),
        // A full set comes before the approval gate.
        (ROLE_SET_FULL, |fx, c| {
            c.agents = accounts(fx, 100, 9);
            c.approvals_required = 5;
        }),
        // The approval gate on its own.
        (CONFIG_INVALID, |_, c| c.approvals_required = 2),
    ];
    for (i, (want, edit)) in cases.iter().enumerate() {
        let mut config = fx.config();
        edit(&fx, &mut config);
        let expected = fx.env.as_contract(&fx.factory, || {
            mandate_config::validate(&fx.env, &config, &fx.fee_address)
        });
        assert_eq!(expected, Err(*want), "case {i}: the shared validation");
        let s = salt(&fx, 10 + u8::try_from(i).unwrap());
        assert_eq!(
            create(&fx, &config, &s),
            Err(Refusal::Code(*want)),
            "case {i}"
        );
        assert_nothing_left(&fx, &fx.principal, &s);
    }
}

#[test]
fn constructor_error_is_returned() {
    // Each of the three codes a configuration can earn comes back to the
    // caller as itself, with no instance left behind.
    let fx = Fx::new();
    let cases: [(MandateError, Edit); 3] = [
        (CONFIG_INVALID, |_, c| c.limits.per_payment_cap = 0),
        (ROLE_CONFLICT, |fx, c| {
            c.executors.push_back(fx.principal.clone())
        }),
        (ROLE_SET_FULL, |fx, c| c.guardians = accounts(fx, 100, 5)),
    ];
    for (i, (want, edit)) in cases.iter().enumerate() {
        let mut config = fx.config();
        edit(&fx, &mut config);
        let s = salt(&fx, 1);
        assert_eq!(
            create(&fx, &config, &s),
            Err(Refusal::Code(*want)),
            "case {i}"
        );
        assert_nothing_left(&fx, &fx.principal, &s);
    }
}

#[test]
fn invalid_config_returns_contract_code_not_invalid_action() {
    // Through the deployer a constructor error is only ever the generic
    // Error(Context, InvalidAction). Getting the contract's own code back
    // shows the factory validated before it deployed. The call is made
    // untyped, so nothing between the host and this test reads the error.
    let fx = Fx::new();
    fx.env.mock_all_auths();
    let mut config = fx.config();
    config.limits.expires_at = super::NOW;
    let s = salt(&fx, 1);
    let got = fx.env.try_invoke_contract::<Address, soroban_sdk::Error>(
        &fx.factory,
        &Symbol::new(&fx.env, "create"),
        (config, s.clone()).into_val(&fx.env),
    );
    let Err(Ok(error)) = got else {
        panic!("create did not fail with an error: {got:?}");
    };
    assert_eq!(error, soroban_sdk::Error::from_contract_error(34));
    assert_eq!(error, soroban_sdk::Error::from(CONFIG_INVALID));
    assert_ne!(
        error,
        soroban_sdk::Error::from_type_and_code(
            xdr::ScErrorType::Context,
            xdr::ScErrorCode::InvalidAction
        )
    );
    assert_nothing_left(&fx, &fx.principal, &s);
}

#[test]
fn failed_create_leaves_nothing_and_retry_with_same_salt_works() {
    let fx = Fx::new();
    let s = salt(&fx, 1);
    let mut bad = fx.config();
    bad.recovery_threshold = 2;
    assert_eq!(create(&fx, &bad, &s), Err(Refusal::Code(CONFIG_INVALID)));
    assert_nothing_left(&fx, &fx.principal, &s);

    let mut unsupported = fx.config();
    unsupported.asset = fx.other_token.clone();
    assert_eq!(
        create(&fx, &unsupported, &s),
        Err(Refusal::Code(MandateError::AssetNotSupported))
    );
    assert_nothing_left(&fx, &fx.principal, &s);

    // The same salt then deploys cleanly to the address it always had.
    let mandate = create(&fx, &fx.config(), &s).unwrap();
    assert_eq!(mandate, fx.predicted(&fx.principal, &s));
    assert_eq!(UjiraMandateClient::new(&fx.env, &mandate).version(), 1);
}

// The deterministic address.

#[test]
fn address_matches_prediction() {
    let fx = Fx::new();
    let s = salt(&fx, 0x21);
    let predicted = fx.predicted(&fx.principal, &s);
    assert_eq!(executable_of(&fx.env, &predicted), None);
    let mandate = create(&fx, &fx.config(), &s).unwrap();
    assert_eq!(mandate, predicted);
}

#[test]
fn address_depends_on_the_principal_and_the_salt() {
    let fx = Fx::new();
    let s = salt(&fx, 1);
    let first = create(&fx, &fx.config(), &s).unwrap();

    // Another salt, another address.
    let other_salt = create(&fx, &fx.config(), &salt(&fx, 2)).unwrap();
    assert_ne!(other_salt, first);

    // Another principal with the same salt gets its own address, so one
    // principal can never take or block another's.
    let mut config = fx.config();
    config.principal = fx.account(50);
    let other_principal = create(&fx, &config, &s).unwrap();
    assert_ne!(other_principal, first);
    assert_eq!(other_principal, fx.predicted(&config.principal, &s));
}

#[test]
fn duplicate_salt_fails_and_leaves_the_first_mandate_alone() {
    let fx = Fx::new();
    let s = salt(&fx, 1);
    let first = create(&fx, &fx.config(), &s).unwrap();

    // The address is taken, so the deploy fails as a host error and the
    // whole call is undone, its event included.
    let mut again = fx.config();
    again.limits.total_budget = 20_000;
    again.tenant_ref = fx.b32(0x11);
    assert_eq!(create(&fx, &again, &s), Err(Refusal::Host));
    assert_eq!(fx.env.events().all().events().len(), 0);

    let view = UjiraMandateClient::new(&fx.env, &first).config();
    assert_eq!(view.limits.total_budget, 10_000);
    assert_eq!(view.tenant_ref, fx.b32(0x7e));
}

#[test]
fn the_mandate_runs_the_factory_code() {
    let fx = Fx::new();
    let mandate = create(&fx, &fx.config(), &salt(&fx, 1)).unwrap();
    assert_eq!(
        executable_of(&fx.env, &mandate),
        Some(xdr::ContractExecutable::Wasm(xdr::Hash(
            fx.wasm_hash.to_array()
        )))
    );
}

// Events.

#[test]
fn emits_mandate_created() {
    let fx = Fx::new();
    let config = fx.config();
    let mandate = create(&fx, &config, &salt(&fx, 1)).unwrap();

    // Exactly two events: the factory's, then the mandate's own.
    let created = MandateCreated {
        tenant_ref: config.tenant_ref.clone(),
        mandate: mandate.clone(),
        principal: config.principal.clone(),
        asset: config.asset.clone(),
        spec_version: SPEC_VERSION,
        fee_address: fx.fee_address.clone(),
    };
    let initialized = MandateInitialized {
        tenant_ref: config.tenant_ref.clone(),
        principal: config.principal.clone(),
        asset: config.asset.clone(),
        recipient_mode: config.recipient_mode,
        agents: config.agents.clone(),
        approvers: config.approvers.clone(),
        approvals_required: config.approvals_required,
        guardians: config.guardians.clone(),
        executors: config.executors.clone(),
        recovery: config.recovery.clone(),
        recovery_threshold: config.recovery_threshold,
        limits: config.limits.clone(),
        attester: config.attester.clone(),
        country_policy_hash: config.country_policy_hash.clone(),
        max_fee_bps: config.max_fee_bps,
        fee_address: fx.fee_address.clone(),
    };
    let events = fx.env.events().all();
    assert_eq!(
        events,
        [
            created.to_xdr(&fx.env, &fx.factory),
            initialized.to_xdr(&fx.env, &mandate)
        ]
    );

    // The wire shape, read without the event type: two topics, the name and
    // the tenant reference, and a map of the other five fields.
    let ContractEventBody::V0(body) = &events.events()[0].body;
    assert_eq!(
        body.topics.to_vec(),
        std::vec![
            ScVal::Symbol(xdr::ScSymbol("mandate_created".try_into().unwrap())),
            ScVal::Bytes(xdr::ScBytes([0x7e; 32].to_vec().try_into().unwrap())),
        ]
    );
    let ScVal::Map(Some(map)) = &body.data else {
        panic!("the data is not a map: {:?}", body.data);
    };
    let keys: std::vec::Vec<String> = map
        .iter()
        .map(|e| match &e.key {
            ScVal::Symbol(s) => s.0.to_utf8_string_lossy(),
            other => panic!("a key that is not a symbol: {other:?}"),
        })
        .collect();
    assert_eq!(
        keys,
        [
            "asset",
            "fee_address",
            "mandate",
            "principal",
            "spec_version"
        ]
    );
    let spec_version = map
        .iter()
        .find(
            |e| matches!(&e.key, ScVal::Symbol(s) if s.0.to_utf8_string_lossy() == "spec_version"),
        )
        .map(|e| e.val.clone());
    assert_eq!(spec_version, Some(ScVal::U32(1)));
}

// The fee address.

#[test]
fn fee_address_is_fixed_and_passed() {
    let fx = Fx::new();
    let config = fx.config();
    let mandate = create(&fx, &config, &salt(&fx, 1)).unwrap();

    // The mandate holds the factory's fee address; the principal had no
    // way to name another, since a configuration has no such field.
    let view = UjiraMandateClient::new(&fx.env, &mandate).config();
    assert_eq!(view.fee_address, fx.fee_address);
    assert_eq!(fx.client().fee_address(), fx.fee_address);

    // A second mandate, from another principal, gets the same one.
    let mut other = fx.config();
    other.principal = fx.account(50);
    let second = create(&fx, &other, &salt(&fx, 1)).unwrap();
    assert_eq!(
        UjiraMandateClient::new(&fx.env, &second)
            .config()
            .fee_address,
        fx.fee_address
    );

    // The factory validates against its own fee address: the fee address
    // can be neither the principal nor in any role.
    let fee = fx.fee_address.clone();
    let cases: [fn(&mut MandateConfig, &Address); 6] = [
        |c, fee| c.principal = fee.clone(),
        |c, fee| c.agents.push_back(fee.clone()),
        |c, fee| c.approvers.push_back(fee.clone()),
        |c, fee| c.guardians.push_back(fee.clone()),
        |c, fee| c.executors.push_back(fee.clone()),
        |c, fee| c.recovery.push_back(fee.clone()),
    ];
    for (i, edit) in cases.iter().enumerate() {
        let mut config = fx.config();
        edit(&mut config, &fee);
        // Keep the gates satisfiable so the role conflict is the only fault.
        config.approvals_required = config.approvers.len().min(1);
        config.recovery_threshold = config.recovery.len().min(1);
        let s = salt(&fx, 20 + u8::try_from(i).unwrap());
        assert_eq!(
            create(&fx, &config, &s),
            Err(Refusal::Code(ROLE_CONFLICT)),
            "case {i}"
        );
        assert_nothing_left(&fx, &config.principal, &s);
    }
}

#[test]
fn a_factory_with_another_fee_address_passes_its_own() {
    let fx = Fx::new();
    let fee = fx.account(77);
    let assets = fx.list(&[&fx.asset]);
    let factory = register_factory(&fx.env, None, &fx.wasm_hash, &assets, &fee).unwrap();
    fx.env.mock_all_auths();
    let mandate =
        crate::UjiraFactoryClient::new(&fx.env, &factory).create(&fx.config(), &salt(&fx, 1));
    assert_eq!(
        UjiraMandateClient::new(&fx.env, &mandate)
            .config()
            .fee_address,
        fee
    );
}
