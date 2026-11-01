// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The constructor's checks, in the order they run: a failing and a passing
//! case for every rule, then the order between rules, then the effects.

use super::{key_of, CreateError, Fx, NOW};
use crate::events::MandateInitialized;
use crate::types::ConfigView;
use crate::{MandateConfig, MandateError, UjiraMandateClient};
use mandate_config::{MODE_ALLOWLIST, MODE_ATTESTED, MODE_BOTH};
use soroban_sdk::testutils::{Address as _, Events as _};
use soroban_sdk::{xdr::ToXdr, Address, Bytes, Event, Vec};

/// Applies `edit` to the base configuration on a fresh host and creates it.
fn create_with(edit: impl FnOnce(&Fx, &mut MandateConfig)) -> Result<Address, CreateError> {
    let fx = Fx::new();
    let mut config = fx.config();
    edit(&fx, &mut config);
    fx.create(&config)
}

fn refused(want: MandateError, edit: impl FnOnce(&Fx, &mut MandateConfig)) {
    assert_eq!(create_with(edit), Err(CreateError::Mandate(want)));
}

fn accepted(edit: impl FnOnce(&Fx, &mut MandateConfig)) {
    let got = create_with(edit);
    assert!(
        got.is_ok(),
        "expected the configuration to pass, got {got:?}"
    );
}

fn accounts(fx: &Fx, first: u8, n: u8) -> Vec<Address> {
    let mut out = Vec::new(&fx.env);
    for i in 0..n {
        out.push_back(fx.account(first + i));
    }
    out
}

fn attester(fx: &Fx, seed: u8) -> Option<Bytes> {
    Some(key_of(&fx.env, seed))
}

const CONFIG_INVALID: MandateError = MandateError::ConfigInvalid;
const ROLE_CONFLICT: MandateError = MandateError::RoleConflict;
const ROLE_SET_FULL: MandateError = MandateError::RoleSetFull;

// Field domains and cross-field rules.

#[test]
fn constructor_accepts_the_base_configuration() {
    accepted(|_, _| {});
}

#[test]
fn constructor_refuses_an_unknown_recipient_mode() {
    refused(CONFIG_INVALID, |_, c| c.recipient_mode = 0);
    refused(CONFIG_INVALID, |fx, c| {
        c.recipient_mode = 4;
        c.attester = attester(fx, 0x42);
    });
}

#[test]
fn constructor_accepts_every_recipient_mode() {
    accepted(|_, c| c.recipient_mode = MODE_ALLOWLIST);
    accepted(|fx, c| {
        c.recipient_mode = MODE_ATTESTED;
        c.attester = attester(fx, 0x42);
    });
    accepted(|fx, c| {
        c.recipient_mode = MODE_BOTH;
        c.attester = attester(fx, 0x42);
    });
}

#[test]
fn constructor_refuses_total_budget_below_one() {
    refused(CONFIG_INVALID, |_, c| c.limits.total_budget = 0);
    refused(CONFIG_INVALID, |_, c| c.limits.total_budget = -1);
    refused(CONFIG_INVALID, |_, c| c.limits.total_budget = i128::MIN);
}

#[test]
fn constructor_accepts_total_budget_at_its_edges() {
    accepted(|_, c| {
        c.limits.total_budget = 1;
        c.limits.per_payment_cap = 1;
    });
    accepted(|_, c| c.limits.total_budget = i128::MAX);
}

#[test]
fn constructor_refuses_per_payment_cap_outside_one_to_budget() {
    refused(CONFIG_INVALID, |_, c| c.limits.per_payment_cap = 0);
    refused(CONFIG_INVALID, |_, c| c.limits.per_payment_cap = -5);
    refused(CONFIG_INVALID, |_, c| {
        c.limits.per_payment_cap = c.limits.total_budget + 1;
    });
}

#[test]
fn constructor_accepts_per_payment_cap_at_its_edges() {
    accepted(|_, c| {
        c.limits.per_payment_cap = 1;
        c.limits.approval_threshold = 0;
    });
    accepted(|_, c| c.limits.per_payment_cap = c.limits.total_budget);
}

#[test]
fn constructor_refuses_daily_cap_below_one() {
    refused(CONFIG_INVALID, |_, c| c.limits.per_recipient_daily_cap = 0);
    refused(CONFIG_INVALID, |_, c| c.limits.per_recipient_daily_cap = -1);
}

#[test]
fn constructor_accepts_daily_cap_at_its_edges() {
    accepted(|_, c| c.limits.per_recipient_daily_cap = 1);
    accepted(|_, c| c.limits.per_recipient_daily_cap = i128::MAX);
}

#[test]
fn constructor_refuses_a_negative_approval_threshold() {
    refused(CONFIG_INVALID, |_, c| c.limits.approval_threshold = -1);
}

#[test]
fn constructor_accepts_approval_threshold_at_its_edges() {
    accepted(|_, c| c.limits.approval_threshold = 0);
    accepted(|_, c| c.limits.approval_threshold = i128::MAX);
}

#[test]
fn constructor_refuses_payout_ttl_outside_its_bounds() {
    refused(CONFIG_INVALID, |_, c| c.limits.payout_ttl_seconds = 0);
    refused(CONFIG_INVALID, |_, c| c.limits.payout_ttl_seconds = 3_599);
    refused(CONFIG_INVALID, |_, c| {
        c.limits.payout_ttl_seconds = 2_592_001
    });
}

#[test]
fn constructor_accepts_payout_ttl_at_its_bounds() {
    accepted(|_, c| c.limits.payout_ttl_seconds = 3_600);
    accepted(|_, c| c.limits.payout_ttl_seconds = 2_592_000);
}

#[test]
fn constructor_refuses_an_expiry_not_after_now() {
    refused(CONFIG_INVALID, |_, c| c.limits.expires_at = NOW);
    refused(CONFIG_INVALID, |_, c| c.limits.expires_at = NOW - 1);
    refused(CONFIG_INVALID, |_, c| c.limits.expires_at = 0);
}

#[test]
fn constructor_accepts_an_expiry_one_second_after_now() {
    accepted(|_, c| c.limits.expires_at = NOW + 1);
    accepted(|_, c| c.limits.expires_at = u64::MAX);
}

#[test]
fn constructor_refuses_max_fee_bps_above_100() {
    refused(CONFIG_INVALID, |_, c| c.max_fee_bps = 101);
    refused(CONFIG_INVALID, |_, c| c.max_fee_bps = u32::MAX);
}

#[test]
fn constructor_accepts_max_fee_bps_at_its_edges() {
    accepted(|_, c| c.max_fee_bps = 0);
    accepted(|_, c| c.max_fee_bps = 100);
}

#[test]
fn constructor_refuses_a_member_repeated_within_a_role_set() {
    refused(CONFIG_INVALID, |fx, c| {
        c.agents = fx.list(&[&fx.agent, &fx.account(20), &fx.agent]);
    });
    refused(CONFIG_INVALID, |fx, c| {
        c.approvers = fx.list(&[&fx.approver, &fx.approver]);
    });
    refused(CONFIG_INVALID, |fx, c| {
        c.guardians = fx.list(&[&fx.guardian, &fx.guardian]);
    });
    refused(CONFIG_INVALID, |fx, c| {
        c.executors = fx.list(&[&fx.executor, &fx.executor]);
    });
    refused(CONFIG_INVALID, |fx, c| {
        c.recovery = fx.list(&[&fx.recovery, &fx.recovery]);
    });
}

#[test]
fn constructor_accepts_distinct_members_and_empty_optional_sets() {
    accepted(|fx, c| c.agents = fx.list(&[&fx.agent, &fx.account(20)]));
    accepted(|fx, c| {
        c.guardians = fx.list(&[]);
        c.executors = fx.list(&[]);
        c.recovery = fx.list(&[]);
        c.recovery_threshold = 0;
    });
}

// The attester is present exactly when the mode needs it.

#[test]
fn constructor_refuses_an_attester_on_an_allowlist_mandate() {
    refused(CONFIG_INVALID, |fx, c| c.attester = attester(fx, 0x42));
}

#[test]
fn constructor_refuses_an_attested_mandate_without_an_attester() {
    refused(CONFIG_INVALID, |_, c| c.recipient_mode = MODE_ATTESTED);
    refused(CONFIG_INVALID, |_, c| c.recipient_mode = MODE_BOTH);
}

#[test]
fn constructor_refuses_an_attester_that_is_not_32_bytes() {
    for len in [0_u32, 31, 33, 64] {
        refused(CONFIG_INVALID, |fx, c| {
            c.recipient_mode = MODE_ATTESTED;
            let mut key = Bytes::new(&fx.env);
            for _ in 0..len {
                key.push_back(0x42);
            }
            c.attester = Some(key);
        });
    }
}

#[test]
fn constructor_refuses_an_all_zero_attester() {
    refused(CONFIG_INVALID, |fx, c| {
        c.recipient_mode = MODE_BOTH;
        c.attester = Some(Bytes::from_array(&fx.env, &[0; 32]));
    });
}

#[test]
fn constructor_accepts_a_well_formed_attester() {
    // One non-zero byte is enough; whether the key is a usable curve point
    // is checked off chain.
    accepted(|fx, c| {
        c.recipient_mode = MODE_ATTESTED;
        let mut raw = [0_u8; 32];
        raw[31] = 1;
        c.attester = Some(Bytes::from_array(&fx.env, &raw));
    });
}

// Role separation.

#[test]
fn constructor_refuses_the_principal_as_an_agent_or_executor() {
    refused(ROLE_CONFLICT, |fx, c| {
        c.agents.push_back(fx.principal.clone());
    });
    refused(ROLE_CONFLICT, |fx, c| {
        c.executors.push_back(fx.principal.clone());
    });
}

#[test]
fn constructor_refuses_an_agent_that_approves() {
    refused(ROLE_CONFLICT, |fx, c| {
        c.approvers.push_back(fx.agent.clone())
    });
}

#[test]
fn constructor_refuses_an_executor_that_approves() {
    refused(ROLE_CONFLICT, |fx, c| {
        c.approvers.push_back(fx.executor.clone());
    });
}

#[test]
fn constructor_refuses_a_recovery_approver_that_spends_or_submits() {
    refused(ROLE_CONFLICT, |fx, c| {
        c.recovery.push_back(fx.agent.clone())
    });
    refused(ROLE_CONFLICT, |fx, c| {
        c.recovery.push_back(fx.executor.clone());
    });
}

#[test]
fn constructor_refuses_an_agent_that_is_an_executor() {
    refused(ROLE_CONFLICT, |fx, c| {
        c.executors.push_back(fx.agent.clone())
    });
}

#[test]
fn constructor_create_with_principal_in_recovery_gives_role_conflict() {
    refused(ROLE_CONFLICT, |fx, c| {
        c.recovery.push_back(fx.principal.clone());
        c.recovery_threshold = 1;
    });
}

#[test]
fn constructor_refuses_the_fee_address_as_principal_or_in_any_role() {
    refused(ROLE_CONFLICT, |fx, c| c.principal = fx.fee_address.clone());
    refused(ROLE_CONFLICT, |fx, c| {
        c.agents.push_back(fx.fee_address.clone());
    });
    refused(ROLE_CONFLICT, |fx, c| {
        c.approvers.push_back(fx.fee_address.clone());
    });
    refused(ROLE_CONFLICT, |fx, c| {
        c.guardians.push_back(fx.fee_address.clone());
    });
    refused(ROLE_CONFLICT, |fx, c| {
        c.executors.push_back(fx.fee_address.clone());
    });
    refused(ROLE_CONFLICT, |fx, c| {
        c.recovery.push_back(fx.fee_address.clone());
    });
}

#[test]
fn constructor_accepts_the_allowed_overlaps() {
    // The principal may approve and guard.
    accepted(|fx, c| {
        c.approvers.push_back(fx.principal.clone());
        c.guardians.push_back(fx.principal.clone());
    });
    // A guardian may hold any other role.
    accepted(|fx, c| c.approvers.push_back(fx.guardian.clone()));
    accepted(|fx, c| c.agents.push_back(fx.guardian.clone()));
    accepted(|fx, c| c.executors.push_back(fx.guardian.clone()));
    accepted(|fx, c| c.recovery.push_back(fx.guardian.clone()));
    // A recovery approver may approve and guard.
    accepted(|fx, c| {
        c.approvers.push_back(fx.recovery.clone());
        c.guardians.push_back(fx.recovery.clone());
    });
}

// The attester key holds no role.

#[test]
fn constructor_refuses_an_attester_key_that_is_a_role_accounts_key() {
    // The base actors' keys are 32 bytes of their seed: principal 1, agent
    // 2, approver 3, guardian 4, executor 5, recovery 6, fee address 7.
    for seed in 1..=7_u8 {
        refused(ROLE_CONFLICT, |fx, c| {
            c.recipient_mode = MODE_BOTH;
            c.attester = attester(fx, seed);
        });
    }
}

#[test]
fn constructor_accepts_an_attester_key_no_role_account_holds() {
    accepted(|fx, c| {
        c.recipient_mode = MODE_ATTESTED;
        c.attester = attester(fx, 0x42);
    });
}

#[test]
fn constructor_never_matches_a_contract_address_against_the_attester() {
    // A contract address has no ed25519 key, so even an attester equal to
    // its contract ID bytes is not a match.
    let fx = Fx::new();
    let wallet = Address::generate(&fx.env);
    // A contract address is the value type, the address type, then the ID.
    let xdr = wallet.clone().to_xdr(&fx.env);
    assert_eq!(xdr.len(), 40);
    let id = xdr.slice(8..40);
    let mut c = fx.config();
    c.recipient_mode = MODE_ATTESTED;
    c.guardians.push_back(wallet);
    c.attester = Some(id);
    assert!(fx.create(&c).is_ok());
}

// Role set bounds.

#[test]
fn constructor_refuses_a_role_set_over_its_bound() {
    refused(ROLE_SET_FULL, |fx, c| c.agents = accounts(fx, 20, 9));
    refused(ROLE_SET_FULL, |fx, c| c.approvers = accounts(fx, 40, 11));
    refused(ROLE_SET_FULL, |fx, c| c.guardians = accounts(fx, 60, 5));
    refused(ROLE_SET_FULL, |fx, c| c.executors = accounts(fx, 70, 5));
    refused(ROLE_SET_FULL, |fx, c| c.recovery = accounts(fx, 80, 11));
}

#[test]
fn constructor_accepts_every_role_set_at_its_bound() {
    accepted(|fx, c| {
        c.agents = accounts(fx, 20, 8);
        c.approvers = accounts(fx, 40, 10);
        c.approvals_required = 10;
        c.guardians = accounts(fx, 60, 4);
        c.executors = accounts(fx, 70, 4);
        c.recovery = accounts(fx, 80, 10);
        c.recovery_threshold = 10;
    });
}

// approvals_required fits the approvers.

#[test]
fn constructor_refuses_an_approval_gate_nobody_can_meet() {
    // No approvers: nothing may need an approval.
    refused(CONFIG_INVALID, |fx, c| {
        c.approvers = fx.list(&[]);
        c.approvals_required = 1;
        c.limits.approval_threshold = c.limits.per_payment_cap + 1;
    });
    refused(CONFIG_INVALID, |fx, c| {
        c.approvers = fx.list(&[]);
        c.approvals_required = 0;
        c.limits.approval_threshold = c.limits.per_payment_cap;
    });
    // With approvers: between 1 and their number.
    refused(CONFIG_INVALID, |_, c| c.approvals_required = 0);
    refused(CONFIG_INVALID, |_, c| c.approvals_required = 2);
}

#[test]
fn constructor_accepts_an_approval_gate_that_can_be_met() {
    accepted(|fx, c| {
        c.approvers = fx.list(&[]);
        c.approvals_required = 0;
        c.limits.approval_threshold = c.limits.per_payment_cap + 1;
    });
    accepted(|fx, c| {
        c.approvers = fx.list(&[&fx.approver, &fx.account(41)]);
        c.approvals_required = 2;
    });
}

// recovery_threshold fits the recovery set.

#[test]
fn constructor_refuses_a_recovery_threshold_outside_the_set() {
    refused(CONFIG_INVALID, |fx, c| {
        c.recovery = fx.list(&[]);
        c.recovery_threshold = 1;
    });
    refused(CONFIG_INVALID, |_, c| c.recovery_threshold = 0);
    refused(CONFIG_INVALID, |_, c| c.recovery_threshold = 2);
}

#[test]
fn constructor_accepts_a_recovery_threshold_within_the_set() {
    accepted(|fx, c| {
        c.recovery = fx.list(&[]);
        c.recovery_threshold = 0;
    });
    accepted(|fx, c| {
        c.recovery = fx.list(&[&fx.recovery, &fx.account(81)]);
        c.recovery_threshold = 2;
    });
}

// The first failing check decides.

#[test]
fn constructor_reports_the_first_failing_check() {
    // A field domain before the attester's form.
    refused(CONFIG_INVALID, |fx, c| {
        c.limits.payout_ttl_seconds = 0;
        c.attester = attester(fx, 0x42);
    });
    // The attester's form before role separation.
    refused(CONFIG_INVALID, |fx, c| {
        c.attester = attester(fx, 0x42);
        c.agents.push_back(fx.principal.clone());
    });
    // A field domain before role separation.
    refused(CONFIG_INVALID, |fx, c| {
        c.max_fee_bps = 101;
        c.approvers.push_back(fx.agent.clone());
    });
    // Role separation before set bounds.
    refused(ROLE_CONFLICT, |fx, c| {
        c.agents = accounts(fx, 20, 9);
        c.agents.push_back(fx.principal.clone());
    });
    // The attester key overlap before set bounds.
    refused(ROLE_CONFLICT, |fx, c| {
        c.recipient_mode = MODE_ATTESTED;
        c.attester = attester(fx, 1);
        c.agents = accounts(fx, 20, 9);
    });
    // Set bounds before the approval gate.
    refused(ROLE_SET_FULL, |fx, c| {
        c.agents = accounts(fx, 20, 9);
        c.approvals_required = 2;
    });
    // The approval gate before the recovery threshold.
    refused(CONFIG_INVALID, |_, c| {
        c.approvals_required = 2;
        c.recovery_threshold = 5;
    });
}

// Effects.

#[test]
fn constructor_stores_the_configuration_and_emits_mandate_initialized() {
    let fx = Fx::new();
    let config = fx.config();
    let id = fx.create(&config).unwrap();
    let event = MandateInitialized {
        tenant_tag: config.tenant_tag.clone(),
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
        attester: None,
        country_policy_hash: config.country_policy_hash.clone(),
        max_fee_bps: config.max_fee_bps,
        fee_address: fx.fee_address.clone(),
    };
    // Exactly one event, from the mandate.
    assert_eq!(fx.env.events().all(), [event.to_xdr(&fx.env, &id)]);

    let view = UjiraMandateClient::new(&fx.env, &id).config();
    assert_eq!(view, expected_view(&config, &fx.fee_address));
}

fn expected_view(c: &MandateConfig, fee_address: &Address) -> ConfigView {
    ConfigView {
        principal: c.principal.clone(),
        asset: c.asset.clone(),
        recipient_mode: c.recipient_mode,
        tenant_tag: c.tenant_tag.clone(),
        agents: c.agents.clone(),
        approvers: c.approvers.clone(),
        approvals_required: c.approvals_required,
        guardians: c.guardians.clone(),
        executors: c.executors.clone(),
        recovery: c.recovery.clone(),
        recovery_threshold: c.recovery_threshold,
        limits: c.limits.clone(),
        attester: c.attester.clone(),
        country_policy_hash: c.country_policy_hash.clone(),
        max_fee_bps: c.max_fee_bps,
        fee_address: fee_address.clone(),
        proposed_principal: None,
        recovery_epoch: 0,
        agent_epoch: 0,
    }
}

#[test]
fn constructor_starts_every_epoch_and_counter_at_zero() {
    let fx = Fx::new();
    let m = fx.mandate();
    let cfg = m.config();
    assert_eq!(cfg.agent_epoch, 0);
    assert_eq!(cfg.recovery_epoch, 0);
    assert_eq!(cfg.proposed_principal, None);
    let st = m.state();
    assert_eq!(st.policy_epoch, 0);
    assert_eq!((st.spent, st.pending, st.committed), (0, 0, 0));
    assert!(!st.paused && !st.revoked && !st.expired);
}

#[test]
fn constructor_stores_an_attested_configuration_as_given() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.recipient_mode = MODE_BOTH;
    config.attester = Some(key_of(&fx.env, 0x42));
    config.max_fee_bps = 100;
    let id = fx.create(&config).unwrap();
    let view = UjiraMandateClient::new(&fx.env, &id).config();
    assert_eq!(view, expected_view(&config, &fx.fee_address));
}
