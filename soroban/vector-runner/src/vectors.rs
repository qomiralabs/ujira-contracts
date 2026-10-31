// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Loading vector files and choosing which vectors a run covers.
//!
//! Every `*.json` in the vector directory except the schema is a vector file,
//! so a file added later is picked up without touching the runner.

use crate::Result;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "ujira-mandate-vectors/1";

/// Every function a call step can name, as the schema lists them.
pub const CALL_FNS: [&str; 35] = [
    "create",
    "request_payout",
    "request_payouts",
    "approve",
    "reject",
    "cancel",
    "cancel_batch",
    "execute",
    "execute_batch",
    "pause",
    "resume",
    "revoke",
    "withdraw",
    "sweep",
    "recover_token",
    "update_limits",
    "add_role",
    "remove_role",
    "set_approvals_required",
    "set_recovery_threshold",
    "allow_recipients",
    "disallow_recipients",
    "set_attestation_policy",
    "propose_principal",
    "accept_principal",
    "cancel_principal_proposal",
    "lower_fee_cap",
    "recovery_revoke",
    "recovery_rotate_agent",
    "version",
    "config",
    "state",
    "payout",
    "daily_used",
    "is_allowed",
];

/// Step kinds other than `call`. They need no contract function, so naming
/// one in `--only-calls` is accepted and never excludes a vector.
pub const OTHER_STEPS: [&str; 7] = [
    "advance_time",
    "set_time",
    "transfer",
    "mint",
    "attest",
    "set_receiving",
    "clawback",
];

pub const CHAINS: [&str; 2] = ["stellar", "celo"];

#[derive(Clone, Debug)]
pub struct Vector {
    /// The file it came from, as given on the command line or found in the
    /// vector directory.
    pub file: String,
    pub id: String,
    pub data: Value,
}

impl Vector {
    pub fn rules(&self) -> Vec<&str> {
        self.data["rules"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    pub fn steps(&self) -> &[Value] {
        self.data["steps"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn setup(&self) -> &Value {
        &self.data["setup"]
    }

    /// Whether the vector runs on `chain`: every chain unless `applies_to`
    /// lists fewer.
    pub fn applies_to(&self, chain: &str) -> bool {
        match self.data["applies_to"].as_array() {
            Some(list) => list.iter().any(|c| c.as_str() == Some(chain)),
            None => true,
        }
    }

    /// Every contract function the vector needs, including the ones its
    /// setup calls on its behalf: `create` for the mandate it starts with and
    /// `allow_recipients` for its allowlist.
    pub fn functions(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let setup = self.setup();
        if setup["mandate"].is_object() {
            out.insert(String::from("create"));
        }
        if setup["allowlist"]
            .as_array()
            .is_some_and(|list| !list.is_empty())
        {
            out.insert(String::from("allow_recipients"));
        }
        for step in self.steps() {
            if step["step"] == "call" {
                if let Some(f) = step["fn"].as_str() {
                    out.insert(f.to_string());
                }
            }
        }
        out
    }
}

/// Which vectors a run covers. Every filter given must match.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// Rule areas: a vector matches when it cites a rule in one of them.
    pub areas: Option<BTreeSet<String>>,
    /// Functions: a vector matches when every function it needs is listed.
    pub only_calls: Option<BTreeSet<String>>,
}

impl Selection {
    pub fn selects(&self, v: &Vector) -> bool {
        if let Some(areas) = &self.areas {
            let cites = v
                .rules()
                .iter()
                .filter_map(|r| r.split('-').nth(1))
                .any(|area| areas.contains(area));
            if !cites {
                return false;
            }
        }
        if let Some(calls) = &self.only_calls {
            if !v.functions().iter().all(|f| calls.contains(f)) {
                return false;
            }
        }
        true
    }
}

/// `--only-calls a,b,c`. Unknown names are refused, so a typo cannot quietly
/// select nothing.
pub fn parse_only_calls(list: &str) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if CALL_FNS.contains(&name) {
            out.insert(name.to_string());
        } else if !OTHER_STEPS.contains(&name) {
            return Err(format!(
                "--only-calls: {name:?} is neither a contract function nor a step kind"
            ));
        }
    }
    Ok(out)
}

/// `--areas A,B`: rule areas such as PAUSE or SWEEP.
pub fn parse_areas(list: &str) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for area in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if !area.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(format!(
                "--areas: {area:?} is not a rule area such as PAUSE"
            ));
        }
        out.insert(area.to_string());
    }
    if out.is_empty() {
        return Err(String::from("--areas needs at least one area"));
    }
    Ok(out)
}

/// Every vector file in `dir`, sorted, leaving out the schema.
pub fn discover(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("cannot list {}: {e}", dir.display()))?
            .path();
        let is_json = path.extension().is_some_and(|x| x == "json");
        let is_schema = path.file_name().is_some_and(|n| n == "schema.json");
        if is_json && !is_schema && path.is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Loads every vector of every file, refusing a repeated ID across files.
pub fn load(paths: &[PathBuf]) -> Result<Vec<Vector>> {
    let mut out: Vec<Vector> = Vec::new();
    let mut ids = BTreeSet::new();
    for path in paths {
        let file = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {file}: {e}"))?;
        for v in parse(&file, &text)? {
            if !ids.insert(v.id.clone()) {
                return Err(format!(
                    "{file}: vector {} appears in more than one file",
                    v.id
                ));
            }
            out.push(v);
        }
    }
    Ok(out)
}

/// Parses one vector file. The schema and check-vectors.sh have already
/// checked the content, so this only checks what the runner depends on.
pub fn parse(file: &str, text: &str) -> Result<Vec<Vector>> {
    let data: Value = serde_json::from_str(text).map_err(|e| format!("{file}: {e}"))?;
    if data["format"] != FORMAT {
        return Err(format!("{file}: format is not {FORMAT}"));
    }
    let list = data["vectors"]
        .as_array()
        .ok_or_else(|| format!("{file}: no vectors list"))?;
    let mut out = Vec::new();
    for v in list {
        let id = v["id"]
            .as_str()
            .ok_or_else(|| format!("{file}: a vector without an id"))?;
        if !v["steps"].is_array() || !v["setup"].is_object() {
            return Err(format!("{file}: vector {id} has no setup or no steps"));
        }
        out.push(Vector {
            file: file.to_string(),
            id: id.to_string(),
            data: v.clone(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod test {
    use super::*;

    fn vector(json: &str) -> Vector {
        let text = format!("{{\"format\":\"{FORMAT}\",\"vectors\":[{json}]}}");
        parse("t.json", &text).unwrap().remove(0)
    }

    #[test]
    fn implicit_setup_calls_count_as_functions() {
        let v = vector(
            r##"{"id":"a","rules":["MANDATE-PAUSE-001"],"setup":{"now":"0","mandate":{},"allowlist":[{"recipient_ref":"#r","destination":{"address":"bob","tag":null}}]},"steps":[{"step":"call","fn":"pause","caller":"g","expect":{"error":"X","events":[]}},{"step":"advance_time","seconds":"1"}]}"##,
        );
        let want: BTreeSet<String> = ["allow_recipients", "create", "pause"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(v.functions(), want);
    }

    #[test]
    fn unknown_only_calls_names_are_refused() {
        assert!(parse_only_calls("state,stat").is_err());
        let calls = parse_only_calls("state, transfer,mint").unwrap();
        assert_eq!(calls.len(), 1);
        assert!(parse_areas("pause").is_err());
    }

    #[test]
    fn discover_finds_every_vector_file_but_the_schema() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors");
        let files = discover(&dir).unwrap();
        assert!(files.iter().any(|f| f.ends_with("example.json")));
        assert!(!files.iter().any(|f| f.ends_with("schema.json")));
        let all = load(&files).unwrap();
        assert!(all
            .iter()
            .any(|v| v.id == "pause_blocks_requests_until_resume"));
    }
}
