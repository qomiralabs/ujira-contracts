# Fixture spec for the precedence tests

A small spec with two check orders. Every adjacent pair of `pause` needs a
precedence vector. The last three pairs of `remove_role` can never fail
together, and the checker exempts them, so only its first two pairs need one.

#### MANDATE-APPROVAL-006 Approver removal keeps the gate working

- Precondition: removing one approver among several would leave too few.
- Effect: refused.
- Error: `CONFIG_INVALID`

#### MANDATE-APPROVAL-008 Removing the last approver

- Precondition: removing the only approver while approvals could be needed.
- Effect: refused.
- Error: `CONFIG_INVALID`

#### MANDATE-PAUSE-002 Pause

- Precondition: pause while already paused.
- Effect: refused.
- Error: `ALREADY_PAUSED`

#### MANDATE-RECOVERYSET-002 Threshold bounds

- Precondition: removing a recovery approver would leave too few for the threshold.
- Effect: refused.
- Error: `CONFIG_INVALID`

#### MANDATE-REVOKE-002 Revoked mandates refuse state changes

- Precondition: a call on a revoked mandate.
- Effect: refused.
- Error: `MANDATE_REVOKED`

#### MANDATE-ROLE-014 Who may pause

- Precondition: pause from a caller that may not pause.
- Effect: refused.
- Error: `UNAUTHORIZED`

#### MANDATE-ROLE-015 Principal-only calls

- Precondition: a principal-only call from any other caller.
- Effect: refused.
- Error: `UNAUTHORIZED` (Celo); host authorization failure (Stellar)

#### MANDATE-ROLE-016 Role change validation

- Precondition: an unknown role code, or an account not in the set.
- Effect: refused.
- Error: `CONFIG_INVALID`

#### MANDATE-ROLE-019 Every caller argument is authorized first

- Precondition: a call that takes a caller address.
- Effect: the caller is authorized before anything else.
- Error: host authorization failure (Stellar); not reachable on Celo

## Operations and check orders

### pause

<!-- check-order: pause -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller may pause (MANDATE-ROLE-014).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The mandate is not already paused (MANDATE-PAUSE-002).
<!-- /check-order -->

### remove_role

<!-- check-order: remove_role -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The role code is known and the account is in the set (MANDATE-ROLE-016).
4. Removing one approver among several keeps the gate working (MANDATE-APPROVAL-006).
5. Removing the last approver needs the threshold above the cap (MANDATE-APPROVAL-008).
6. Removing one recovery approver keeps the threshold reachable (MANDATE-RECOVERYSET-002).
<!-- /check-order -->
