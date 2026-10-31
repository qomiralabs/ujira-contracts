// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Compares a trace with the vector it ran, or two traces with each other.
//!
//! Against a vector: every call's result must be the expected one, its
//! events must be exactly the expected list in order, `token_events: []`
//! means no token emitted anything, and `state` is matched only on the keys
//! it names. The invariants run after every step. The first difference ends
//! the comparison and names the vector, the step and the field.

use crate::canon::{canon, canon_key};
use crate::invariants::Checker;
use crate::trace::status;
use crate::vectors::Vector;
use serde_json::{Map, Value};
use std::fmt;

/// The first difference found, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub vector: String,
    /// 1-based, as check-vectors.sh counts steps. None for the vector as a
    /// whole.
    pub step: Option<usize>,
    /// The function of a call step, or the step kind.
    pub what: String,
    /// A path such as `events[1].fields.by` or `state.mandate.free`.
    pub field: String,
    pub detail: String,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.step {
            Some(n) => write!(
                f,
                "vector {}, step {} ({}): {}: {}",
                self.vector, n, self.what, self.field, self.detail
            ),
            None => write!(f, "vector {}: {}: {}", self.vector, self.field, self.detail),
        }
    }
}

type Diff = std::result::Result<(), (String, String)>;

fn diff(field: impl Into<String>, detail: impl Into<String>) -> Diff {
    Err((field.into(), detail.into()))
}

fn show(v: &Value) -> String {
    v.to_string()
}

fn canon_or_raw(v: &Value) -> Value {
    canon(v).unwrap_or_else(|_| v.clone())
}

/// Compares one vector's trace with the vector, on `chain`.
pub fn compare_vector(v: &Vector, tv: &Value, chain: &str) -> Result<(), Mismatch> {
    let whole = |field: &str, detail: String| Mismatch {
        vector: v.id.clone(),
        step: None,
        what: String::from("vector"),
        field: field.to_string(),
        detail,
    };
    if tv["id"] != v.id.as_str() {
        return Err(whole("id", format!("the trace is for {}", show(&tv["id"]))));
    }
    let expected = v.steps();
    let actual: &[Value] = tv["steps"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let run_status = tv["status"].as_str().unwrap_or("");
    if run_status != status::RAN && run_status != status::ABORTED {
        return Err(whole(
            "status",
            format!(
                "the vector was {}: {}",
                run_status,
                tv["reason"].as_str().unwrap_or("no reason given")
            ),
        ));
    }
    let mut checker = Checker::new(&tv["setup_state"]).map_err(|e| whole("setup_state", e))?;

    for (i, exp) in expected.iter().enumerate() {
        let n = i + 1;
        let what = exp["fn"]
            .as_str()
            .or_else(|| exp["step"].as_str())
            .unwrap_or("?")
            .to_string();
        let at = |field: String, detail: String| Mismatch {
            vector: v.id.clone(),
            step: Some(n),
            what: what.clone(),
            field,
            detail,
        };
        let Some(act) = actual.get(i) else {
            let detail = if run_status == status::ABORTED {
                format!(
                    "the run stopped before this step: {}",
                    tv["reason"].as_str().unwrap_or("no reason given")
                )
            } else {
                String::from("the trace has no record of this step")
            };
            return Err(at(String::from("step"), detail));
        };
        compare_step(exp, act, n, chain).map_err(|(f, d)| at(f, d))?;
        checker
            .after_step(act)
            .map_err(|d| at(String::from("invariant"), d))?;
    }
    if actual.len() > expected.len() {
        return Err(whole(
            "steps",
            format!(
                "the trace has {} steps, the vector {}",
                actual.len(),
                expected.len()
            ),
        ));
    }
    if run_status == status::ABORTED {
        return Err(whole(
            "status",
            format!(
                "the run was aborted: {}",
                tv["reason"].as_str().unwrap_or("no reason given")
            ),
        ));
    }
    Ok(())
}

fn compare_step(exp: &Value, act: &Value, n: usize, chain: &str) -> Diff {
    if act["step"] != exp["step"] {
        return diff(
            "step",
            format!(
                "expected a {} step, got {}",
                show(&exp["step"]),
                show(&act["step"])
            ),
        );
    }
    if act["index"].as_u64() != u64::try_from(n).ok() {
        return diff(
            "index",
            format!("expected {n}, got {}", show(&act["index"])),
        );
    }
    match exp["step"].as_str() {
        Some("call") => compare_call(exp, act, chain),
        Some("transfer") if exp.get("events").is_some() => {
            no_events(act, "events")?;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn no_events(act: &Value, key: &str) -> Diff {
    match act[key].as_array() {
        Some(list) if list.is_empty() => Ok(()),
        Some(list) => diff(
            key,
            format!("expected none, got {}", show(&Value::Array(list.clone()))),
        ),
        None => diff(
            key,
            String::from("expected none, but the trace has no list"),
        ),
    }
}

fn compare_call(exp: &Value, act: &Value, chain: &str) -> Diff {
    for key in ["fn", "caller"] {
        if act[key] != exp[key] {
            return diff(
                key,
                format!("expected {}, got {}", show(&exp[key]), show(&act[key])),
            );
        }
    }
    if let Some(first) = act["decode_errors"].as_array().and_then(|e| e.first()) {
        return diff(
            "decode",
            format!(
                "the contract's answer does not match the interface: {}",
                first.as_str().unwrap_or("?")
            ),
        );
    }
    let e = &exp["expect"];
    let result = &act["result"];
    let refusal = match e["per_chain"][chain].as_object() {
        Some(over) => {
            let mut want = Map::new();
            for (k, val) in over {
                want.insert(k.clone(), val.clone());
            }
            Some(Value::Object(want))
        }
        None => e["error"]
            .as_str()
            .map(|name| serde_json::json!({ "error": name })),
    };
    if let Some(want) = refusal {
        if *result != want {
            return diff(
                "result",
                format!("expected {}, got {}", show(&want), show(result)),
            );
        }
        no_events(act, "events")?;
        return Ok(());
    }

    if result["ok"] != true {
        return diff(
            "result",
            format!("expected the call to succeed, got {}", show(result)),
        );
    }
    if let Some(want) = e.get("return") {
        let got = result.get("return").unwrap_or(&Value::Null);
        if canon_or_raw(want) != canon_or_raw(got) {
            return diff(
                "return",
                format!("expected {}, got {}", show(want), show(got)),
            );
        }
    }
    compare_events(&e["events"], &act["events"])?;
    if e.get("token_events").is_some() {
        no_events(act, "token_events")?;
    }
    if let Some(want) = e.get("state") {
        compare_state(want, &act["state"])?;
    }
    Ok(())
}

fn event_name(e: &Value) -> String {
    e["name"].as_str().unwrap_or("?").to_string()
}

/// Exhaustive and ordered: a missing, extra or moved event is a difference.
pub fn compare_events(expected: &Value, actual: &Value) -> Diff {
    let exp: &[Value] = expected.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let Some(act) = actual.as_array() else {
        return diff("events", String::from("the trace has no event list"));
    };
    let count = exp.len().max(act.len());
    for i in 0..count {
        let field = format!("events[{i}]");
        match (exp.get(i), act.get(i)) {
            (Some(e), None) => {
                return diff(
                    field,
                    format!(
                        "expected event {}, got no event: the event is missing",
                        event_name(e)
                    ),
                )
            }
            (None, Some(a)) => {
                return diff(field, format!("unexpected extra event {}", event_name(a)))
            }
            (Some(e), Some(a)) => {
                let (en, an) = (event_name(e), event_name(a));
                if en != an {
                    let moved = exp.iter().any(|x| event_name(x) == an)
                        && act.iter().any(|x| event_name(x) == en);
                    let note = if moved {
                        ": the events are out of order"
                    } else {
                        ""
                    };
                    return diff(
                        format!("{field}.name"),
                        format!("expected {en}, got {an}{note}"),
                    );
                }
                compare_exact(
                    &canon_or_raw(&e["fields"]),
                    &canon_or_raw(&a["fields"]),
                    &format!("{field}.fields"),
                )?;
            }
            (None, None) => {}
        }
    }
    Ok(())
}

/// Every key on both sides, and equal values.
fn compare_exact(want: &Value, got: &Value, path: &str) -> Diff {
    match (want.as_object(), got.as_object()) {
        (Some(w), Some(g)) => {
            for (k, wv) in w {
                match g.get(k) {
                    None => return diff(format!("{path}.{k}"), "missing"),
                    Some(gv) if gv != wv => {
                        return diff(
                            format!("{path}.{k}"),
                            format!("expected {}, got {}", show(wv), show(gv)),
                        )
                    }
                    Some(_) => {}
                }
            }
            if let Some(extra) = g.keys().find(|k| !w.contains_key(*k)) {
                return diff(format!("{path}.{extra}"), "not expected");
            }
            Ok(())
        }
        _ if want == got => Ok(()),
        _ => diff(path, format!("expected {}, got {}", show(want), show(got))),
    }
}

/// Only what `want` names is compared.
fn compare_partial(want: &Value, got: &Value, path: &str) -> Diff {
    match want.as_object() {
        Some(w) => {
            let Some(g) = got.as_object() else {
                return diff(path, format!("expected {}, got {}", show(want), show(got)));
            };
            for (k, wv) in w {
                match g.get(k) {
                    None => {
                        return diff(
                            format!("{path}.{k}"),
                            "not recorded in the trace: the contract did not report it",
                        )
                    }
                    Some(gv) if gv != wv => {
                        return diff(
                            format!("{path}.{k}"),
                            format!("expected {}, got {}", show(wv), show(gv)),
                        )
                    }
                    Some(_) => {}
                }
            }
            Ok(())
        }
        None if want == got => Ok(()),
        None => diff(path, format!("expected {}, got {}", show(want), show(got))),
    }
}

pub fn compare_state(want: &Value, got: &Value) -> Diff {
    if got.is_null() {
        return diff(
            "state",
            "expected state, but the trace has none: there is no mandate to read",
        );
    }
    let want = canon_or_raw(want);
    let got = canon_or_raw(got);
    if let Some(m) = want.get("mandate") {
        compare_partial(m, &got["mandate"], "state.mandate")?;
    }
    if let Some(p) = want["payouts"].as_object() {
        for (id, wv) in p {
            let key = canon_key(id).unwrap_or_else(|_| id.clone());
            let path = format!("state.payouts.{id}");
            let Some(gv) = got["payouts"].get(&key) else {
                return diff(path, "not recorded in the trace");
            };
            if wv.is_null() || gv.is_null() {
                if wv != gv {
                    return diff(path, format!("expected {}, got {}", show(wv), show(gv)));
                }
                continue;
            }
            compare_partial(wv, gv, &path)?;
        }
    }
    if let Some(b) = want["balances"].as_object() {
        for (token, actors) in b {
            compare_partial(
                actors,
                &got["balances"][token],
                &format!("state.balances.{token}"),
            )?;
        }
    }
    if let Some(d) = want["daily"].as_object() {
        for (rr, wv) in d {
            let key = canon_key(rr).unwrap_or_else(|_| rr.clone());
            let path = format!("state.daily.{rr}");
            let Some(gv) = got["daily"].get(&key) else {
                return diff(path, "not recorded in the trace");
            };
            compare_exact(wv, gv, &path)?;
        }
    }
    Ok(())
}

/// The trace a chain that did exactly what `v` expects would write: every
/// result and event list as expected. The state is only what the vector
/// states: after a successful call, what it expects then; after a refused
/// call, the same as before, since a refusal changes nothing; and unknown
/// (null) after any other step. The invariants skip what the state does not
/// say. The comparator's self tests start from it and change one thing at a
/// time.
pub fn echo_trace(v: &Vector, chain: &str) -> Value {
    let mut state = Value::Null;
    let mut steps = Vec::new();
    for (i, s) in v.steps().iter().enumerate() {
        let mut rec = s.as_object().cloned().unwrap_or_default();
        rec.remove("expect");
        rec.insert(String::from("index"), Value::from(i + 1));
        rec.insert(String::from("now"), Value::String(String::from("0")));
        let e = &s["expect"];
        if s["step"] == "call" {
            let result = if let Some(over) = e["per_chain"].get(chain) {
                over.clone()
            } else if let Some(name) = e["error"].as_str() {
                serde_json::json!({ "error": name })
            } else {
                serde_json::json!({
                    "ok": true,
                    "return": e.get("return").cloned().unwrap_or(Value::Null),
                })
            };
            rec.insert(String::from("result"), result);
            rec.insert(
                String::from("events"),
                e.get("events").cloned().unwrap_or(Value::Array(Vec::new())),
            );
            rec.insert(String::from("token_events"), Value::Array(Vec::new()));
            let refused = e.get("error").is_some() || e["per_chain"].get(chain).is_some();
            if !refused {
                state = e.get("state").cloned().unwrap_or(Value::Null);
            }
        } else {
            state = Value::Null;
            if s["step"] == "transfer" {
                rec.insert(String::from("events"), Value::Array(Vec::new()));
                rec.insert(String::from("token_events"), Value::Array(Vec::new()));
            }
        }
        rec.insert(String::from("state"), state.clone());
        steps.push(Value::Object(rec));
    }
    serde_json::json!({
        "id": v.id,
        "file": v.file,
        "status": status::RAN,
        "actors": {},
        "setup_state": Value::Null,
        "steps": steps,
    })
}

/// The first path where two values differ, with both sides.
pub fn first_difference(a: &Value, b: &Value, path: &str) -> Option<(String, String, String)> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, xv) in x {
                let sub = format!("{path}.{k}");
                match y.get(k) {
                    None => return Some((sub, show(xv), String::from("nothing"))),
                    Some(yv) => {
                        if let Some(d) = first_difference(xv, yv, &sub) {
                            return Some(d);
                        }
                    }
                }
            }
            y.iter()
                .find(|(k, _)| !x.contains_key(*k))
                .map(|(k, yv)| (format!("{path}.{k}"), String::from("nothing"), show(yv)))
        }
        (Value::Array(x), Value::Array(y)) => {
            for i in 0..x.len().max(y.len()) {
                let sub = format!("{path}[{i}]");
                match (x.get(i), y.get(i)) {
                    (Some(xv), Some(yv)) => {
                        if let Some(d) = first_difference(xv, yv, &sub) {
                            return Some(d);
                        }
                    }
                    (Some(xv), None) => return Some((sub, show(xv), String::from("nothing"))),
                    (None, Some(yv)) => return Some((sub, String::from("nothing"), show(yv))),
                    (None, None) => {}
                }
            }
            None
        }
        _ if a == b => None,
        _ => Some((path.to_string(), show(a), show(b))),
    }
}

/// Compares two traces of the same vectors step by step: results, events,
/// whether tokens moved, and the state after each step. Addresses and token
/// event shapes are chain specific and are left out.
pub fn compare_traces(a: &Value, b: &Value) -> Vec<Mismatch> {
    let mut out = Vec::new();
    let empty = Vec::new();
    let av = a["vectors"].as_array().unwrap_or(&empty);
    let bv = b["vectors"].as_array().unwrap_or(&empty);
    for x in av {
        let id = x["id"].as_str().unwrap_or("?").to_string();
        let whole = |field: &str, detail: String| Mismatch {
            vector: id.clone(),
            step: None,
            what: String::from("vector"),
            field: field.to_string(),
            detail,
        };
        let Some(y) = bv.iter().find(|y| y["id"] == x["id"]) else {
            out.push(whole("id", String::from("only in the first trace")));
            continue;
        };
        if x["status"] != y["status"] {
            out.push(whole(
                "status",
                format!("{} and {}", show(&x["status"]), show(&y["status"])),
            ));
            continue;
        }
        if let Some(m) = compare_trace_steps(&id, x, y) {
            out.push(m);
        }
    }
    for y in bv {
        if !av.iter().any(|x| x["id"] == y["id"]) {
            out.push(Mismatch {
                vector: y["id"].as_str().unwrap_or("?").to_string(),
                step: None,
                what: String::from("vector"),
                field: String::from("id"),
                detail: String::from("only in the second trace"),
            });
        }
    }
    out
}

fn compare_trace_steps(id: &str, x: &Value, y: &Value) -> Option<Mismatch> {
    let empty = Vec::new();
    let xs = x["steps"].as_array().unwrap_or(&empty);
    let ys = y["steps"].as_array().unwrap_or(&empty);
    let mismatch = |n: usize, what: &str, field: String, detail: String| Mismatch {
        vector: id.to_string(),
        step: Some(n),
        what: what.to_string(),
        field,
        detail,
    };
    for i in 0..xs.len().max(ys.len()) {
        let n = i + 1;
        let (Some(s), Some(t)) = (xs.get(i), ys.get(i)) else {
            return Some(mismatch(
                n,
                "step",
                String::from("step"),
                String::from("only one trace has this step"),
            ));
        };
        let what = s["fn"]
            .as_str()
            .or_else(|| s["step"].as_str())
            .unwrap_or("?")
            .to_string();
        for key in ["step", "fn", "result", "events", "state"] {
            if let Some((path, l, r)) =
                first_difference(&canon_or_raw(&s[key]), &canon_or_raw(&t[key]), key)
            {
                return Some(mismatch(n, &what, path, format!("{l} and {r}")));
            }
        }
        let moved = |v: &Value| v["token_events"].as_array().is_some_and(|l| !l.is_empty());
        if moved(s) != moved(t) {
            return Some(mismatch(
                n,
                &what,
                String::from("token_events"),
                format!(
                    "tokens moved in {} trace only",
                    if moved(s) { "the first" } else { "the second" }
                ),
            ));
        }
    }
    None
}
