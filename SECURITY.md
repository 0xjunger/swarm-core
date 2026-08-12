# Security policy

## Status

Swarm Core is a research prototype. It is not audited. Do not use it in a
production system.

## Scope

This policy covers the code in this repository: the `swarm-core`,
`swarm-sim`, and `swarm-verify` crates, and the specification in
`SPEC.md`. A security concern in one of these is in scope, for example:

- A way to make `swarm-verify::verify` report `Satisfied` on a bundle that
  contains a real violation.
- A way to make two different byte strings decode to the same value (a
  break of canonical encoding, `SPEC.md` §5.1).
- A way to forge a signature, a proof of equivocation, or a chain link
  without the corresponding private key.
- A panic, an infinite loop, or unbounded memory growth reachable from
  untrusted input to `swarm-verify`.

Out of scope: anything about hardware, flight, or network transport — none
of that exists in this repository yet (`README.md`, Status).

## Reporting a problem

Report a security concern to `<CONTACT>`. Include:

- A description of the problem.
- The steps to reproduce it, or a minimal example.
- The invariant or guarantee it breaks, if you know which one.

Do not open a public issue for a security concern until the project has
had a chance to review it.
