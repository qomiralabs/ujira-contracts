// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! `tests/run`: runs the selected shared vectors on one chain, compares each
//! trace with its vector and prints a summary.
//!
//! Exit codes: 0 every selected vector that applies to the chain ran and
//! passed; 1 a vector failed; 2 bad usage or unreadable input; 3 nothing
//! failed, but some vectors could not run because the contract wasm is not
//! built, which is never a pass.

use std::process::ExitCode;
use vector_runner::cli::Args;
use vector_runner::run::{run, Options};
use vector_runner::trace;
use vector_runner::vectors::{parse_areas, parse_only_calls, Selection};

const USAGE: &str = "usage: tests/run --chain stellar [--files a.json,b.json] [--areas A,B] \
[--only-calls f,g] [--trace-out FILE] [--factory-wasm FILE] [--mandate-wasm FILE] [--list]";

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("tests/run: {e}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> Result<u8, String> {
    let mut args = Args::from_env()?;
    let chain = args
        .value("--chain")?
        .ok_or_else(|| String::from("--chain is required"))?;
    let root = match args.value("--root")? {
        Some(r) => args.path(&r),
        None => vector_runner::run::repo_root(),
    };
    let files = args.value("--files")?.map(|list| {
        list.split(',')
            .filter(|f| !f.is_empty())
            .map(|f| args.path(f))
            .collect::<Vec<_>>()
    });
    let selection = Selection {
        areas: args
            .value("--areas")?
            .map(|a| parse_areas(&a))
            .transpose()?,
        only_calls: args
            .value("--only-calls")?
            .map(|c| parse_only_calls(&c))
            .transpose()?,
    };
    let trace_out = args.value("--trace-out")?.map(|p| args.path(&p));
    let factory_wasm = match args.value("--factory-wasm")? {
        Some(p) => args.path(&p),
        None => Options::default_wasm(&root, "ujira_factory"),
    };
    let mandate_wasm = match args.value("--mandate-wasm")? {
        Some(p) => args.path(&p),
        None => Options::default_wasm(&root, "ujira_mandate"),
    };
    let list = args.flag("--list");
    if let Some(extra) = args.positional()?.first() {
        return Err(format!("unexpected argument {extra}"));
    }

    let opts = Options {
        chain,
        root,
        files,
        selection,
        factory_wasm,
        mandate_wasm,
    };
    if list {
        for v in vector_runner::run::select(&opts)? {
            println!("{} {}", v.file, v.id);
        }
        return Ok(0);
    }

    let summary = run(&opts)?;
    if let Some(path) = &trace_out {
        trace::write(path, &summary.trace)?;
        println!("trace written to {}", path.display());
    }
    for m in &summary.failed {
        println!("FAIL {m}");
    }
    if let Some(reason) = &summary.not_runnable_reason {
        println!("not runnable: {reason}");
    }
    println!(
        "{} selected: {} passed, {} failed, {} not runnable, {} skipped for {}",
        summary.selected,
        summary.passed.len(),
        summary.failed.len(),
        summary.not_runnable.len(),
        summary.skipped.len(),
        opts.chain,
    );
    if !summary.failed.is_empty() {
        return Ok(1);
    }
    if !summary.not_runnable.is_empty() {
        return Ok(3);
    }
    Ok(0)
}
