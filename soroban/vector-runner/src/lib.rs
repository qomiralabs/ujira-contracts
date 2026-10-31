// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The shared vector runner.
//!
//! A vector file says, as data, what the mandate must do. This crate turns
//! each vector into a run against the compiled factory and mandate wasm on the
//! Soroban test host, records what happened as a trace, and compares the
//! trace with the vector's expectations, checking the spec's invariants after
//! every step. The comparator and the trace format are chain neutral; only
//! `stellar` knows about Soroban.
//!
//! - `vectors`: loading vector files and choosing which vectors run.
//! - `trace`: the trace format, written by a driver and read by the comparator.
//! - `canon`: the one canonical form both sides are compared in.
//! - `compare`: a trace against its vector, or two traces against each other.
//! - `invariants`: I1 to I10, checked after every step of a trace.
//! - `attest`: the attestation message, signatures and tamper kinds.
//! - `stellar`: the Soroban driver.

pub mod attest;
pub mod canon;
pub mod cli;
pub mod compare;
pub mod invariants;
pub mod run;
pub mod spec;
pub mod stellar;
pub mod trace;
pub mod values;
pub mod vectors;

/// Every failure in the runner is a message for a person to read; nothing
/// recovers from one programmatically.
pub type Result<T> = std::result::Result<T, String>;
