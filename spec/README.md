# spec

The mandate rules, written once and independent of any chain or language, plus the JSON test vectors that both contracts must pass.

A mandate is the set of limits a principal puts on an agent key: total budget, per-payment cap, per-recipient daily cap, allowed recipients or countries, approval threshold and expiry, along with the principal's ability to pause or revoke it at any time.

What belongs here:

- The written specification of each rule and how the rules combine.
- Test vectors in JSON: a starting mandate state, a sequence of actions and the expected outcome of each one.

The contracts in `soroban/` and `celo/` are implementations of this spec. When the spec and a contract disagree, the contract is wrong.
