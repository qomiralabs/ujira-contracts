// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Command line parsing shared by the two binaries. Paths on the command
//! line are relative to `--cwd`, the directory the caller ran from, because
//! the wrapper scripts run cargo from `soroban/` so its toolchain pin
//! applies.

use crate::Result;
use std::path::{Path, PathBuf};

pub struct Args {
    rest: Vec<String>,
    pub cwd: PathBuf,
}

impl Args {
    pub fn from_env() -> Result<Args> {
        let mut rest: Vec<String> = std::env::args().skip(1).collect();
        let cwd = match take_value(&mut rest, "--cwd")? {
            Some(c) => PathBuf::from(c),
            None => std::env::current_dir().map_err(|e| format!("no current directory: {e}"))?,
        };
        Ok(Args { rest, cwd })
    }

    pub fn value(&mut self, flag: &str) -> Result<Option<String>> {
        take_value(&mut self.rest, flag)
    }

    pub fn flag(&mut self, flag: &str) -> bool {
        match self.rest.iter().position(|a| a == flag) {
            Some(i) => {
                self.rest.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn path(&self, p: &str) -> PathBuf {
        resolve(&self.cwd, p)
    }

    /// Whatever is left once every known flag was taken.
    pub fn positional(self) -> Result<Vec<String>> {
        if let Some(unknown) = self.rest.iter().find(|a| a.starts_with("--")) {
            return Err(format!("unknown option {unknown}"));
        }
        Ok(self.rest)
    }
}

pub fn resolve(cwd: &Path, p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn take_value(rest: &mut Vec<String>, flag: &str) -> Result<Option<String>> {
    let Some(i) = rest.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if i + 1 >= rest.len() {
        return Err(format!("{flag} needs a value"));
    }
    let v = rest.remove(i + 1);
    rest.remove(i);
    Ok(Some(v))
}
