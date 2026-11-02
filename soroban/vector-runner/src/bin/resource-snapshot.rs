// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Prints the resource snapshot as JSON, with sorted keys, for
//! `scripts/check-resources.py` to compare with the committed one. Reads the wasm
//! `scripts/soroban-build.sh` leaves.
//!
//! Exit codes: 0 printed, 2 a scenario could not be measured.

use std::process::ExitCode;
use vector_runner::resources::{load_scenarios, load_vectors, snapshot};
use vector_runner::run::{repo_root, Options};
use vector_runner::spec::Spec;
use vector_runner::stellar::Contracts;

fn real_main() -> Result<String, String> {
    let root = repo_root();
    let spec = Spec::load(&root)?;
    let contracts = Contracts::load(
        &Options::default_wasm(&root, "ujira_factory"),
        &Options::default_wasm(&root, "ujira_mandate"),
    )?;
    let vectors = load_vectors(&root)?;
    let scenarios = load_scenarios(&root.join("soroban/vector-runner/resources/scenarios.json"))?;
    let out = snapshot(&spec, &contracts, &vectors, &scenarios)?;
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    match real_main() {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("resource-snapshot: {e}");
            ExitCode::from(2)
        }
    }
}
