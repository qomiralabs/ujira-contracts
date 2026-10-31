// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The contract's interface as the compiled wasm declares it, and the
//! authorization every state-changing entry point demands.

extern crate std;

use super::{mandate_wasm, Fx};
use crate::types::{AllowEntry, Leg, ROLE_GUARDIAN};
use crate::{MandateError, UjiraMandateClient};
use core::fmt::Debug;
use soroban_sdk::{
    testutils::Address as _,
    token::StellarAssetClient,
    xdr::{Limited, Limits, ReadXdr, ScSpecEntry, ScSpecTypeDef},
    Address, InvokeError, MuxedAddress, Vec,
};
use std::collections::BTreeMap;
use std::string::String;

fn leb128(b: &[u8], i: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = b[*i];
        *i += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// The payload of the wasm's `contractspecv0` custom section, which is what
/// a network and every client read as the contract's interface.
fn spec_section(wasm: &[u8]) -> &[u8] {
    let mut i = 8;
    while i < wasm.len() {
        let id = wasm[i];
        i += 1;
        let size = leb128(wasm, &mut i);
        let end = i + size;
        if id == 0 {
            let mut j = i;
            let n = leb128(wasm, &mut j);
            if &wasm[j..j + n] == b"contractspecv0" {
                return &wasm[j + n..end];
            }
        }
        i = end;
    }
    panic!("the wasm has no contract spec");
}

/// Every function's inputs, by name, as the wasm declares them.
fn functions() -> BTreeMap<String, std::vec::Vec<(String, ScSpecTypeDef)>> {
    let mut r = Limited::new(
        std::io::Cursor::new(spec_section(mandate_wasm())),
        Limits::none(),
    );
    let mut out = BTreeMap::new();
    for entry in ScSpecEntry::read_xdr_iter(&mut r) {
        if let ScSpecEntry::FunctionV0(f) = entry.unwrap() {
            let inputs = f
                .inputs
                .iter()
                .map(|i| (i.name.to_utf8_string_lossy(), i.type_.clone()))
                .collect();
            out.insert(f.name.to_utf8_string_lossy(), inputs);
        }
    }
    out
}

#[test]
fn withdraw_and_sweep_have_no_address_argument() {
    let fns = functions();
    // Both always pay the principal. `withdraw` takes only the amount, and
    // `sweep` only the caller it authorizes, never a destination.
    assert_eq!(
        fns["withdraw"],
        [(String::from("amount"), ScSpecTypeDef::I128)]
    );
    assert_eq!(
        fns["sweep"],
        [(String::from("caller"), ScSpecTypeDef::Address)]
    );
    // Revoke takes nothing at all.
    assert_eq!(fns["revoke"], []);
}

type Outcome<T, C> = Result<Result<T, C>, Result<MandateError, InvokeError>>;

/// Runs `call` with no authorization at all, which must fail in the host
/// rather than with a mandate code, then again with every signature it
/// asks for, which must then succeed.
fn needs_auth<T: Debug, C: Debug>(fx: &Fx, name: &str, call: impl Fn() -> Outcome<T, C>) {
    assert_eq!(needs_auth_code(fx, name, call), None, "{name}");
}

/// As `needs_auth`, but the signed run may be refused by the mandate, and
/// its error is returned.
fn needs_auth_code<T: Debug, C: Debug>(
    fx: &Fx,
    name: &str,
    call: impl Fn() -> Outcome<T, C>,
) -> Option<MandateError> {
    fx.env.set_auths(&[]);
    let refused = call();
    fx.env.mock_all_auths();
    assert!(
        matches!(refused, Err(Err(_))),
        "{name} without a signature: {refused:?}"
    );
    match call() {
        Ok(Ok(_)) => None,
        Err(Ok(e)) => Some(e),
        other => panic!("{name} with its signature: {other:?}"),
    }
}

#[test]
fn every_entrypoint_requires_auth() {
    let fx = Fx::new();
    let principal = Address::generate(&fx.env);
    let mut config = fx.config();
    config.principal = principal.clone();
    config.max_fee_bps = 50;
    let m: UjiraMandateClient = fx.ready_with(&config);
    let other = fx
        .env
        .register_stellar_asset_contract_v2(fx.account(97))
        .address();
    StellarAssetClient::new(&fx.env, &other).mint(&m.address, &5);
    let (a, g) = (&fx.agent, &fx.guardian);
    let id = |n| fx.b32(n);

    needs_auth(&fx, "request_payout", || {
        m.try_request_payout(a, &fx.req(1, 500))
    });
    needs_auth(&fx, "request_payouts", || {
        m.try_request_payouts(a, &Vec::from_array(&fx.env, [fx.req(2, 100)]))
    });
    needs_auth(&fx, "approve", || m.try_approve(&fx.approver, &id(1)));
    m.request_payout(a, &fx.req(3, 500));
    needs_auth(&fx, "reject", || m.try_reject(&fx.approver, &id(3)));
    needs_auth(&fx, "cancel", || m.try_cancel(a, &id(1)));
    needs_auth(&fx, "cancel_batch", || {
        m.try_cancel_batch(a, &Vec::from_array(&fx.env, [id(2)]))
    });
    // A contract destination holds the asset without a trustline, so the
    // executes below pay out for real.
    let vault = MuxedAddress::from(Address::generate(&fx.env));
    fx.allow(&m.address, &fx.bob(), &vault);
    let to_vault = |n: u8| {
        let mut r = fx.req(n, 100);
        r.legs = Vec::from_array(
            &fx.env,
            [Leg {
                destination: vault.clone(),
                amount: 100,
            }],
        );
        r
    };
    m.request_payout(a, &to_vault(4));
    m.request_payout(a, &to_vault(5));
    needs_auth(&fx, "execute", || m.try_execute(a, &id(4)));
    needs_auth(&fx, "execute_batch", || {
        m.try_execute_batch(&fx.executor, &Vec::from_array(&fx.env, [id(5)]))
    });
    assert_eq!(m.state().spent, 200);
    needs_auth(&fx, "pause", || m.try_pause(g));
    needs_auth(&fx, "resume", || m.try_resume());
    let limits = m.config().limits;
    needs_auth(&fx, "update_limits", || m.try_update_limits(&limits));
    let newbie = fx.account(30);
    needs_auth(&fx, "add_role", || m.try_add_role(&ROLE_GUARDIAN, &newbie));
    needs_auth(&fx, "remove_role", || {
        m.try_remove_role(&ROLE_GUARDIAN, &newbie)
    });
    needs_auth(&fx, "set_approvals_required", || {
        m.try_set_approvals_required(&1)
    });
    needs_auth(&fx, "set_recovery_threshold", || {
        m.try_set_recovery_threshold(&1)
    });
    let entries = Vec::from_array(
        &fx.env,
        [AllowEntry {
            recipient_ref: id(0x61),
            destination: fx.bob_wallet(),
        }],
    );
    needs_auth(&fx, "allow_recipients", || m.try_allow_recipients(&entries));
    needs_auth(&fx, "disallow_recipients", || {
        m.try_disallow_recipients(&entries)
    });
    let key = super::key_of(&fx.env, 0xa9);
    let got = needs_auth_code(&fx, "set_attestation_policy", || {
        m.try_set_attestation_policy(&key, &id(1))
    });
    assert_eq!(got, Some(MandateError::WrongRecipientMode));
    needs_auth(&fx, "lower_fee_cap", || m.try_lower_fee_cap(&10));
    let heir = fx.account(44);
    needs_auth(&fx, "propose_principal", || m.try_propose_principal(&heir));
    needs_auth(&fx, "cancel_principal_proposal", || {
        m.try_cancel_principal_proposal()
    });
    needs_auth(&fx, "withdraw", || m.try_withdraw(&1));
    needs_auth(&fx, "recover_token", || m.try_recover_token(&other, &1));
    needs_auth(&fx, "recovery_rotate_agent", || {
        m.try_recovery_rotate_agent(&fx.recovery, a, &None)
    });
    needs_auth(&fx, "recovery_revoke", || {
        m.try_recovery_revoke(&fx.recovery)
    });
    let got = needs_auth_code(&fx, "revoke", || m.try_revoke());
    assert_eq!(got, Some(MandateError::MandateRevoked));
    needs_auth(&fx, "sweep", || m.try_sweep(&principal));
    m.propose_principal(&heir);
    needs_auth(&fx, "accept_principal", || m.try_accept_principal());
    assert_eq!(m.config().principal, heir);
}
