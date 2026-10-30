# Fixture spec for the vector coverage tests

A small spec with three live MANDATE rules, one retired rule and two CHAINOPS
rules, so coverage can be tested without the full spec.

#### MANDATE-FEE-003 Fee within the cap

- Precondition: a fee above the cap.
- Effect: refused.
- Error: `FEE_EXCEEDS_CAP`

#### MANDATE-PAUSE-001 Paused mandates refuse new work

- Precondition: a call while paused.
- Effect: refused.
- Error: `MANDATE_PAUSED`

#### MANDATE-PAUSE-002 Pause

- Precondition: pause while paused.
- Effect: refused.
- Error: `ALREADY_PAUSED`

#### MANDATE-RECIPIENT-011 Retired: low s

- Precondition: none.
- Effect: retired; the ID is never reused.
- Error: none

#### CHAINOPS-FEE-001 The fee is shown and reconciled

- Off chain: every receipt shows the fee.

#### CHAINOPS-SWEEP-001 Every sweep is booked once

- Off chain: every sweep is booked once.
