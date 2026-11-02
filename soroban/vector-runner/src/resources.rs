// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The resource snapshot: what each public contract function uses, measured on
//! the compiled wasm through the same driver the vectors run on.
//!
//! `soroban/vector-runner/resources/scenarios.json` names one successful call
//! for every public function, by the vector that makes it, plus the heavy cases
//! that decide whether a batch fits in a transaction. A scenario's call runs
//! exactly as it does in its vector, signed by the real key, so the figures
//! include authorization. `scripts/check-resources.py` compares the result with
//! the committed `soroban/resource-snapshot.json`.

use crate::spec::Spec;
use crate::stellar::{self, CallResources, Contracts, Outcome, World};
use crate::vectors::{self, Vector};
use crate::Result;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;

/// The factory's one call a vector makes; a scenario measures it.
const FACTORY_CREATE: &str = "create";
/// Any vector with a factory will do for reading the factory's views.
const FACTORY_VIEWS_VECTOR: &str = "amount_one_unit_accepted_and_paid";

/// One measured call: the `occurrence`th call to `function` in `vector`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    pub name: String,
    pub vector: String,
    pub function: String,
    pub occurrence: u64,
}

/// Reads the scenario list. Every field is required, so an entry cannot quietly
/// measure something other than what its name says.
pub fn load_scenarios(path: &Path) -> Result<Vec<Scenario>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let data: Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    let entries = data["scenarios"]
        .as_array()
        .ok_or_else(|| format!("{} has no scenarios list", path.display()))?;
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for entry in entries {
        let field = |key: &str| {
            entry[key]
                .as_str()
                .map(String::from)
                .ok_or_else(|| format!("scenario {entry} has no {key}"))
        };
        let scenario = Scenario {
            name: field("name")?,
            vector: field("vector")?,
            function: field("fn")?,
            occurrence: entry["occurrence"]
                .as_u64()
                .ok_or_else(|| format!("scenario {entry} has no occurrence"))?,
        };
        if scenario.occurrence == 0 {
            return Err(format!(
                "scenario {}: occurrence counts from 1",
                scenario.name
            ));
        }
        if !names.insert(scenario.name.clone()) {
            return Err(format!("scenario {} is listed twice", scenario.name));
        }
        out.push(scenario);
    }
    Ok(out)
}

/// The functions a contract exports, read from the wasm's export section. Names
/// starting with an underscore are the host's entry points, not public calls.
pub fn exported_functions(wasm: &[u8]) -> Result<Vec<String>> {
    fn leb(bytes: &[u8], at: &mut usize) -> Result<usize> {
        let (mut value, mut shift) = (0usize, 0u32);
        loop {
            let byte = *bytes
                .get(*at)
                .ok_or_else(|| String::from("the wasm ends inside a number"))?;
            *at += 1;
            value |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift > 28 {
                return Err(String::from("a number in the wasm is too long"));
            }
        }
    }
    if wasm.get(..4) != Some(b"\0asm".as_slice()) {
        return Err(String::from("not a wasm module"));
    }
    let mut at = 8;
    let mut names = Vec::new();
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let size = leb(wasm, &mut at)?;
        let end = at + size;
        if id == 7 {
            let count = leb(wasm, &mut at)?;
            for _ in 0..count {
                let length = leb(wasm, &mut at)?;
                let name = wasm
                    .get(at..at + length)
                    .ok_or_else(|| String::from("an export name runs past the wasm"))?;
                at += length;
                let kind = *wasm
                    .get(at)
                    .ok_or_else(|| String::from("the wasm ends inside an export"))?;
                at += 1;
                leb(wasm, &mut at)?;
                let name = String::from_utf8_lossy(name).into_owned();
                if kind == 0 && !name.starts_with('_') {
                    names.push(name);
                }
            }
        }
        at = end;
    }
    names.sort();
    Ok(names)
}

/// Runs the vector up to the scenario's call, which must succeed, and returns
/// what that call used.
pub fn measure(
    spec: &Spec,
    contracts: &Contracts,
    v: &Vector,
    s: &Scenario,
) -> Result<CallResources> {
    let mut world = World::new(spec, contracts, v)?;
    world
        .setup(v)
        .map_err(|e| format!("{}: setup: {e}", s.name))?;
    let mut seen = 0;
    for (i, step) in v.steps().iter().enumerate() {
        let (record, stop) = world.step(i + 1, step)?;
        if stop {
            return Err(format!(
                "{}: the test host panicked at step {}",
                s.name,
                i + 1
            ));
        }
        if step["step"] != "call" || step["fn"] != s.function.as_str() {
            continue;
        }
        seen += 1;
        if seen == s.occurrence {
            if record["result"].get("ok") != Some(&Value::Bool(true)) {
                return Err(format!(
                    "{}: {} in {} did not succeed: {}",
                    s.name, s.function, s.vector, record["result"]
                ));
            }
            return world
                .call_resources()
                .ok_or_else(|| format!("{}: the host reported no resources", s.name));
        }
    }
    Err(format!(
        "{}: {} has no call {} to {}",
        s.name, s.vector, s.occurrence, s.function
    ))
}

/// Measures every scenario, and refuses a run that leaves a public function
/// without one, so a new call cannot arrive without its figures.
pub fn snapshot(
    spec: &Spec,
    contracts: &Contracts,
    vectors: &[Vector],
    scenarios: &[Scenario],
) -> Result<Value> {
    let mut public: BTreeSet<String> = exported_functions(&contracts.mandate)?
        .into_iter()
        .collect();
    public.insert(String::from(FACTORY_CREATE));
    let covered: BTreeSet<&str> = scenarios.iter().map(|s| s.function.as_str()).collect();
    let missing: Vec<&str> = public
        .iter()
        .map(String::as_str)
        .filter(|f| !covered.contains(f))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "no scenario measures {}; add one to soroban/vector-runner/resources/scenarios.json",
            missing.join(", ")
        ));
    }
    let find = |id: &str| {
        vectors
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| format!("no vector {id}"))
    };
    let mut out = Map::new();
    for s in scenarios {
        let v = find(&s.vector).map_err(|e| format!("{}: {e}", s.name))?;
        out.insert(s.name.clone(), figures(&measure(spec, contracts, v, s)?));
    }
    // The factory's other functions are views with no arguments that no vector
    // calls. A new one with arguments fails here until it gets a scenario.
    let v = find(FACTORY_VIEWS_VECTOR)?;
    let mut world = World::new(spec, contracts, v)?;
    world.setup(v)?;
    for f in exported_functions(&contracts.factory)? {
        if f == FACTORY_CREATE {
            continue;
        }
        let factory = world.factory().clone();
        match stellar::call(world.env(), &factory, &f, &[])? {
            Outcome::Ok(_) => {}
            other => return Err(format!("factory {f} did not succeed: {other:?}")),
        }
        let r = stellar::last_invocation(world.env())
            .ok_or_else(|| format!("factory {f}: the host reported no resources"))?;
        out.insert(format!("factory_{f}"), figures(&r));
    }
    Ok(json!({ "scenarios": out }))
}

fn figures(r: &CallResources) -> Value {
    json!({
        "instructions": r.instructions,
        "memory_bytes": r.memory_bytes,
        "read_entries": r.read_entries,
        "write_entries": r.write_entries,
        "read_bytes": r.read_bytes,
        "write_bytes": r.write_bytes,
    })
}

/// Every vector the scenarios may name: the shared ones, plus the ones under
/// `soroban/vector-runner/resources` that exist only to be measured.
pub fn load_vectors(root: &Path) -> Result<Vec<Vector>> {
    let mut files = vectors::discover(&root.join("spec/vectors"))?;
    files.push(root.join("soroban/vector-runner/resources/vectors.json"));
    vectors::load(&files)
}

#[cfg(test)]
mod test {
    use super::*;

    /// A module with one exported function, one exported memory and one export
    /// the host calls.
    fn module() -> Vec<u8> {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        // A custom section first, which must be stepped over.
        wasm.extend([0, 3, 1, b'x', b'y']);
        let exports: Vec<u8> = [
            vec![3],
            vec![4],
            b"call".to_vec(),
            vec![0, 0],
            vec![3],
            b"mem".to_vec(),
            vec![2, 0],
            vec![2],
            b"__".to_vec(),
            vec![0, 1],
        ]
        .concat();
        wasm.extend([7, u8::try_from(exports.len()).unwrap()]);
        wasm.extend(exports);
        wasm
    }

    #[test]
    fn exports_are_public_functions_only() {
        assert_eq!(exported_functions(&module()).unwrap(), vec!["call"]);
    }

    #[test]
    fn a_file_that_is_not_wasm_is_refused() {
        assert!(exported_functions(b"not wasm").is_err());
    }

    #[test]
    fn a_truncated_export_is_refused() {
        let wasm = module();
        assert!(exported_functions(&wasm[..wasm.len() - 2]).is_err());
    }

    fn write(name: &str, text: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("resources-test-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn scenarios_load_and_every_field_is_required() {
        let good = write(
            "good.json",
            r#"{ "scenarios": [ { "name": "a", "vector": "v", "fn": "f", "occurrence": 2 } ] }"#,
        );
        assert_eq!(
            load_scenarios(&good).unwrap(),
            vec![Scenario {
                name: String::from("a"),
                vector: String::from("v"),
                function: String::from("f"),
                occurrence: 2
            }]
        );
        let no_fn = write(
            "no-fn.json",
            r#"{ "scenarios": [ { "name": "a", "vector": "v", "occurrence": 1 } ] }"#,
        );
        assert!(load_scenarios(&no_fn).unwrap_err().contains("has no fn"));
        let zero = write(
            "zero.json",
            r#"{ "scenarios": [ { "name": "a", "vector": "v", "fn": "f", "occurrence": 0 } ] }"#,
        );
        assert!(load_scenarios(&zero).unwrap_err().contains("counts from 1"));
        let twice = write(
            "twice.json",
            r#"{ "scenarios": [ { "name": "a", "vector": "v", "fn": "f", "occurrence": 1 },
                                { "name": "a", "vector": "w", "fn": "f", "occurrence": 1 } ] }"#,
        );
        assert!(load_scenarios(&twice).unwrap_err().contains("listed twice"));
    }
}
