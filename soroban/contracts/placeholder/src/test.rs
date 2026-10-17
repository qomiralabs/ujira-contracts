// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

use super::{Placeholder, PlaceholderClient};
use soroban_sdk::Env;

#[test]
fn version_is_one() {
    let env = Env::default();
    let contract_id = env.register(Placeholder, ());
    let client = PlaceholderClient::new(&env, &contract_id);

    assert_eq!(client.version(), 1);
}
