// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Prototype of the mandate's batch request and batch execute, built only to
//! measure what a batch costs. It keeps the storage layout, check order,
//! events and transfers of the real contract, so the numbers carry over, and
//! leaves out everything a batch does not touch (approvals, cancel, pause,
//! roles beyond the agent list). It is never deployed outside testnet.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token::TokenClient,
    xdr::ToXdr, Address, Bytes, BytesN, Env, IntoVal, MuxedAddress, Symbol, Val, Vec,
};

/// Codes from the mandate's shared error table.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    Unauthorized = 10,
    MandateExpired = 21,
    AmountInvalid = 30,
    LegsInvalid = 31,
    LegsSumMismatch = 32,
    BatchInvalid = 33,
    ConfigInvalid = 34,
    RequestDeadlinePassed = 35,
    BudgetExceeded = 40,
    PaymentCapExceeded = 41,
    DailyCapExceeded = 42,
    InsufficientFunds = 43,
    FeeExceedsCap = 46,
    RecipientNotAllowed = 50,
    AttestationInvalid = 51,
    AttestationExpired = 52,
    PolicyChanged = 53,
    PayoutIdExists = 60,
    PayoutNotFound = 61,
    PayoutWrongStatus = 62,
    PayoutLapsed = 63,
    TransferFailed = 72,
    InternalInvariant = 90,
}

pub const MODE_ALLOWLIST: u32 = 1;
pub const MODE_ATTESTED: u32 = 2;
pub const MODE_BOTH: u32 = 3;

pub const STATUS_PENDING: u32 = 1;
pub const STATUS_APPROVED: u32 = 2;
pub const STATUS_EXECUTED: u32 = 3;

pub const MAX_LEGS: u32 = 4;
/// Deliberately above every candidate `MAX_BATCH`, so the spike can measure
/// batches the real contract will refuse.
pub const PROBE_MAX_BATCH: u32 = 64;

const I128_MAX: i128 = i128::MAX;
const SECONDS_PER_DAY: u64 = 86_400;
const BPS_DENOMINATOR: i128 = 10_000;
const MAX_ATTESTATION_VALIDITY: u64 = 86_400;

// Rent horizons in ledgers (5 s each). Records must outlive the longest
// payout TTL (30 days); a daily bucket only has to outlive its day.
const RECORD_TTL: u32 = 535_680;
const DAILY_TTL: u32 = 34_560;
const INSTANCE_TTL: u32 = 535_680;
const INSTANCE_BUMP_AT: u32 = 518_400;

const DOMAIN: &[u8; 15] = b"UJIRA/ATTEST/V1";

// secp256k1 group order n and floor(n / 2), big-endian.
const ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
];
const HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];
const ZERO: [u8; 32] = [0; 32];

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leg {
    pub destination: MuxedAddress,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attestation {
    pub valid_until: u64,
    pub signature: Bytes,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRequest {
    pub payout_id: BytesN<32>,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub deadline: u64,
    pub task_ref: BytesN<32>,
    pub attestations: Vec<Attestation>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub principal: Address,
    pub asset: Address,
    pub fee_address: Address,
    pub agents: Vec<Address>,
    pub recipient_mode: u32,
    pub attester: BytesN<65>,
    pub country_policy_hash: BytesN<32>,
    pub max_fee_bps: u32,
    pub total_budget: i128,
    pub per_payment_cap: i128,
    pub per_recipient_daily_cap: i128,
    pub approval_threshold: i128,
    pub payout_ttl_seconds: u64,
    pub expires_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    pub spent: i128,
    pub pending: i128,
    pub committed: i128,
    pub policy_epoch: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Payout {
    pub agent: Address,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub status: u32,
    pub created_at: u64,
    pub expires_at: u64,
    pub policy_epoch: u32,
    pub approvals: Vec<Address>,
}

/// A record while the payout is open, a bare status once it is terminal.
/// Only ever serialized into storage, so the Rust size of the variants does
/// not matter, and there is no allocator to box with.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum PayoutEntry {
    Open(Payout),
    Closed(u32),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Daily {
    pub day: u64,
    pub used: i128,
}

/// A muxed address cannot sit inside a storage key, so the allowlist key
/// carries the address and the tag separately.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllowKey {
    pub recipient_ref: BytesN<32>,
    pub address: Address,
    pub tag: Option<u64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllowEntry {
    pub recipient_ref: BytesN<32>,
    pub destination: MuxedAddress,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemResult {
    pub payout_id: BytesN<32>,
    pub code: u32,
    pub status: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Key {
    Config,
    Counters,
    Payout(BytesN<32>),
    Daily(BytesN<32>),
    Allow(AllowKey),
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MandateInitialized {
    pub principal: Address,
    pub asset: Address,
    pub recipient_mode: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecipientsAllowed {
    pub count: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRequested {
    #[topic]
    pub payout_id: BytesN<32>,
    pub agent: Address,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub task_ref: BytesN<32>,
    pub status: u32,
    pub expires_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRequestRejected {
    #[topic]
    pub payout_id: BytesN<32>,
    pub agent: Address,
    pub code: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutExecuted {
    #[topic]
    pub payout_id: BytesN<32>,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub day: u64,
    pub spent: i128,
}

#[contract]
pub struct BatchProto;

#[contractimpl]
impl BatchProto {
    pub fn __constructor(env: Env, config: Config) -> Result<(), Error> {
        if config.agents.is_empty()
            || !(MODE_ALLOWLIST..=MODE_BOTH).contains(&config.recipient_mode)
            || config.attester.get(0) != Some(0x04)
        {
            return Err(Error::ConfigInvalid);
        }
        let store = env.storage().instance();
        store.set(&Key::Config, &config);
        store.set(&Key::Counters, &Counters::default());
        MandateInitialized {
            principal: config.principal,
            asset: config.asset,
            recipient_mode: config.recipient_mode,
        }
        .publish(&env);
        Ok(())
    }

    pub fn allow_recipients(env: Env, entries: Vec<AllowEntry>) -> Result<(), Error> {
        let cfg = load_config(&env)?;
        cfg.principal.require_auth();
        let store = env.storage().persistent();
        for e in entries.iter() {
            let key = Key::Allow(AllowKey {
                recipient_ref: e.recipient_ref,
                address: e.destination.address(),
                tag: e.destination.id(),
            });
            store.set(&key, &true);
            store.extend_ttl(&key, RECORD_TTL, RECORD_TTL);
        }
        RecipientsAllowed {
            count: entries.len(),
        }
        .publish(&env);
        Ok(())
    }

    /// The digest an attester signs for one leg of a request to this mandate.
    pub fn digest(
        env: Env,
        recipient_ref: BytesN<32>,
        destination: MuxedAddress,
        valid_until: u64,
    ) -> Result<BytesN<32>, Error> {
        let cfg = load_config(&env)?;
        Ok(keccak_digest(
            &env,
            &recipient_ref,
            &cfg.country_policy_hash,
            &destination,
            valid_until,
        )?
        .to_bytes())
    }

    pub fn request_payouts(
        env: Env,
        caller: Address,
        reqs: Vec<PayoutRequest>,
    ) -> Result<Vec<ItemResult>, Error> {
        caller.require_auth();
        let cfg = load_config(&env)?;
        if !cfg.agents.contains(&caller) {
            return Err(Error::Unauthorized);
        }
        let now = env.ledger().timestamp();
        if now >= cfg.expires_at {
            return Err(Error::MandateExpired);
        }
        if reqs.is_empty() || reqs.len() > PROBE_MAX_BATCH {
            return Err(Error::BatchInvalid);
        }
        let mut counters = load_counters(&env)?;
        // One balance read per call; every item is judged against it plus
        // what earlier items in the batch reserved.
        let balance = TokenClient::new(&env, &cfg.asset).balance(&env.current_contract_address());

        let mut results = Vec::new(&env);
        for req in reqs.iter() {
            let (code, status) =
                match request_one(&env, &cfg, &mut counters, balance, now, &caller, &req) {
                    Ok(status) => (0, status),
                    Err(e) => {
                        PayoutRequestRejected {
                            payout_id: req.payout_id.clone(),
                            agent: caller.clone(),
                            code: e as u32,
                        }
                        .publish(&env);
                        (e as u32, 0)
                    }
                };
            results.push_back(ItemResult {
                payout_id: req.payout_id,
                code,
                status,
            });
        }
        save_counters(&env, &counters);
        Ok(results)
    }

    pub fn execute_batch(env: Env, caller: Address, ids: Vec<BytesN<32>>) -> Result<(), Error> {
        caller.require_auth();
        let cfg = load_config(&env)?;
        if caller != cfg.principal && !cfg.agents.contains(&caller) {
            return Err(Error::Unauthorized);
        }
        let now = env.ledger().timestamp();
        if now >= cfg.expires_at {
            return Err(Error::MandateExpired);
        }
        if ids.is_empty() || ids.len() > PROBE_MAX_BATCH {
            return Err(Error::BatchInvalid);
        }
        let mut counters = load_counters(&env)?;
        let mandate = env.current_contract_address();
        let token = cfg.asset.clone();
        let mut balance = TokenClient::new(&env, &token).balance(&mandate);
        let today = now / SECONDS_PER_DAY;
        let store = env.storage().persistent();

        for id in ids.iter() {
            let key = Key::Payout(id.clone());
            let p = match store.get::<Key, PayoutEntry>(&key) {
                None => return Err(Error::PayoutNotFound),
                Some(PayoutEntry::Closed(_)) => return Err(Error::PayoutWrongStatus),
                Some(PayoutEntry::Open(p)) => p,
            };
            if p.status != STATUS_APPROVED {
                return Err(Error::PayoutWrongStatus);
            }
            if now >= p.expires_at {
                return Err(Error::PayoutLapsed);
            }
            let charged = p.total.checked_add(p.fee).ok_or(Error::InternalInvariant)?;
            if charged > cfg.per_payment_cap {
                return Err(Error::PaymentCapExceeded);
            }
            if p.fee > fee_cap(p.total, cfg.max_fee_bps)? {
                return Err(Error::FeeExceedsCap);
            }
            if cfg.recipient_mode != MODE_ATTESTED {
                for leg in p.legs.iter() {
                    if !is_allowed(&env, &p.recipient_ref, &leg.destination) {
                        return Err(Error::RecipientNotAllowed);
                    }
                }
            }
            if cfg.recipient_mode != MODE_ALLOWLIST && p.policy_epoch != counters.policy_epoch {
                return Err(Error::PolicyChanged);
            }
            let used = used_today(&env, &p.recipient_ref, today);
            let used_after = used.checked_add(p.total).ok_or(Error::InternalInvariant)?;
            if used_after > cfg.per_recipient_daily_cap {
                return Err(Error::DailyCapExceeded);
            }
            if charged > balance {
                return Err(Error::InsufficientFunds);
            }

            // Checks, effects, interactions.
            store.set(&key, &PayoutEntry::Closed(STATUS_EXECUTED));
            counters.committed = counters
                .committed
                .checked_sub(charged)
                .ok_or(Error::InternalInvariant)?;
            counters.spent = counters
                .spent
                .checked_add(charged)
                .ok_or(Error::InternalInvariant)?;
            let daily_key = Key::Daily(p.recipient_ref.clone());
            store.set(
                &daily_key,
                &Daily {
                    day: today,
                    used: used_after,
                },
            );
            store.extend_ttl(&daily_key, DAILY_TTL, DAILY_TTL);
            balance = balance
                .checked_sub(charged)
                .ok_or(Error::InternalInvariant)?;
            PayoutExecuted {
                payout_id: id.clone(),
                recipient_ref: p.recipient_ref.clone(),
                total: p.total,
                fee: p.fee,
                legs: p.legs.clone(),
                day: today,
                spent: counters.spent,
            }
            .publish(&env);

            for leg in p.legs.iter() {
                try_transfer(&env, &token, &mandate, &leg.destination, leg.amount)?;
            }
            if p.fee > 0 {
                try_transfer(
                    &env,
                    &token,
                    &mandate,
                    &MuxedAddress::from(cfg.fee_address.clone()),
                    p.fee,
                )?;
            }
        }
        save_counters(&env, &counters);
        Ok(())
    }

    pub fn payout(env: Env, payout_id: BytesN<32>) -> Option<PayoutEntry> {
        env.storage().persistent().get(&Key::Payout(payout_id))
    }

    pub fn counters(env: Env) -> Result<Counters, Error> {
        load_counters(&env)
    }
}

fn load_config(env: &Env) -> Result<Config, Error> {
    env.storage()
        .instance()
        .get(&Key::Config)
        .ok_or(Error::InternalInvariant)
}

fn load_counters(env: &Env) -> Result<Counters, Error> {
    env.storage()
        .instance()
        .get(&Key::Counters)
        .ok_or(Error::InternalInvariant)
}

fn save_counters(env: &Env, counters: &Counters) {
    let store = env.storage().instance();
    store.set(&Key::Counters, counters);
    store.extend_ttl(INSTANCE_BUMP_AT, INSTANCE_TTL);
}

fn used_today(env: &Env, recipient_ref: &BytesN<32>, today: u64) -> i128 {
    match env
        .storage()
        .persistent()
        .get::<Key, Daily>(&Key::Daily(recipient_ref.clone()))
    {
        Some(d) if d.day == today => d.used,
        _ => 0,
    }
}

fn is_allowed(env: &Env, recipient_ref: &BytesN<32>, destination: &MuxedAddress) -> bool {
    env.storage().persistent().has(&Key::Allow(AllowKey {
        recipient_ref: recipient_ref.clone(),
        address: destination.address(),
        tag: destination.id(),
    }))
}

/// `floor(total * bps / 10000)` without forming `total * bps`, which can
/// overflow i128 for amounts near the domain limit.
fn fee_cap(total: i128, bps: u32) -> Result<i128, Error> {
    let bps = i128::from(bps);
    let whole = (total / BPS_DENOMINATOR)
        .checked_mul(bps)
        .ok_or(Error::InternalInvariant)?;
    let part = (total % BPS_DENOMINATOR)
        .checked_mul(bps)
        .ok_or(Error::InternalInvariant)?
        / BPS_DENOMINATOR;
    whole.checked_add(part).ok_or(Error::InternalInvariant)
}

/// Steps 3 to 12 of a single request, then its effects. Nothing is written
/// unless every check passes, so a rejected item leaves no trace but its event.
fn request_one(
    env: &Env,
    cfg: &Config,
    counters: &mut Counters,
    balance: i128,
    now: u64,
    agent: &Address,
    req: &PayoutRequest,
) -> Result<u32, Error> {
    let store = env.storage().persistent();
    let key = Key::Payout(req.payout_id.clone());
    if store.has(&key) {
        return Err(Error::PayoutIdExists);
    }
    if now >= req.deadline {
        return Err(Error::RequestDeadlinePassed);
    }
    if req.total < 1 || req.fee < 0 || req.fee > I128_MAX - req.total {
        return Err(Error::AmountInvalid);
    }
    let n = req.legs.len();
    if !(1..=MAX_LEGS).contains(&n) {
        return Err(Error::LegsInvalid);
    }
    let mut sum: i128 = 0;
    for (i, leg) in req.legs.iter().enumerate() {
        if leg.amount < 1 {
            return Err(Error::LegsInvalid);
        }
        for other in req.legs.iter().skip(i + 1) {
            if other.destination.address() == leg.destination.address()
                && other.destination.id() == leg.destination.id()
            {
                return Err(Error::LegsInvalid);
            }
        }
        sum = sum.checked_add(leg.amount).ok_or(Error::LegsSumMismatch)?;
    }
    if sum != req.total {
        return Err(Error::LegsSumMismatch);
    }

    match cfg.recipient_mode {
        MODE_ALLOWLIST => {
            if !req.attestations.is_empty() {
                return Err(Error::AttestationInvalid);
            }
            for leg in req.legs.iter() {
                if !is_allowed(env, &req.recipient_ref, &leg.destination) {
                    return Err(Error::RecipientNotAllowed);
                }
            }
        }
        _ => {
            if req.attestations.len() != n {
                return Err(Error::AttestationInvalid);
            }
            for (leg, att) in req.legs.iter().zip(req.attestations.iter()) {
                if cfg.recipient_mode == MODE_BOTH
                    && !is_allowed(env, &req.recipient_ref, &leg.destination)
                {
                    return Err(Error::RecipientNotAllowed);
                }
                verify_attestation(env, cfg, &req.recipient_ref, &leg.destination, &att, now)?;
            }
        }
    }

    if req.fee > fee_cap(req.total, cfg.max_fee_bps)? {
        return Err(Error::FeeExceedsCap);
    }
    let charged = req.total.checked_add(req.fee).ok_or(Error::AmountInvalid)?;
    if charged > cfg.per_payment_cap {
        return Err(Error::PaymentCapExceeded);
    }
    let reserved = counters
        .spent
        .checked_add(counters.pending)
        .and_then(|v| v.checked_add(counters.committed))
        .ok_or(Error::InternalInvariant)?;
    if charged
        > cfg
            .total_budget
            .checked_sub(reserved)
            .ok_or(Error::InternalInvariant)?
    {
        return Err(Error::BudgetExceeded);
    }
    let used = used_today(env, &req.recipient_ref, now / SECONDS_PER_DAY);
    if req.total
        > cfg
            .per_recipient_daily_cap
            .checked_sub(used)
            .ok_or(Error::InternalInvariant)?
    {
        return Err(Error::DailyCapExceeded);
    }
    let held = counters
        .pending
        .checked_add(counters.committed)
        .ok_or(Error::InternalInvariant)?;
    let free = if balance > held { balance - held } else { 0 };
    if charged > free {
        return Err(Error::InsufficientFunds);
    }

    let status = if charged >= cfg.approval_threshold {
        counters.pending = counters
            .pending
            .checked_add(charged)
            .ok_or(Error::InternalInvariant)?;
        STATUS_PENDING
    } else {
        counters.committed = counters
            .committed
            .checked_add(charged)
            .ok_or(Error::InternalInvariant)?;
        STATUS_APPROVED
    };
    let expires_at = now
        .checked_add(cfg.payout_ttl_seconds)
        .ok_or(Error::InternalInvariant)?;
    store.set(
        &key,
        &PayoutEntry::Open(Payout {
            agent: agent.clone(),
            recipient_ref: req.recipient_ref.clone(),
            total: req.total,
            fee: req.fee,
            legs: req.legs.clone(),
            status,
            created_at: now,
            expires_at,
            policy_epoch: counters.policy_epoch,
            approvals: Vec::new(env),
        }),
    );
    store.extend_ttl(&key, RECORD_TTL, RECORD_TTL);
    PayoutRequested {
        payout_id: req.payout_id.clone(),
        agent: agent.clone(),
        recipient_ref: req.recipient_ref.clone(),
        total: req.total,
        fee: req.fee,
        legs: req.legs.clone(),
        task_ref: req.task_ref.clone(),
        status,
        expires_at,
    }
    .publish(env);
    Ok(status)
}

/// The prechecks run before recovery because the host traps, instead of
/// returning, on high s, out-of-range scalars and recovery ids above 3.
fn verify_attestation(
    env: &Env,
    cfg: &Config,
    recipient_ref: &BytesN<32>,
    destination: &MuxedAddress,
    att: &Attestation,
    now: u64,
) -> Result<(), Error> {
    if now >= att.valid_until {
        return Err(Error::AttestationExpired);
    }
    // valid_until > now here, so this cannot wrap and there is no now + MAX.
    if att.valid_until - now > MAX_ATTESTATION_VALIDITY {
        return Err(Error::AttestationInvalid);
    }
    if att.signature.len() != 65 {
        return Err(Error::AttestationInvalid);
    }
    let v = att.signature.get(64).ok_or(Error::AttestationInvalid)?;
    if v != 27 && v != 28 {
        return Err(Error::AttestationInvalid);
    }
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    att.signature.slice(0..32).copy_into_slice(&mut r);
    att.signature.slice(32..64).copy_into_slice(&mut s);
    if r == ZERO || r >= ORDER || s == ZERO || s > HALF_ORDER {
        return Err(Error::AttestationInvalid);
    }
    let mut rs = [0u8; 64];
    rs[..32].copy_from_slice(&r);
    rs[32..].copy_from_slice(&s);
    let digest = keccak_digest(
        env,
        recipient_ref,
        &cfg.country_policy_hash,
        destination,
        att.valid_until,
    )?;
    let recovered =
        env.crypto()
            .secp256k1_recover(&digest, &BytesN::from_array(env, &rs), u32::from(v - 27));
    if recovered != cfg.attester {
        return Err(Error::AttestationInvalid);
    }
    Ok(())
}

fn keccak_digest(
    env: &Env,
    recipient_ref: &BytesN<32>,
    policy: &BytesN<32>,
    destination: &MuxedAddress,
    valid_until: u64,
) -> Result<soroban_sdk::crypto::Hash<32>, Error> {
    let mandate = env.current_contract_address().to_xdr(env);
    let dest = destination.address().to_xdr(env);
    let (has_tag, tag) = match destination.id() {
        Some(id) => (1u8, id),
        None => (0u8, 0u64),
    };
    let mut b = Bytes::from_array(env, DOMAIN);
    b.append(&env.ledger().network_id().into());
    b.append(&recipient_ref.clone().into());
    b.append(&policy.clone().into());
    b.extend_from_array(&valid_until.to_be_bytes());
    b.push_back(u8::try_from(mandate.len()).map_err(|_| Error::InternalInvariant)?);
    b.append(&mandate);
    b.push_back(u8::try_from(dest.len()).map_err(|_| Error::InternalInvariant)?);
    b.append(&dest);
    b.push_back(has_tag);
    b.extend_from_array(&tag.to_be_bytes());
    Ok(env.crypto().keccak256(&b))
}

/// Any error the token returns, of any type, is TRANSFER_FAILED, and
/// returning it reverts the whole call, legs already paid included.
fn try_transfer(
    env: &Env,
    token: &Address,
    from: &Address,
    to: &MuxedAddress,
    amount: i128,
) -> Result<(), Error> {
    let args: Vec<Val> = (from.clone(), to.clone(), amount).into_val(env);
    match env.try_invoke_contract::<(), soroban_sdk::Error>(
        token,
        &Symbol::new(env, "transfer"),
        args,
    ) {
        Ok(Ok(())) => Ok(()),
        _ => Err(Error::TransferFailed),
    }
}
