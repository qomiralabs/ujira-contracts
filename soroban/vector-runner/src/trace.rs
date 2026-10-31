// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The trace format, `ujira-mandate-trace/1`. `tests/trace.schema.json` is
//! its schema and `tests/README.md` describes every field.
//!
//! A trace records what a chain did with each vector, in the vector's own
//! written forms: actor names for addresses, labels or hex for 32-byte
//! values, decimal strings for amounts and times, names for statuses and
//! errors, and `{ address, tag }` for destinations. A driver writes it and the
//! comparator reads it, so the comparator never sees anything chain
//! specific.

use crate::Result;
use serde_json::{json, Value};
use std::path::Path;

pub const FORMAT: &str = "ujira-mandate-trace/1";

/// What happened to a vector in a run.
pub mod status {
    /// Every step ran and was recorded.
    pub const RAN: &str = "ran";
    /// The run stopped part way, for the reason given; the steps so far are
    /// recorded.
    pub const ABORTED: &str = "aborted";
    /// The vector cannot run yet, for example because the contract wasm it
    /// needs has not been built. Never a pass.
    pub const NOT_RUNNABLE: &str = "not_runnable";
    /// The vector does not apply to this chain.
    pub const SKIPPED: &str = "skipped";
}

pub fn new_trace(chain: &str, contracts: Value, vectors: Vec<Value>) -> Value {
    json!({
        "format": FORMAT,
        "chain": chain,
        "contracts": contracts,
        "vectors": vectors,
    })
}

/// A vector that did not run, with the reason.
pub fn unrun(id: &str, file: &str, status: &str, reason: &str) -> Value {
    json!({ "id": id, "file": file, "status": status, "reason": reason })
}

pub fn read(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn parse(text: &str) -> Result<Value> {
    let trace: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if trace["format"] != FORMAT {
        return Err(format!("format is not {FORMAT}"));
    }
    if !trace["vectors"].is_array() {
        return Err(String::from("no vectors list"));
    }
    Ok(trace)
}

/// Pretty JSON with a trailing newline. serde_json keeps object keys sorted,
/// so the same run always writes the same bytes.
pub fn to_text(trace: &Value) -> Result<String> {
    let mut text = serde_json::to_string_pretty(trace).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

pub fn write(path: &Path, trace: &Value) -> Result<()> {
    std::fs::write(path, to_text(trace)?)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn vectors(trace: &Value) -> &[Value] {
    trace["vectors"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub fn find<'a>(trace: &'a Value, id: &str) -> Option<&'a Value> {
    vectors(trace).iter().find(|v| v["id"] == id)
}
