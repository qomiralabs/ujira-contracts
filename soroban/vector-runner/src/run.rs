// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! One run of the selected vectors on one chain: run each, compare its trace
//! with its expectations, and count what passed, what failed and what could
//! not run.

use crate::compare::{compare_vector, Mismatch};
use crate::spec::Spec;
use crate::stellar::{self, Contracts, PrincipalMode};
use crate::trace::{self, status};
use crate::vectors::{self, Selection, Vector};
use crate::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Options {
    pub chain: String,
    /// The repository root, where `spec/` is.
    pub root: PathBuf,
    /// Vector files to run; every file in `spec/vectors` when None.
    pub files: Option<Vec<PathBuf>>,
    pub selection: Selection,
    pub factory_wasm: PathBuf,
    pub mandate_wasm: PathBuf,
    /// Who the principal is: as each vector declares, or a passkey wallet.
    pub principal: PrincipalMode,
}

impl Options {
    /// Where `scripts/soroban-build.sh` leaves the production contracts.
    pub fn default_wasm(root: &Path, crate_name: &str) -> PathBuf {
        root.join("soroban/target/wasm32v1-none/release")
            .join(format!("{crate_name}.wasm"))
    }
}

#[derive(Default)]
pub struct Summary {
    pub selected: usize,
    pub passed: Vec<String>,
    pub failed: Vec<Mismatch>,
    pub not_runnable: Vec<String>,
    pub not_runnable_reason: Option<String>,
    pub skipped: Vec<String>,
    pub trace: Value,
}

pub fn select(opts: &Options) -> Result<Vec<Vector>> {
    let files = match &opts.files {
        Some(f) => f.clone(),
        None => vectors::discover(&opts.root.join("spec/vectors"))?,
    };
    let all = vectors::load(&files)?;
    // A trace names each file relative to the repository, so the same run
    // writes the same trace on any machine.
    let root = std::fs::canonicalize(&opts.root).unwrap_or_else(|_| opts.root.clone());
    Ok(all
        .into_iter()
        .filter(|v| opts.selection.selects(v))
        .map(|mut v| {
            let path = std::fs::canonicalize(&v.file).unwrap_or_else(|_| PathBuf::from(&v.file));
            if let Ok(rel) = path.strip_prefix(&root) {
                v.file = rel.display().to_string();
            }
            v
        })
        .collect())
}

pub fn run(opts: &Options) -> Result<Summary> {
    if opts.chain != "stellar" {
        return Err(format!(
            "there is no runner for chain {:?}; UJIRA runs on stellar only",
            opts.chain
        ));
    }
    let spec = Spec::load(&opts.root)?;
    let selected = select(opts)?;
    let contracts = Contracts::load(&opts.factory_wasm, &opts.mandate_wasm);
    let mut summary = Summary {
        selected: selected.len(),
        ..Summary::default()
    };
    let mut entries = Vec::new();
    for v in &selected {
        if !v.applies_to(&opts.chain) {
            let reason = format!("applies to {} only", v.data["applies_to"]);
            entries.push(trace::unrun(&v.id, &v.file, status::SKIPPED, &reason));
            summary.skipped.push(v.id.clone());
            continue;
        }
        let c = match &contracts {
            Ok(c) => c,
            Err(e) => {
                let reason = format!(
                    "the contract wasm is not built ({e}); build it with scripts/soroban-build.sh"
                );
                entries.push(trace::unrun(&v.id, &v.file, status::NOT_RUNNABLE, &reason));
                summary.not_runnable.push(v.id.clone());
                summary.not_runnable_reason = Some(reason);
                continue;
            }
        };
        if opts.principal == PrincipalMode::SmartAccount {
            if let Some(why) = stellar::smart_principal_unsupported(v) {
                let reason = format!("not run with a smart account principal: {why}");
                entries.push(trace::unrun(&v.id, &v.file, status::SKIPPED, &reason));
                summary.skipped.push(v.id.clone());
                continue;
            }
        }
        let tv = stellar::run_vector_as(&spec, c, v, opts.principal);
        match compare_vector(v, &tv, &opts.chain) {
            Ok(()) => summary.passed.push(v.id.clone()),
            Err(m) => summary.failed.push(m),
        }
        entries.push(tv);
    }
    let described = contracts.as_ref().map_or(Value::Null, Contracts::describe);
    summary.trace = trace::new_trace(&opts.chain, described, entries);
    Ok(summary)
}

/// The repository this crate sits in.
pub fn repo_root() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&p).unwrap_or(p)
}
