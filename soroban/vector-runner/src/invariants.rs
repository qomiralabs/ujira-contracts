// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The spec's invariants I1 to I10 and I13, checked after every step of a
//! trace.
//!
//! Each check reads only the trace: the state before and after the step, the
//! step's result and its events. A check whose inputs the trace does not
//! have, for example because the mandate does not exist yet, is skipped for
//! that step; a missing field the vector expects is the comparator's to
//! report.

use crate::canon::canon;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const TERMINAL: [&str; 3] = ["EXECUTED", "REJECTED", "CANCELLED"];

type Check = std::result::Result<(), String>;

fn fail(invariant: &str, detail: String) -> Check {
    Err(format!("{invariant}: {detail}"))
}

fn amount(v: &Value) -> Option<i128> {
    v.as_str().and_then(|s| s.parse::<i128>().ok())
}

fn names(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn charged(v: &Value) -> Option<i128> {
    amount(&v["total"])?.checked_add(amount(&v["fee"])?)
}

/// Carries what the invariants need from one step to the next.
pub struct Checker {
    prev: Value,
    executed: BTreeSet<String>,
    needs_approval: BTreeSet<String>,
    approvals: BTreeMap<String, BTreeSet<String>>,
    /// How many times each approver has been removed. An approver added back
    /// may approve the same payout again, and that is not approving twice.
    removals: BTreeMap<String, u32>,
    executed_per_day: BTreeMap<(String, String), i128>,
    /// Set once the asset itself has taken the mandate's balance, after
    /// which I2 no longer has to hold.
    asset_intervened: bool,
    /// Every payout ID a `payout_requested` event has named.
    requested: BTreeSet<String>,
    /// Open payouts whose agent has since been removed, whose reservations
    /// that removal released.
    released: BTreeSet<String>,
}

impl Checker {
    pub fn new(setup_state: &Value) -> std::result::Result<Checker, String> {
        Ok(Checker {
            prev: canon(setup_state)?,
            executed: BTreeSet::new(),
            needs_approval: BTreeSet::new(),
            approvals: BTreeMap::new(),
            removals: BTreeMap::new(),
            executed_per_day: BTreeMap::new(),
            asset_intervened: false,
            requested: BTreeSet::new(),
            released: BTreeSet::new(),
        })
    }

    /// Checks the invariants after one recorded step and moves on.
    pub fn after_step(&mut self, step: &Value) -> Check {
        let cur = canon(&step["state"])?;
        let events = canon(&step["events"])?;
        let events: &[Value] = events.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let kind = step["step"].as_str().unwrap_or("");
        let is_call = kind == "call";
        let ok = step["result"]["ok"] == true;
        let prev = std::mem::take(&mut self.prev);

        if is_call && !ok {
            self.failed_call_changes_nothing(step, events, &prev, &cur)?;
        }
        for e in events.iter().filter(|e| e["name"] == "payout_requested") {
            if let Some(id) = e["fields"]["payout_id"].as_str() {
                self.requested.insert(id.to_string());
            }
        }
        for e in events.iter().filter(|e| e["name"] == "role_removed") {
            if e["fields"]["role"] == 1 {
                self.note_released(&e["fields"]["account"], &prev["payouts"]);
            }
        }
        if kind == "clawback" && step["from"] == "mandate" {
            self.asset_intervened = true;
        }
        if !cur.is_null() {
            let m = &cur["mandate"];
            i1(m)?;
            i13(m)?;
            i7(&prev["mandate"], m)?;
            if !self.asset_intervened {
                i2(m)?;
            }
            // The sums are only known when every payout requested so far
            // has its record in the state.
            let all_recorded = self
                .requested
                .iter()
                .all(|id| cur["payouts"].get(id).is_some());
            if all_recorded {
                i10(m, &cur["payouts"], &self.released)?;
            }
            i3_records(&cur["payouts"])?;
            i6_records(&prev["payouts"], &cur["payouts"])?;
            if !prev.is_null() && is_call && ok {
                self.i5(step, events, &prev, &cur)?;
            }
        }
        if !prev.is_null() {
            self.events(events, &prev)?;
        }
        if !cur.is_null() {
            self.i8_status(&cur["payouts"])?;
        }
        self.prev = cur;
        Ok(())
    }

    /// The open payouts of an agent that was just removed hold nothing now.
    fn note_released(&mut self, agent: &Value, payouts: &Value) {
        for (id, rec) in payouts.as_object().into_iter().flatten() {
            let open = rec["status"] == "PENDING" || rec["status"] == "APPROVED";
            if open && rec["agent"] == *agent {
                self.released.insert(id.clone());
            }
        }
    }

    /// I9: a failed call emits nothing and changes nothing.
    fn failed_call_changes_nothing(
        &self,
        step: &Value,
        events: &[Value],
        prev: &Value,
        cur: &Value,
    ) -> Check {
        if let Some(e) = events.first() {
            return fail("I9", format!("the call failed but emitted {}", e["name"]));
        }
        if step["token_events"]
            .as_array()
            .is_some_and(|l| !l.is_empty())
        {
            return fail(
                "I9",
                String::from("the call failed but a token emitted events"),
            );
        }
        if let Some((path, before, after)) = crate::compare::first_difference(prev, cur, "state") {
            return fail(
                "I9",
                format!("the call failed but changed {path} from {before} to {after}"),
            );
        }
        Ok(())
    }

    /// The checks that read events: I3 and I4 at execution, I6 on reuse and
    /// repeated execution, I8 on approvals.
    fn events(&mut self, events: &[Value], prev: &Value) -> Check {
        let limits = &prev["mandate"]["limits"];
        let cap = amount(&limits["per_payment_cap"]);
        let threshold = amount(&limits["approval_threshold"]);
        let daily_cap = amount(&limits["per_recipient_daily_cap"]);
        for e in events {
            let f = &e["fields"];
            let id = f["payout_id"].as_str().unwrap_or("").to_string();
            match e["name"].as_str().unwrap_or("") {
                "payout_requested" => {
                    if !prev["payouts"][&id].is_null() {
                        return fail("I6", format!("payout {id} was requested a second time"));
                    }
                    let c = charged(f);
                    if let (Some(c), Some(cap)) = (c, cap) {
                        if c > cap {
                            return fail(
                                "I3",
                                format!("payout {id} charges {c} at request, above the cap {cap}"),
                            );
                        }
                    }
                    if let (Some(c), Some(t)) = (c, threshold) {
                        if c >= t {
                            if f["status"] != "PENDING" {
                                return fail(
                                    "I8",
                                    format!(
                                        "payout {id} charges {c}, at or above the approval threshold {t}, but was requested {}",
                                        f["status"]
                                    ),
                                );
                            }
                            self.needs_approval.insert(id);
                        }
                    }
                }
                "payout_approval_added" => {
                    let who = f["approver"].as_str().unwrap_or("").to_string();
                    let m = &prev["mandate"];
                    if m.get("approvers").is_some() && !names(&m["approvers"]).contains(&who) {
                        return fail("I8", format!("{who} approved {id} but is not an approver"));
                    }
                    if names(&m["agents"]).contains(&who) || names(&m["executors"]).contains(&who) {
                        return fail(
                            "I8",
                            format!("{who} approved {id} while an agent or an executor"),
                        );
                    }
                    let removed = self.removals.get(&who).copied().unwrap_or(0);
                    if !self
                        .approvals
                        .entry(id.clone())
                        .or_default()
                        .insert(format!("{who}/{removed}"))
                    {
                        return fail("I8", format!("{who} approved {id} twice"));
                    }
                }
                "payout_approved" => self.has_approvals(&id)?,
                "role_removed" if f["role"] == 2 => {
                    if let Some(who) = f["account"].as_str() {
                        let n = self.removals.entry(who.to_string()).or_insert(0);
                        *n = n.saturating_add(1);
                    }
                }
                "payout_executed" => {
                    if !self.executed.insert(id.clone()) {
                        return fail("I6", format!("payout {id} was executed twice"));
                    }
                    if let (Some(c), Some(cap)) = (charged(f), cap) {
                        if c > cap {
                            return fail(
                                "I3",
                                format!("payout {id} charges {c} at execute, above the cap {cap}"),
                            );
                        }
                    }
                    let key = (
                        f["recipient_ref"].as_str().unwrap_or("").to_string(),
                        f["day"].as_str().unwrap_or("").to_string(),
                    );
                    if let Some(total) = amount(&f["total"]) {
                        let used = self.executed_per_day.entry(key.clone()).or_insert(0);
                        *used = used.saturating_add(total);
                        if let Some(dc) = daily_cap {
                            if *used > dc {
                                return fail(
                                    "I4",
                                    format!(
                                        "recipient {} has {} executed on day {}, above the cap {dc}",
                                        key.0, used, key.1
                                    ),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn has_approvals(&self, id: &str) -> Check {
        if self.needs_approval.contains(id) && self.approvals.get(id).is_none_or(BTreeSet::is_empty)
        {
            return fail(
                "I8",
                format!("payout {id} needed approval but was approved by no approver"),
            );
        }
        Ok(())
    }

    /// I8 on the records: a payout that needed approval only reaches
    /// APPROVED, or EXECUTED, through at least one approval.
    fn i8_status(&self, payouts: &Value) -> Check {
        for id in &self.needs_approval {
            let s = &payouts[id]["status"];
            if s == "APPROVED" || s == "EXECUTED" {
                self.has_approvals(id)?;
            }
        }
        Ok(())
    }

    /// I5: the settlement asset leaves the mandate only through execute, to
    /// the payout's destinations and the fee address, and through withdraw
    /// and sweep, to the principal.
    fn i5(&self, step: &Value, events: &[Value], prev: &Value, cur: &Value) -> Check {
        let before = &prev["balances"]["asset"];
        let after = &cur["balances"]["asset"];
        let (Some(b), Some(a)) = (amount(&before["mandate"]), amount(&after["mandate"])) else {
            return Ok(());
        };
        if a >= b {
            return Ok(());
        }
        let out = b.saturating_sub(a);
        let mut credits: BTreeMap<String, i128> = BTreeMap::new();
        match step["fn"].as_str().unwrap_or("") {
            "execute" | "execute_batch" => {
                for e in events.iter().filter(|e| e["name"] == "payout_executed") {
                    let f = &e["fields"];
                    for leg in f["legs"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                        let to = leg["destination"]["address"].as_str().unwrap_or("");
                        let n = amount(&leg["amount"]).unwrap_or(0);
                        let slot = credits.entry(to.to_string()).or_insert(0);
                        *slot = slot.saturating_add(n);
                    }
                    let fee = amount(&f["fee"]).unwrap_or(0);
                    if fee > 0 {
                        let slot = credits.entry(String::from("fee_address")).or_insert(0);
                        *slot = slot.saturating_add(fee);
                    }
                }
            }
            "withdraw" | "sweep" => {
                // Without the principal there is no one to check the credit
                // against.
                let Some(principal) = prev["mandate"]["principal"].as_str() else {
                    return Ok(());
                };
                credits.insert(principal.to_string(), out);
            }
            other => {
                return fail(
                    "I5",
                    format!("{out} of the settlement asset left the mandate during {other}"),
                )
            }
        }
        let paid = credits
            .values()
            .fold(0i128, |acc, v| acc.saturating_add(*v));
        if paid != out {
            return fail(
                "I5",
                format!("{out} left the mandate but the step accounts for {paid}"),
            );
        }
        if let Some(holders) = after.as_object() {
            for (holder, now) in holders {
                if holder == "mandate" {
                    continue;
                }
                let (Some(x), Some(y)) = (amount(&before[holder]), amount(now)) else {
                    continue;
                };
                let want = credits.get(holder).copied().unwrap_or(0);
                let got = y.saturating_sub(x);
                if got != want {
                    return fail(
                        "I5",
                        format!("{holder} received {got} of the settlement asset, expected {want}"),
                    );
                }
            }
        }
        Ok(())
    }
}

/// I13: the recovery threshold is 0 exactly when the recovery set is empty,
/// and otherwise between 1 and the size of the set.
fn i13(m: &Value) -> Check {
    let (Some(set), Some(t)) = (m["recovery"].as_array(), m["recovery_threshold"].as_u64()) else {
        return Ok(());
    };
    let n = set.len() as u64;
    let fits = if n == 0 { t == 0 } else { (1..=n).contains(&t) };
    if fits {
        Ok(())
    } else {
        fail(
            "I13",
            format!("the recovery threshold is {t} with {n} recovery approvers"),
        )
    }
}

/// I1: spent + pending + committed <= total_budget.
fn i1(m: &Value) -> Check {
    let (Some(s), Some(p), Some(c), Some(budget)) = (
        amount(&m["spent"]),
        amount(&m["pending"]),
        amount(&m["committed"]),
        amount(&m["limits"]["total_budget"]),
    ) else {
        return Ok(());
    };
    let used = s.checked_add(p).and_then(|x| x.checked_add(c));
    match used {
        Some(u) if u <= budget => Ok(()),
        _ => fail(
            "I1",
            format!("spent {s} + pending {p} + committed {c} is above the budget {budget}"),
        ),
    }
}

/// I2: while active, balance >= pending + committed.
fn i2(m: &Value) -> Check {
    if m["revoked"] != false || m["expired"] != false {
        return Ok(());
    }
    let (Some(bal), Some(p), Some(c)) = (
        amount(&m["balance"]),
        amount(&m["pending"]),
        amount(&m["committed"]),
    ) else {
        return Ok(());
    };
    match p.checked_add(c) {
        Some(held) if bal >= held => Ok(()),
        _ => fail(
            "I2",
            format!("the active mandate holds {bal}, below pending {p} + committed {c}"),
        ),
    }
}

/// I7: spent never decreases and revoked never clears.
fn i7(before: &Value, after: &Value) -> Check {
    if let (Some(x), Some(y)) = (amount(&before["spent"]), amount(&after["spent"])) {
        if y < x {
            return fail("I7", format!("spent went down from {x} to {y}"));
        }
    }
    if before["revoked"] == true && after["revoked"] == false {
        return fail("I7", String::from("revoked was cleared"));
    }
    Ok(())
}

/// I10: pending and committed are the charges of the PENDING and APPROVED
/// payouts, leaving out those of an agent removed since, and both are zero
/// once revoked.
fn i10(m: &Value, payouts: &Value, released: &BTreeSet<String>) -> Check {
    let (Some(p), Some(c)) = (amount(&m["pending"]), amount(&m["committed"])) else {
        return Ok(());
    };
    if m["revoked"] == true {
        if p != 0 || c != 0 {
            return fail(
                "I10",
                format!("revoked, yet pending is {p} and committed is {c}"),
            );
        }
        return Ok(());
    }
    // Revoke zeroes both without touching the records, so the sums only
    // hold when the state says the mandate is not revoked.
    if m["revoked"] != false {
        return Ok(());
    }
    let mut want_p: i128 = 0;
    let mut want_c: i128 = 0;
    for (id, rec) in payouts.as_object().into_iter().flatten() {
        if released.contains(id) {
            continue;
        }
        let target = match rec["status"].as_str() {
            Some("PENDING") => &mut want_p,
            Some("APPROVED") => &mut want_c,
            _ => continue,
        };
        let Some(ch) = charged(rec) else {
            // The record does not carry its amounts, so the sum cannot be
            // known; the comparator reports the missing fields if a vector
            // expects them.
            let _ = id;
            return Ok(());
        };
        *target = target.saturating_add(ch);
    }
    if p != want_p || c != want_c {
        return fail(
            "I10",
            format!(
                "pending {p} and committed {c}, but the open payouts charge {want_p} pending and {want_c} approved"
            ),
        );
    }
    Ok(())
}

/// I3 on the records: the legs of every payout sum to its total.
fn i3_records(payouts: &Value) -> Check {
    for (id, rec) in payouts.as_object().into_iter().flatten() {
        let (Some(legs), Some(total)) = (rec["legs"].as_array(), amount(&rec["total"])) else {
            continue;
        };
        let sum = legs
            .iter()
            .try_fold(0i128, |acc, l| acc.checked_add(amount(&l["amount"])?));
        if sum != Some(total) {
            return fail(
                "I3",
                format!("the legs of {id} do not sum to its total {total}"),
            );
        }
    }
    Ok(())
}

/// I6 on the records: a terminal status never changes and a used ID never
/// becomes unused.
fn i6_records(before: &Value, after: &Value) -> Check {
    for (id, old) in before.as_object().into_iter().flatten() {
        if old.is_null() {
            continue;
        }
        // A payout the state does not mention is not known either way; one
        // recorded as null has no record.
        let Some(new) = after.get(id) else {
            continue;
        };
        if new.is_null() {
            return fail("I6", format!("payout {id} had a record and now has none"));
        }
        let was = old["status"].as_str().unwrap_or("");
        if TERMINAL.contains(&was) && new["status"] != was {
            return fail(
                "I6",
                format!("payout {id} was {was} and is now {}", new["status"]),
            );
        }
    }
    Ok(())
}
