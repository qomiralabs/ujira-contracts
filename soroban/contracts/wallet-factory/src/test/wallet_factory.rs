// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! What the factory promises: one pinned code, one address per configuration,
//! no powers, and no operator key or contract in a new wallet.

use super::*;
use crate::{Signer, WalletDeployed, WalletFactoryError};
use soroban_sdk::{
    testutils::Events as _, Address, Bytes, Env, Event as _, IntoVal, Map, Val, Vec,
};
use std::collections::BTreeSet;

fn no_policies(env: &Env) -> Map<Address, Val> {
    Map::new(env)
}

/// The fixtures are exactly what the pinned build produced, and a wallet runs
/// the account code given at construction, which no call can replace.
#[test]
fn deploys_only_pinned_hash() {
    let fx = Fx::new();
    assert_eq!(hex(&sha256(&fx.env, ACCOUNT_WASM)), pin("account"));
    assert_eq!(hex(&sha256(&fx.env, VERIFIER_WASM)), pin("verifier"));
    assert_eq!(hex(&sha256(&fx.env, POLICY_WASM)), pin("policy"));
    assert_eq!(hex(&fx.account_hash.to_array()), pin("account"));

    let factory = fx.factory();
    assert_eq!(factory.account_wasm_hash(), fx.account_hash);
    let wallet = factory.deploy(
        &Vec::from_array(&fx.env, [fx.passkey(1)]),
        &no_policies(&fx.env),
        &fx.salt(1),
    );
    assert_eq!(executable_of(&fx.env, &wallet), fx.account_hash);

    // Other code on the same network changes nothing: nothing a caller passes
    // names code, as the wasm's own interface declares.
    let other = fx.env.deployer().upload_contract_wasm(VERIFIER_WASM);
    assert_ne!(other, fx.account_hash);
    assert_eq!(inputs_of("deploy"), ["signers", "policies", "salt"]);
    assert_eq!(
        inputs_of("predict_address"),
        ["signers", "policies", "salt"]
    );
}

#[test]
fn deploy_emits_one_event() {
    let fx = Fx::new();
    let factory = fx.factory();
    let salt = fx.salt(1);
    let wallet = factory.deploy(
        &Vec::from_array(&fx.env, [fx.passkey(1)]),
        &no_policies(&fx.env),
        &salt,
    );
    let event = WalletDeployed {
        wallet,
        wasm_hash: fx.account_hash.clone(),
        salt,
    };
    assert_eq!(
        fx.env
            .events()
            .all()
            .filter_by_contract(&factory.address)
            .events(),
        [event.to_xdr(&fx.env, &factory.address)]
    );
}

/// The address follows from the configuration alone, computed here from XDR
/// the way a client would, without asking any contract.
#[test]
fn address_is_deterministic() {
    let fx = Fx::new();
    let factory = fx.factory();
    let signers = Vec::from_array(&fx.env, [fx.passkey(2), fx.passkey(1)]);
    let policies = no_policies(&fx.env);
    let salt = fx.salt(7);

    let verifier = ScAddress::from(&fx.verifier);
    let raw = [
        (verifier.clone(), raw(&passkey_key(&fx.env, 2))),
        (verifier, raw(&passkey_key(&fx.env, 1))),
    ];
    let offchain = offchain_address(&fx.env, &factory.address, &raw, &salt);
    assert_eq!(
        factory.predict_address(&signers, &policies, &salt),
        offchain
    );
    assert_eq!(factory.deploy(&signers, &policies, &salt), offchain);

    // The same configuration cannot make a second wallet.
    assert!(factory.try_deploy(&signers, &policies, &salt).is_err());

    // Order does not matter, and anything else that changes does.
    let sorted = Vec::from_array(&fx.env, [fx.passkey(1), fx.passkey(2)]);
    assert_eq!(factory.predict_address(&sorted, &policies, &salt), offchain);
    assert_ne!(
        factory.predict_address(&signers, &policies, &fx.salt(8)),
        offchain
    );
    let fewer = Vec::from_array(&fx.env, [fx.passkey(1)]);
    assert_ne!(factory.predict_address(&fewer, &policies, &salt), offchain);
    let other_factory = fx.factory();
    assert_ne!(
        other_factory.predict_address(&signers, &policies, &salt),
        offchain
    );
}

/// A different policy, or different install parameters, is another address.
#[test]
fn policies_change_the_address() {
    let fx = Fx::new();
    let factory = fx.factory();
    let signers = Vec::from_array(&fx.env, [fx.passkey(1), fx.passkey(2)]);
    let salt = fx.salt(1);
    let none = factory.predict_address(&signers, &no_policies(&fx.env), &salt);
    let two = factory.predict_address(&signers, &fx.threshold_policy(&signers, 2), &salt);
    let one = factory.predict_address(&signers, &fx.threshold_policy(&signers, 1), &salt);
    assert_ne!(none, two);
    assert_ne!(two, one);
}

/// The interface the compiled wasm declares has nothing that could change the
/// factory, or a wallet, after construction.
#[test]
fn has_no_admin_or_upgrade() {
    let names: BTreeSet<std::string::String> =
        functions().into_iter().map(|(name, _)| name).collect();
    let want: BTreeSet<std::string::String> = [
        "__constructor",
        "account_wasm_hash",
        "deny_list",
        "deploy",
        "predict_address",
    ]
    .into_iter()
    .map(std::string::String::from)
    .collect();
    assert_eq!(names, want);
    for name in &names {
        for word in [
            "admin", "owner", "upgrade", "set_", "fee", "pause", "transfer", "withdraw",
        ] {
            assert!(!name.contains(word), "{name} looks like a power");
        }
    }
}

/// Anyone may pay for a deploy: the factory asks nobody to authorize it, so a
/// host with no authorizations at all can run one.
#[test]
fn a_deploy_needs_no_authorization() {
    let fx = Fx::new();
    let factory = fx.factory();
    let signers = Vec::from_array(&fx.env, [fx.passkey(1), fx.passkey(2)]);
    let policies = fx.threshold_policy(&signers, 2);
    factory.deploy(&signers, &policies, &fx.salt(1));
}

#[test]
fn refuses_no_signers_and_repeats() {
    let fx = Fx::new();
    let factory = fx.factory();
    let salt = fx.salt(1);
    let none: Vec<Signer> = Vec::new(&fx.env);
    assert_eq!(
        code(factory.try_deploy(&none, &no_policies(&fx.env), &salt)),
        WalletFactoryError::NoSigners
    );
    let twice = Vec::from_array(&fx.env, [fx.passkey(1), fx.passkey(1)]);
    assert_eq!(
        code(factory.try_deploy(&twice, &no_policies(&fx.env), &salt)),
        WalletFactoryError::RepeatedSigner
    );
}

/// The factory itself and every denied address, in each place an address can
/// appear, and the raw key of a denied account under a verifier.
#[test]
fn refuses_itself_and_the_deny_list_as_signer_or_policy() {
    let fx = Fx::new();
    let env = &fx.env;
    let denied_contract = contract(env, 0xd1);
    let denied_account = account(env, 0xd2);
    let factory = fx.factory_with(&Vec::from_array(
        env,
        [denied_contract.clone(), denied_account.clone()],
    ));
    let salt = fx.salt(1);

    let signer_cases = [
        Signer::Delegated(factory.address.clone()),
        Signer::Delegated(denied_contract.clone()),
        Signer::Delegated(denied_account.clone()),
        Signer::External(factory.address.clone(), passkey_key(env, 3)),
        Signer::External(denied_contract.clone(), passkey_key(env, 3)),
        Signer::External(fx.verifier.clone(), Bytes::from_array(env, &[0xd2; 32])),
    ];
    for signer in signer_cases {
        let signers = Vec::from_array(env, [fx.passkey(1), signer.clone()]);
        assert_eq!(
            code(factory.try_deploy(&signers, &no_policies(env), &salt)),
            WalletFactoryError::SignerDenied,
            "{signer:?}"
        );
        assert_eq!(
            code(factory.try_predict_address(&signers, &no_policies(env), &salt)),
            WalletFactoryError::SignerDenied
        );
    }

    let signers = Vec::from_array(env, [fx.passkey(1)]);
    for policy in [factory.address.clone(), denied_contract, denied_account] {
        let mut policies: Map<Address, Val> = Map::new(env);
        policies.set(policy.clone(), 1u32.into_val(env));
        assert_eq!(
            code(factory.try_deploy(&signers, &policies, &salt)),
            WalletFactoryError::PolicyDenied,
            "{policy:?}"
        );
    }
}

/// A denied account's key blocks only that key.
#[test]
fn a_denied_account_does_not_block_other_keys() {
    let fx = Fx::new();
    let factory = fx.factory_with(&Vec::from_array(&fx.env, [account(&fx.env, 0xd2)]));
    factory.deploy(
        &Vec::from_array(&fx.env, [fx.passkey(1)]),
        &no_policies(&fx.env),
        &fx.salt(1),
    );
}

#[test]
fn constructor_refuses_a_repeated_deny_entry() {
    let fx = Fx::new();
    let denied = contract(&fx.env, 0xd1);
    let list = Vec::from_array(&fx.env, [denied.clone(), denied]);
    assert_eq!(
        fx.try_factory_with(&list).err(),
        Some(WalletFactoryError::DenyListInvalid)
    );
}

/// What the factory builds is a working account holding exactly what it was
/// given: one `Default` rule with those signers and that threshold policy.
#[test]
fn the_wallet_holds_exactly_the_configuration_given() {
    let fx = Fx::new();
    let factory = fx.factory();
    let signers = Vec::from_array(&fx.env, [fx.passkey(1), fx.passkey(2), fx.passkey(3)]);
    let policies = fx.threshold_policy(&signers, 2);
    let wallet = factory.deploy(&signers, &policies, &fx.salt(1));

    assert_eq!(rules_count(&fx.env, &wallet), 1);
    let only = rule(&fx.env, &wallet, 0);
    assert_eq!(only.context_type, ContextRuleType::Default);
    assert_eq!(only.signers.len(), 3);
    for signer in signers.iter() {
        assert!(only.signers.contains(&signer));
    }
    assert_eq!(
        only.policies,
        Vec::from_array(&fx.env, [fx.threshold.clone()])
    );
    assert_eq!(only.valid_until, None);
    assert_eq!(threshold_of(&fx.env, &fx.threshold, 0, &wallet), 2);
}

/// The account's own check sees through another encoding of one key, which
/// only the verifier can do, so the factory leaves it to the account.
#[test]
fn the_account_refuses_one_key_in_two_encodings() {
    let fx = Fx::new();
    let factory = fx.factory();
    let signers = Vec::from_array(
        &fx.env,
        [
            fx.passkey(1),
            Signer::External(fx.verifier.clone(), passkey_key_with_credential(&fx.env, 1)),
        ],
    );
    assert!(factory
        .try_deploy(&signers, &no_policies(&fx.env), &fx.salt(1))
        .is_err());
}
