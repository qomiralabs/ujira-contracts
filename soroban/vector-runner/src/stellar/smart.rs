// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Passkey wallets as actors: the pinned OpenZeppelin smart account, deployed
//! through the wallet factory, with test P-256 passkeys that sign real WebAuthn
//! assertions. A call such an actor authorizes runs the account's own
//! `__check_auth`, its WebAuthn verifier and its weighted threshold policy, as
//! it would on a network.
//!
//! Also a test account whose `__check_auth` calls back into the contract that
//! asked for its authorization, to show the host refuses that.

use crate::values::sha256;
use p256::ecdsa::signature::hazmat::PrehashSigner;
use p256::ecdsa::{Signature, SigningKey};
use soroban_sdk::auth::{Context, CustomAccountInterface};
use soroban_sdk::crypto::Hash;
use soroban_sdk::xdr::{ScVal, ToXdr};
use soroban_sdk::{
    contract, contractimpl, contracttype, Address, Bytes, BytesN, Env, IntoVal, Map, Symbol,
    TryFromVal, Val, Vec,
};
use std::panic::AssertUnwindSafe;

/// The pinned upstream wasm, exactly as `scripts/check-wallet-hash.sh` checks
/// it against the hashes `scripts/build-smart-account.sh` pins.
pub const ACCOUNT_WASM: &[u8] =
    include_bytes!("../../../contracts/wallet-factory/fixtures/multisig_account_example.wasm");
pub const VERIFIER_WASM: &[u8] = include_bytes!(
    "../../../contracts/wallet-factory/fixtures/multisig_webauthn_verifier_example.wasm"
);
pub const POLICY_WASM: &[u8] = include_bytes!(
    "../../../contracts/wallet-factory/fixtures/multisig_weighted_threshold_policy_example.wasm"
);

/// The relying party the test passkeys belong to. Nothing on chain checks it.
const RP_ID: &str = "ujira.test";
/// User present and user verified, which the verifier requires.
const FLAGS: u8 = 0x05;
/// The constructor's `Default` rule, the only rule a new wallet has.
const DEFAULT_RULE: u32 = 0;

/// The upstream types, with their field and variant names, so they encode the
/// same as the account expects.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Signer {
    Delegated(Address),
    External(Address, Bytes),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthPayload {
    pub signers: Map<Signer, Bytes>,
    pub context_rule_ids: Vec<u32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthDigestPreimage {
    pub account: Address,
    pub signature_payload: BytesN<32>,
    pub context_rule_ids: Vec<u32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebAuthnSigData {
    pub signature: BytesN<64>,
    pub authenticator_data: Bytes,
    pub client_data: Bytes,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeightedThresholdAccountParams {
    pub signer_weights: Map<Signer, u32>,
    pub threshold: u32,
}

/// The contracts every wallet of one world shares: the factory, and the one
/// verifier and one policy a network deploys once.
#[derive(Clone)]
pub struct Kit {
    pub factory: Address,
    pub verifier: Address,
    pub policy: Address,
}

impl Kit {
    pub fn deploy(env: &Env, wallet_factory_wasm: &[u8]) -> crate::Result<Kit> {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            let account_hash = env.deployer().upload_contract_wasm(ACCOUNT_WASM);
            let verifier = env.register(VERIFIER_WASM, ());
            let policy = env.register(POLICY_WASM, ());
            let factory = env.register(
                wallet_factory_wasm,
                (account_hash, Vec::<Address>::new(env)),
            );
            Kit {
                factory,
                verifier,
                policy,
            }
        }))
        .map_err(|_| String::from("the wallet factory, verifier or policy could not be registered"))
    }
}

/// A wallet's passkeys and how many must sign.
#[derive(Clone)]
pub struct SmartAccount {
    pub keys: std::vec::Vec<SigningKey>,
    pub threshold: u32,
    pub verifier: Address,
}

/// The test passkey `index` of the actor `name`: its P-256 secret is the
/// SHA-256 of a fixed label, so every run makes the same wallet.
fn passkey(name: &str, index: u32) -> crate::Result<SigningKey> {
    let seed = sha256(format!("ujira-test-passkey:{name}:{index}").as_bytes());
    SigningKey::from_slice(&seed).map_err(|e| format!("no passkey for {name}: {e}"))
}

/// The 65-byte uncompressed public key, the key data the verifier takes.
fn public_key(env: &Env, key: &SigningKey) -> Bytes {
    Bytes::from_slice(env, key.verifying_key().to_encoded_point(false).as_bytes())
}

impl SmartAccount {
    fn signer(&self, env: &Env, key: &SigningKey) -> Signer {
        Signer::External(self.verifier.clone(), public_key(env, key))
    }
}

/// Deploys a wallet for `name` with `signers` passkeys of weight 1 and the
/// weighted threshold policy set to `threshold`, through the wallet factory.
pub fn create(
    env: &Env,
    kit: &Kit,
    name: &str,
    signers: u32,
    threshold: u32,
) -> crate::Result<(Address, SmartAccount)> {
    if signers == 0 || threshold == 0 || threshold > signers {
        return Err(format!(
            "{name}: a smart account needs 1 to {signers} as its threshold, got {threshold}"
        ));
    }
    let keys = (0..signers)
        .map(|i| passkey(name, i))
        .collect::<crate::Result<std::vec::Vec<_>>>()?;
    let account = SmartAccount {
        keys,
        threshold,
        verifier: kit.verifier.clone(),
    };
    let mut list = Vec::new(env);
    let mut weights = Map::new(env);
    for key in &account.keys {
        let s = account.signer(env, key);
        list.push_back(s.clone());
        weights.set(s, 1u32);
    }
    let params = WeightedThresholdAccountParams {
        signer_weights: weights,
        threshold,
    };
    let mut policies: Map<Address, Val> = Map::new(env);
    policies.set(kit.policy.clone(), params.into_val(env));
    let salt = BytesN::from_array(env, &sha256(name.as_bytes()));
    let address = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let args: Vec<Val> = (list, policies, salt).into_val(env);
        env.invoke_contract::<Address>(&kit.factory, &Symbol::new(env, "deploy"), args)
    }))
    .map_err(|_| format!("the wallet factory refused to deploy a wallet for {name}"))?;
    Ok((address, account))
}

/// Base64url without padding, as WebAuthn writes a challenge.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize]));
        }
    }
    out
}

/// The credential a wallet presents for one root invocation whose host
/// signature payload is `payload`, signed by its first `count` passkeys.
/// Each passkey signs a WebAuthn assertion whose challenge is the account's
/// digest of the payload, its own address and the rule used.
pub fn signature(
    env: &Env,
    account: &Address,
    wallet: &SmartAccount,
    payload: [u8; 32],
    count: u32,
) -> crate::Result<ScVal> {
    let rules = Vec::from_array(env, [DEFAULT_RULE]);
    let preimage = AuthDigestPreimage {
        account: account.clone(),
        signature_payload: BytesN::from_array(env, &payload),
        context_rule_ids: rules.clone(),
    };
    let mut xdr = std::vec![0u8; 0];
    for b in preimage.to_xdr(env).iter() {
        xdr.push(b);
    }
    let digest = sha256(&xdr);
    let client = format!(
        r#"{{"type":"webauthn.get","challenge":"{}","origin":"https://{RP_ID}","crossOrigin":false}}"#,
        base64url(&digest)
    );
    let mut auth_data = sha256(RP_ID.as_bytes()).to_vec();
    auth_data.push(FLAGS);
    auth_data.extend_from_slice(&[0, 0, 0, 0]);
    let mut message = auth_data.clone();
    message.extend_from_slice(&sha256(client.as_bytes()));
    let to_sign = sha256(&message);

    let mut signers = Map::new(env);
    let count = usize::try_from(count).map_err(|_| String::from("too many signatures"))?;
    if count > wallet.keys.len() {
        return Err(format!(
            "the wallet has {} passkeys, so {count} cannot sign",
            wallet.keys.len()
        ));
    }
    for key in wallet.keys.iter().take(count) {
        let sig: Signature = key
            .sign_prehash(&to_sign)
            .map_err(|e| format!("a passkey could not sign: {e}"))?;
        // The host verifies secp256r1 only in its low-s form.
        let sig = sig.normalize_s().unwrap_or(sig);
        let mut raw = [0u8; 64];
        raw.copy_from_slice(&sig.to_bytes());
        let data = WebAuthnSigData {
            signature: BytesN::from_array(env, &raw),
            authenticator_data: Bytes::from_slice(env, &auth_data),
            client_data: Bytes::from_slice(env, client.as_bytes()),
        };
        signers.set(wallet.signer(env, key), data.to_xdr(env));
    }
    let auth = AuthPayload {
        signers,
        context_rule_ids: rules,
    };
    let val: Val = auth.into_val(env);
    ScVal::try_from_val(env, &val).map_err(|e| format!("cannot encode the wallet signature: {e:?}"))
}

/// A test account that, while it is asked to authorize a call, calls the
/// `version` view of the contract that asked. The host must refuse that, since
/// that contract is already on the call stack. It lets the factory's `create`
/// through without calling back, so a vector's setup can make the mandate.
#[contract]
pub struct CallsBackAccount;

#[contractimpl]
impl CustomAccountInterface for CallsBackAccount {
    type Signature = ();
    type Error = soroban_sdk::Error;

    #[allow(non_snake_case)]
    fn __check_auth(
        env: Env,
        _signature_payload: Hash<32>,
        _signature: (),
        auth_contexts: Vec<Context>,
    ) -> core::result::Result<(), soroban_sdk::Error> {
        let create = Symbol::new(&env, "create");
        for context in auth_contexts.iter() {
            if let Context::Contract(c) = context {
                if c.fn_name != create {
                    let _: u32 = env.invoke_contract(
                        &c.contract,
                        &Symbol::new(&env, "version"),
                        Vec::new(&env),
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::base64url;

    #[test]
    fn base64url_has_no_padding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64url(&[0u8; 32]).len(), 43);
    }
}
