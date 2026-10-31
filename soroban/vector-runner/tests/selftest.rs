// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Proves the runner and the comparator without the real mandate: against
//! traces built from a vector and changed one thing at a time, and against
//! the test-only reference factory and mandate compiled to wasm.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use vector_runner::compare::{compare_traces, compare_vector, echo_trace, Mismatch};
use vector_runner::run::{run, select, Options};
use vector_runner::spec::Spec;
use vector_runner::stellar::{self, executable_hash, Contracts, World};
use vector_runner::trace;
use vector_runner::values::sha256;
use vector_runner::vectors::{self, parse_only_calls, Selection, Vector};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn here(p: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(p)
}

fn reference_options(files: &[&str]) -> Options {
    Options {
        chain: String::from("stellar"),
        root: root(),
        files: Some(files.iter().map(|f| here(f)).collect()),
        selection: Selection::default(),
        factory_wasm: here("fixtures/vector_runner_reference_factory.wasm"),
        mandate_wasm: here("fixtures/vector_runner_reference_mandate.wasm"),
    }
}

fn spec_vector(file: &str, id: &str) -> Vector {
    let all = vectors::load(&[root().join("spec/vectors").join(file)]).unwrap();
    all.into_iter().find(|v| v.id == id).unwrap()
}

fn mismatch(v: &Vector, tv: &Value) -> Mismatch {
    compare_vector(v, tv, "stellar").expect_err("the comparator accepted a wrong trace")
}

fn events_mut(tv: &mut Value, step: usize) -> &mut Vec<Value> {
    tv["steps"][step - 1]["events"].as_array_mut().unwrap()
}

#[test]
fn echo_trace_of_every_shared_vector_passes() {
    // The comparator agrees with a chain that does exactly what each vector
    // says, for every vector that runs on Stellar, and the trace survives a
    // round trip through its text form unchanged.
    let files = vectors::discover(&root().join("spec/vectors")).unwrap();
    let all = vectors::load(&files).unwrap();
    assert!(all.len() > 100, "the shared vectors went missing");
    let mut entries = Vec::new();
    for v in all.iter().filter(|v| v.applies_to("stellar")) {
        let tv = echo_trace(v, "stellar");
        if let Err(m) = compare_vector(v, &tv, "stellar") {
            panic!("{m}");
        }
        entries.push(tv);
    }
    let t = trace::new_trace("stellar", Value::Null, entries);
    let back = trace::parse(&trace::to_text(&t).unwrap()).unwrap();
    assert_eq!(back, t);
    assert!(compare_traces(&t, &back).is_empty());
}

#[test]
fn extra_event_fails() {
    let v = spec_vector("example.json", "pause_blocks_requests_until_resume");
    let mut tv = echo_trace(&v, "stellar");
    events_mut(&mut tv, 1).push(json!({ "name": "resumed", "fields": { "by": "guardian" } }));
    let m = mismatch(&v, &tv);
    assert_eq!(m.step, Some(1));
    assert_eq!(m.field, "events[1]");
    assert!(m.detail.contains("unexpected extra event resumed"), "{m}");
    assert!(m.to_string().contains("pause_blocks_requests_until_resume"));
}

#[test]
fn missing_event_fails() {
    let v = spec_vector("example.json", "pause_blocks_requests_until_resume");
    let mut tv = echo_trace(&v, "stellar");
    events_mut(&mut tv, 6).clear();
    let m = mismatch(&v, &tv);
    assert_eq!((m.step, m.field.as_str()), (Some(6), "events[0]"));
    assert!(
        m.detail.contains("payout_requested") && m.detail.contains("missing"),
        "{m}"
    );

    // A refused call that emits anything fails too.
    let mut tv = echo_trace(&v, "stellar");
    events_mut(&mut tv, 3).push(json!({ "name": "paused", "fields": { "by": "guardian" } }));
    let m = mismatch(&v, &tv);
    assert_eq!((m.step, m.field.as_str()), (Some(3), "events"));
}

#[test]
fn reordered_events_fail() {
    let v = spec_vector(
        "unexpected-asset.json",
        "create_accepts_the_supported_asset",
    );
    let mut tv = echo_trace(&v, "stellar");
    events_mut(&mut tv, 1).swap(0, 1);
    let m = mismatch(&v, &tv);
    assert_eq!((m.step, m.field.as_str()), (Some(1), "events[0].name"));
    assert!(m.detail.contains("out of order"), "{m}");
}

#[test]
fn wrong_event_field_names_the_field() {
    let v = spec_vector("example.json", "pause_blocks_requests_until_resume");
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][0]["events"][0]["fields"]["by"] = json!("principal");
    let m = mismatch(&v, &tv);
    assert_eq!(m.field, "events[0].fields.by");
    assert!(m.detail.contains("guardian") && m.detail.contains("principal"));
}

#[test]
fn state_is_matched_only_on_what_the_vector_names() {
    let v = spec_vector("example.json", "pause_blocks_requests_until_resume");
    let mut tv = echo_trace(&v, "stellar");
    // Extra state the vector does not name is fine.
    tv["steps"][5]["state"]["balances"]["asset"]["bob"] = json!("0");
    compare_vector(&v, &tv, "stellar").unwrap();
    tv["steps"][5]["state"]["mandate"]["free"] = json!("901");
    let m = mismatch(&v, &tv);
    assert_eq!((m.step, m.field.as_str()), (Some(6), "state.mandate.free"));
}

#[test]
fn token_events_are_compared_when_the_vector_asks() {
    // The revoke expects token_events: [], so a token moving fails it.
    let v = spec_vector("revoke.json", "revoke_with_principal_unable_to_receive");
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][1]["token_events"] = json!([{ "token": "asset", "name": "transfer" }]);
    let m = mismatch(&v, &tv);
    assert_eq!((m.step, m.field.as_str()), (Some(2), "token_events"));
    // Without token_events in the expectation, token events are not compared.
    let v = spec_vector("revoke.json", "revoke_with_principal_unable_to_receive");
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][6]["token_events"] = json!([{ "token": "asset", "name": "transfer" }]);
    compare_vector(&v, &tv, "stellar").unwrap();
}

#[test]
fn invariant_violation_fails() {
    let v = spec_vector("example.json", "pause_blocks_requests_until_resume");

    // I1: the reservation is above the budget the mandate reports.
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][5]["state"]["mandate"]["limits"] = json!({ "total_budget": "50" });
    let m = mismatch(&v, &tv);
    assert_eq!((m.step, m.field.as_str()), (Some(6), "invariant"));
    assert!(m.detail.starts_with("I1:"), "{m}");

    // I9: a refused call changed the state.
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][2]["state"]["mandate"]["paused"] = json!(false);
    let m = mismatch(&v, &tv);
    assert_eq!(m.step, Some(3));
    assert!(m.detail.starts_with("I9:"), "{m}");

    // I13: a recovery threshold with no recovery set, and one that fits.
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][0]["state"]["mandate"]["recovery"] = json!([]);
    tv["steps"][0]["state"]["mandate"]["recovery_threshold"] = json!(1);
    let m = mismatch(&v, &tv);
    assert_eq!(m.step, Some(1));
    assert!(m.detail.starts_with("I13:"), "{m}");
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][0]["state"]["mandate"]["recovery"] = json!(["r1", "r2"]);
    tv["steps"][0]["state"]["mandate"]["recovery_threshold"] = json!(2);
    compare_vector(&v, &tv, "stellar").unwrap();

    // I10: committed does not match the open payouts.
    let mut tv = echo_trace(&v, "stellar");
    tv["steps"][5]["state"]["mandate"]["committed"] = json!("99");
    tv["steps"][5]["state"]["mandate"]["revoked"] = json!(false);
    let mut v2 = v.clone();
    v2.data["steps"][5]["expect"]["state"]["mandate"]["committed"] = json!("99");
    let m = mismatch(&v2, &tv);
    assert!(m.detail.starts_with("I10:"), "{m}");
}

#[test]
fn per_chain_overrides_replace_the_expected_result() {
    let v = spec_vector("revoke.json", "revoke_return_after_principal_transfer");
    let tv = echo_trace(&v, "stellar");
    compare_vector(&v, &tv, "stellar").unwrap();
    // Read for another chain, the same trace shows a host failure where an
    // error code was expected.
    let m = compare_vector(&v, &tv, "celo").unwrap_err();
    assert_eq!(m.field, "result");
}

#[test]
fn two_traces_are_compared_step_by_step() {
    let v = spec_vector("example.json", "pause_blocks_requests_until_resume");
    let a = trace::new_trace("stellar", Value::Null, vec![echo_trace(&v, "stellar")]);
    let mut b = a.clone();
    b["vectors"][0]["steps"][4]["result"] = json!({ "error": "NOT_PAUSED" });
    let diffs = compare_traces(&a, &b);
    assert_eq!(diffs.len(), 1);
    assert_eq!(diffs[0].step, Some(5));
    assert!(diffs[0].field.starts_with("result"));
}

#[test]
fn only_calls_selects_matching_vectors() {
    let calls =
        parse_only_calls("create,version,config,state,payout,daily_used,is_allowed").unwrap();
    let opts = Options {
        selection: Selection {
            areas: None,
            only_calls: Some(calls.clone()),
        },
        ..reference_options(&[])
    };
    let opts = Options {
        files: None,
        ..opts
    };
    let picked = select(&opts).unwrap();
    let ids: BTreeSet<&str> = picked.iter().map(|v| v.id.as_str()).collect();
    assert!(
        ids.contains("create_accepts_the_supported_asset"),
        "{ids:?}"
    );
    assert!(!ids.contains("pause_blocks_requests_until_resume"));
    for v in &picked {
        assert!(
            v.functions().is_subset(&calls),
            "{} needs {:?}",
            v.id,
            v.functions()
        );
    }
    // A vector set up with an allowlist needs allow_recipients even if no
    // step calls it.
    let with_allowlist = spec_vector("withdrawal.json", "withdraw_negative_amount_refused");
    assert!(with_allowlist.functions().contains("allow_recipients"));
    assert!(!ids.contains("withdraw_negative_amount_refused"));
    // --areas narrows by the rules a vector cites.
    let pause_only = Options {
        selection: Selection {
            areas: Some(vectors::parse_areas("PAUSE").unwrap()),
            only_calls: None,
        },
        files: None,
        ..reference_options(&[])
    };
    for v in select(&pause_only).unwrap() {
        assert!(
            v.rules().iter().any(|r| r.starts_with("MANDATE-PAUSE-")),
            "{}",
            v.id
        );
    }
}

#[test]
fn reference_contract_passes_its_vectors() {
    let s = run(&reference_options(&["selftest/reference.json"])).unwrap();
    let failed: Vec<String> = s.failed.iter().map(ToString::to_string).collect();
    assert!(
        failed.is_empty(),
        "{failed:#?}\n{}",
        trace::to_text(&s.trace).unwrap()
    );
    assert_eq!(s.passed.len(), 2);
    assert!(s.not_runnable.is_empty());
}

#[test]
fn reference_contract_wrong_return_is_reported() {
    let s = run(&reference_options(&["selftest/wrong.json"])).unwrap();
    assert!(s.passed.is_empty());
    let m = s
        .failed
        .iter()
        .find(|m| m.vector == "reference_wrong_version")
        .expect("the wrong version passed");
    assert_eq!((m.step, m.field.as_str()), (Some(2), "return"));
    let text = m.to_string();
    assert!(
        text.contains("reference_wrong_version") && text.contains("step 2"),
        "{text}"
    );
    assert!(text.contains("expected 1, got 2"), "{text}");
}

#[test]
fn token_error_is_never_read_as_a_mandate_error() {
    // The Stellar Asset Contract's code 11 is also ROLE_CONFLICT. A token
    // failure that escapes the mandate must be reported as the token's.
    let s = run(&reference_options(&["selftest/wrong.json"])).unwrap();
    let m = s
        .failed
        .iter()
        .find(|m| m.vector == "reference_token_error_escapes")
        .expect("the escaped token error passed");
    assert_eq!((m.step, m.field.as_str()), (Some(2), "result"));
    assert!(m.detail.contains("escaped_error"), "{m}");
    assert!(m.detail.contains("\"from\":\"asset\""), "{m}");
    assert!(!m.detail.contains("ROLE_CONFLICT"), "{m}");
}

#[test]
fn missing_wasm_is_not_runnable_not_a_pass() {
    let mut opts = reference_options(&["selftest/reference.json"]);
    opts.mandate_wasm = here("fixtures/no_such_contract.wasm");
    let s = run(&opts).unwrap();
    assert!(s.passed.is_empty() && s.failed.is_empty());
    assert_eq!(s.not_runnable.len(), 2);
    for tv in trace::vectors(&s.trace) {
        assert_eq!(tv["status"], "not_runnable");
    }
}

#[test]
fn runner_registers_wasm_only() {
    let spec = Spec::load(&root()).unwrap();
    let opts = reference_options(&[]);
    let contracts = Contracts::load(&opts.factory_wasm, &opts.mandate_wasm).unwrap();
    let all = vectors::load(&[here("selftest/reference.json")]).unwrap();
    let v = all
        .iter()
        .find(|v| v.id == "reference_attests_and_moves_tokens")
        .unwrap();
    let mut w = World::new(&spec, &contracts, v).unwrap();
    w.setup(v).unwrap();
    // The factory and the mandate it deployed run exactly the wasm the run
    // was given; a native registration has no wasm hash at all.
    let factory = executable_hash(w.env(), w.factory()).unwrap();
    let mandate = executable_hash(w.env(), w.mandate().unwrap()).unwrap();
    assert_eq!(factory, Some(sha256(&contracts.factory)));
    assert_eq!(mandate, Some(sha256(&contracts.mandate)));
}

#[test]
fn token_kinds_and_receiving_on_the_test_host() {
    let spec = Spec::load(&root()).unwrap();
    let opts = reference_options(&[]);
    let contracts = Contracts::load(&opts.factory_wasm, &opts.mandate_wasm).unwrap();
    let base = vectors::load(&[here("selftest/reference.json")]).unwrap();
    let base = base
        .iter()
        .find(|v| v.id == "reference_attests_and_moves_tokens")
        .unwrap();

    for kind in ["standard", "returns_value", "moves_settlement"] {
        let mut v = base.clone();
        v.data["setup"]["tokens"] = json!({ "other_token": { "kind": kind } });
        v.data["setup"]["mints"] = json!([
            { "token": "asset", "to": "bob", "amount": "100" },
            { "token": "other_token", "to": "bob", "amount": "100" }
        ]);
        let mut w = World::new(&spec, &contracts, &v).unwrap();
        w.setup(&v).unwrap();
        let other = w.address_of("other_token").unwrap();
        let bob = w.address_of("bob").unwrap();
        let carol = w.address_of("carol").unwrap();
        w.env().mock_all_auths();
        let got = stellar::call(
            w.env(),
            &other,
            "transfer",
            &[
                soroban_sdk::xdr::ScVal::Address(bob.clone()),
                soroban_sdk::xdr::ScVal::Address(carol.clone()),
                stellar::scval::i128_val(10),
            ],
        )
        .unwrap();
        match kind {
            "standard" => assert_eq!(got, stellar::Outcome::Ok(soroban_sdk::xdr::ScVal::Void)),
            "returns_value" => {
                assert_eq!(
                    got,
                    stellar::Outcome::Ok(soroban_sdk::xdr::ScVal::Bool(true))
                )
            }
            // Mocked auth lets the nested settlement transfer through here;
            // what matters is that it moved the settlement asset too.
            _ => assert_eq!(got, stellar::Outcome::Ok(soroban_sdk::xdr::ScVal::Void)),
        }
        let (state, errors) = w.capture_state();
        // The reference mandate has no daily_used view; nothing else may
        // fail to read.
        assert!(
            errors.iter().all(|e| e.starts_with("daily_used()")),
            "{errors:?}"
        );
        assert_eq!(state["balances"]["other_token"]["carol"], "10", "{kind}");
        let carol_asset = if kind == "moves_settlement" {
            "10"
        } else {
            "0"
        };
        assert_eq!(state["balances"]["asset"]["carol"], carol_asset, "{kind}");
    }

    // set_receiving and clawback act on the settlement asset.
    let mut v = base.clone();
    v.data["setup"]["mints"] = json!([{ "token": "asset", "to": "bob", "amount": "100" }]);
    let mut w = World::new(&spec, &contracts, &v).unwrap();
    w.setup(&v).unwrap();
    w.set_receiving("carol", "deauthorized").unwrap();
    assert!(w.transfer("asset", "bob", "carol", 5).is_err());
    w.set_receiving("carol", "no_trustline").unwrap();
    assert!(w.transfer("asset", "bob", "carol", 5).is_err());
    let (state, _) = w.capture_state();
    assert_eq!(state["balances"]["asset"]["carol"], "0");
    w.set_receiving("carol", "receivable").unwrap();
    w.transfer("asset", "bob", "carol", 5).unwrap();
    // A trustline is only removed at a zero balance, as on a network.
    assert!(w.set_receiving("carol", "no_trustline").is_err());
    w.clawback("bob", 30).unwrap();
    let (state, _) = w.capture_state();
    assert_eq!(state["balances"]["asset"]["bob"], "65");
    assert_eq!(state["balances"]["asset"]["carol"], "5");
}

/// The chain-specific acceptance tests need the real factory and mandate,
/// built first by `scripts/soroban-build.sh`. A case whose mandate calls do
/// not exist yet is ignored, never passed: run those with
/// `cargo test -p vector-runner chain_specific -- --ignored`.
fn chain_specific(id: &str) {
    let opts = Options {
        chain: String::from("stellar"),
        root: root(),
        files: Some(vec![here("chain-specific/stellar.json")]),
        selection: Selection::default(),
        factory_wasm: Options::default_wasm(&root(), "ujira_factory"),
        mandate_wasm: Options::default_wasm(&root(), "ujira_mandate"),
    };
    let s = run(&opts).unwrap();
    assert!(s.not_runnable.is_empty(), "{:?}", s.not_runnable_reason);
    if let Some(m) = s.failed.iter().find(|m| m.vector == id) {
        panic!("{m}");
    }
    assert!(s.passed.iter().any(|p| p == id), "{id} did not run");
}

#[test]
#[ignore = "needs the mandate and factory wasm, which #31 to #34 build"]
fn chain_specific_revoke_without_trustline() {
    chain_specific("chain_revoke_without_trustline_moves_nothing");
}

#[test]
#[ignore = "needs the mandate and factory wasm, which #31 to #34 build"]
fn chain_specific_two_leg_execute_second_leg_fails() {
    chain_specific("chain_two_leg_execute_second_leg_fails_leaves_first_unpaid");
}

#[test]
fn chain_specific_create_invalid_config_returns_contract_code() {
    chain_specific("chain_create_invalid_config_returns_the_contract_code");
}

#[test]
#[ignore = "needs the mandate and factory wasm, which #31 to #34 build"]
fn chain_specific_archived_payout_record_exists() {
    chain_specific("chain_archived_payout_record_returns_payout_id_exists");
}
