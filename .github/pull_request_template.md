<!-- markdownlint-disable-file MD041 -->

## Linked issue

Closes #

## What and why

What changes, and why it is needed.

## Tests

The commands you ran and their results. List every acceptance criterion with the command that proves it.

## Docs

What docs changed, or "no docs change" and why.

## Checklist

- [ ] Tests: written before or with the code, run, and passing, with the output above
- [ ] Docs: updated, or no docs change with the reason above
- [ ] No em or en dashes anywhere in the change, the commits or this description
- [ ] Plain ASCII only in this description
- [ ] No secrets, keys or tokens in code, history, logs or this description
- [ ] Zero warnings from the compiler, linters and tests
- [ ] Money in integer minor units with an explicit currency, never a float
- [ ] A bug fix starts with a failing test that reproduces it
- [ ] Every review this change type needs (below) is requested

## Required reviews

- Every change: code review, QA verification and a hygiene review.
- Auth, identity, payouts, idempotency, router, secrets, dependencies, CI and infrastructure: add a security review.
- Contracts: add an exploit review and an audit review.
- Dashboard, website, embed and `packages/ui`: add a design review.

## Before merge

- Any new commit after an approval needs review again.
- Merges are squash only.
