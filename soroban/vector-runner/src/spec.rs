// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! What the runner reads from the spec itself rather than repeating: the
//! error table and the event table. Reading them at run time means a renamed
//! error or a new event field reaches the runner without a code change.

use crate::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Payout statuses and their codes on Stellar.
pub const STATUSES: [(u32, &str); 5] = [
    (1, "PENDING"),
    (2, "APPROVED"),
    (3, "EXECUTED"),
    (4, "REJECTED"),
    (5, "CANCELLED"),
];

/// Recipient modes and their codes on Stellar.
pub const RECIPIENT_MODES: [(u32, &str); 3] = [(1, "ALLOWLIST"), (2, "ATTESTED"), (3, "BOTH")];

pub fn status_name(code: u32) -> Option<&'static str> {
    STATUSES.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

pub fn status_code(name: &str) -> Option<u32> {
    STATUSES.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
}

pub fn mode_name(code: u32) -> Option<&'static str> {
    RECIPIENT_MODES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| *n)
}

pub fn mode_code(name: &str) -> Option<u32> {
    RECIPIENT_MODES
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(c, _)| *c)
}

/// One row of the event table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventSpec {
    /// The canonical fields, in the table's order.
    pub fields: Vec<String>,
    /// The field carried as the second topic: `payout_id` or `tenant_ref`.
    pub key: String,
}

#[derive(Clone, Debug)]
pub struct Spec {
    errors_by_code: BTreeMap<u32, String>,
    errors_by_name: BTreeMap<String, u32>,
    events: BTreeMap<String, EventSpec>,
}

impl Spec {
    /// Reads `spec/errors.json` and `spec/EVENTS.md` under the repository root.
    pub fn load(root: &Path) -> Result<Spec> {
        let errors = read(&root.join("spec").join("errors.json"))?;
        let events = read(&root.join("spec").join("EVENTS.md"))?;
        Spec::parse(&errors, &events)
    }

    pub fn parse(errors_json: &str, events_md: &str) -> Result<Spec> {
        let table: Value =
            serde_json::from_str(errors_json).map_err(|e| format!("errors.json: {e}"))?;
        let entries = table
            .as_array()
            .ok_or_else(|| String::from("errors.json is not a list"))?;
        let mut errors_by_code = BTreeMap::new();
        let mut errors_by_name = BTreeMap::new();
        for entry in entries {
            let name = entry["name"]
                .as_str()
                .ok_or_else(|| format!("errors.json entry without a name: {entry}"))?;
            let code = entry["code"]
                .as_u64()
                .and_then(|c| u32::try_from(c).ok())
                .ok_or_else(|| format!("errors.json entry {name} without a u32 code"))?;
            if errors_by_code.insert(code, name.to_string()).is_some() {
                return Err(format!("errors.json repeats code {code}"));
            }
            if errors_by_name.insert(name.to_string(), code).is_some() {
                return Err(format!("errors.json repeats {name}"));
            }
        }

        let mut events = BTreeMap::new();
        for line in events_md.lines() {
            if !line.starts_with("| `") {
                continue;
            }
            let cells: Vec<&str> = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if cells.len() < 3 {
                return Err(format!("EVENTS.md row with too few cells: {line}"));
            }
            let fields: Vec<String> = cells[1]
                .split(',')
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty())
                .collect();
            let key = cells[2].to_string();
            if key != "payout_id" && key != "tenant_ref" {
                return Err(format!("EVENTS.md row with topic key {key:?}: {line}"));
            }
            for name in backticked(cells[0]) {
                events.insert(
                    name,
                    EventSpec {
                        fields: fields.clone(),
                        key: key.clone(),
                    },
                );
            }
        }
        if errors_by_code.is_empty() || events.is_empty() {
            return Err(String::from("the error or the event table is empty"));
        }
        Ok(Spec {
            errors_by_code,
            errors_by_name,
            events,
        })
    }

    pub fn error_name(&self, code: u32) -> Option<&str> {
        self.errors_by_code.get(&code).map(String::as_str)
    }

    pub fn error_code(&self, name: &str) -> Option<u32> {
        self.errors_by_name.get(name).copied()
    }

    pub fn event(&self, name: &str) -> Option<&EventSpec> {
        self.events.get(name)
    }
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn backticked(cell: &str) -> Vec<String> {
    cell.split('`')
        .enumerate()
        .filter(|(i, part)| i % 2 == 1 && crate::values::is_name(part))
        .map(|(_, part)| part.to_string())
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;

    fn repo_spec() -> Spec {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Spec::load(&root).unwrap()
    }

    #[test]
    fn every_error_name_is_a_mandate_error_code() {
        // The runner names errors from the spec table and the contracts
        // return the shared enum, so every code must mean the same to both.
        use mandate_errors::MandateError;
        let spec = repo_spec();
        for (code, name) in &spec.errors_by_code {
            let e = soroban_sdk::Error::from_contract_error(*code);
            let m = MandateError::try_from(e)
                .unwrap_or_else(|_| panic!("{name} ({code}) is not a MandateError"));
            assert_eq!(m as u32, *code);
        }
        assert_eq!(
            spec.error_code("TRANSFER_FAILED"),
            Some(MandateError::TransferFailed as u32)
        );
    }

    #[test]
    fn reads_the_real_tables() {
        let spec = repo_spec();
        assert_eq!(spec.error_name(22), Some("MANDATE_PAUSED"));
        assert_eq!(spec.error_code("AGENT_REMOVED"), Some(65));
        let paused = spec.event("paused").unwrap();
        assert_eq!(paused.fields, ["by"]);
        assert_eq!(paused.key, "tenant_ref");
        // A shared row names two events.
        assert_eq!(spec.event("resumed"), Some(paused));
        let requested = spec.event("payout_requested").unwrap();
        assert_eq!(requested.key, "payout_id");
        assert_eq!(requested.fields[0], "payout_id");
        let created = spec.event("mandate_created").unwrap();
        assert!(created.fields.contains(&String::from("tenant_ref")));
    }
}
