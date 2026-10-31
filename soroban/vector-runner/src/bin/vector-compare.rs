// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! `tests/compare`: compares a trace with the vectors it ran, or two traces
//! with each other, and names the vector, the step and the field of every
//! difference.
//!
//! - `tests/compare TRACE [--files a.json,b.json]` checks each vector the
//!   trace ran against its expectations and the invariants.
//! - `tests/compare TRACE_A TRACE_B` compares the two step by step.
//!
//! Exit codes: 0 no difference, 1 a difference, 2 bad usage or unreadable
//! input.

use std::path::PathBuf;
use std::process::ExitCode;
use vector_runner::cli::Args;
use vector_runner::compare::{compare_traces, compare_vector};
use vector_runner::trace::{self, status};
use vector_runner::vectors;

const USAGE: &str =
    "usage: tests/compare TRACE [--files a.json,b.json] | tests/compare TRACE_A TRACE_B";

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("tests/compare: {e}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> Result<u8, String> {
    let mut args = Args::from_env()?;
    let root = vector_runner::run::repo_root();
    let files = args.value("--files")?;
    let cwd = args.cwd.clone();
    let traces: Vec<PathBuf> = args
        .positional()?
        .iter()
        .map(|p| vector_runner::cli::resolve(&cwd, p))
        .collect();
    match traces.as_slice() {
        [a, b] => {
            let diffs = compare_traces(&trace::read(a)?, &trace::read(b)?);
            for d in &diffs {
                println!("DIFF {d}");
            }
            println!("{} difference(s)", diffs.len());
            Ok(u8::from(!diffs.is_empty()))
        }
        [t] => {
            let tr = trace::read(t)?;
            let chain = tr["chain"]
                .as_str()
                .ok_or_else(|| String::from("the trace names no chain"))?
                .to_string();
            let paths = match files {
                Some(list) => list
                    .split(',')
                    .filter(|f| !f.is_empty())
                    .map(|f| vector_runner::cli::resolve(&cwd, f))
                    .collect(),
                None => vectors::discover(&root.join("spec/vectors"))?,
            };
            let all = vectors::load(&paths)?;
            let mut failed = 0usize;
            let mut checked = 0usize;
            for tv in trace::vectors(&tr) {
                let st = tv["status"].as_str().unwrap_or("");
                if st == status::SKIPPED || st == status::NOT_RUNNABLE {
                    continue;
                }
                let id = tv["id"].as_str().unwrap_or("?");
                let v = all
                    .iter()
                    .find(|v| v.id == id)
                    .ok_or_else(|| format!("the trace ran {id}, which no vector file has"))?;
                checked += 1;
                if let Err(m) = compare_vector(v, tv, &chain) {
                    println!("FAIL {m}");
                    failed += 1;
                }
            }
            println!("{checked} compared, {failed} failed");
            Ok(u8::from(failed > 0))
        }
        _ => Err(String::from("give one trace or two")),
    }
}
