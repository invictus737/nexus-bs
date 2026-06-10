# Nexus-BS vs FlowStation Function Delta Report

Report date: 2026-06-09 09:23:53 EEST

## Compared Releases

Primary baseline:

- Nexus-BS: `v0.1.61-1-g4ab64b2`, commit `4ab64b2`
- FlowStation: `v0.2.7`, commit `c2f0ee6`

Secondary reference:

- Nexus-BS: `v0.1.61-1-g4ab64b2`, commit `4ab64b2`
- FlowStation: `v0.3.0`, commit `fcac34e`

This report is an engineering history record. It is not a formal TETRA/ETSI
certification statement.

## Method

Rust functions were compared by:

- relative path
- function name
- ordinal for repeated same-name functions in the same file
- normalized function body hash

The scope is `crates/tetra-*`. Production counts use `src/**/*.rs`; tests use
`tests/**/*.rs`.

Moved or renamed functions may appear as added/removed rather than modified.
The numbers are therefore a practical code-delta metric, not a semantic proof
of protocol coverage.

## Primary Baseline: FlowStation v0.2.7

### Production Rust Functions

| Scope | FlowStation | Nexus-BS | Modified | Added | Removed |
|---|---:|---:|---:|---:|---:|
| All `tetra-*` crates | 1867 | 3303 | 516 / 27.6% | 1444 / 77.3% | 8 / 0.4% |
| TETRA protocol core only | 1596 | 2853 | 436 / 27.3% | 1265 / 79.3% | 8 / 0.5% |
| Tests | 50 | 1168 | 19 | 1122 | 4 |

### TETRA Component Breakdown

| Component | Modified | Added |
|---|---:|---:|
| UMAC/MAC scheduler | 66 | 269 |
| CMCE call control | 84 | 171 |
| CMCE/SDS PDUs | 41 | 234 |
| UMAC/MAC PDUs | 32 | 114 |
| MM attach / EE / affiliation | 28 | 106 |
| MM PDUs / IEs | 35 | 97 |
| LLC timers / ACKs | 14 | 79 |
| Brew/IP gateway | 30 | 44 |
| SDS service | 16 | 32 |
| MLE broadcast / network time | 22 | 23 |
| MLE PDUs | 20 | 31 |
| LMAC / burst codec | 18 | 8 |
| PHY / RF IO | 16 | 8 |
| SNDCP / WAP bearer | 1 | 4 |
| Parrot private-call service | 0 | 15 |

### Largest Areas Of Change

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`: MAC/UMAC scheduling,
  grants, STCH/TCH/S handling.
- `crates/tetra-entities/src/mm/mm_bs.rs`: attach, restart recovery, group
  affiliation, energy economy.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/*`: group/private call
  control, release handling, timers, Parrot.
- `crates/tetra-entities/src/llc/llc_bs_ms.rs`: ACK handling, retransmission
  behavior, duplicate guards.
- `crates/tetra-entities/src/cmce/subentities/sds_bs.rs` and SDS PDUs: SDS,
  WAP, Home Mode Display.
- `crates/tetra-entities/src/net_brew/*`: Brew/IP gateway integration.

## Secondary Reference: FlowStation v0.3.0

### Production Rust Functions

| Scope | FlowStation | Nexus-BS | Modified | Added | Removed |
|---|---:|---:|---:|---:|---:|
| All `tetra-*` crates | 1891 | 3303 | 537 / 28.4% | 1445 / 76.4% | 33 / 1.7% |
| TETRA protocol core only | 1609 | 2853 | 457 / 28.4% | 1266 / 78.7% | 22 / 1.4% |
| Tests | 65 | 1168 | 21 | 1120 | 17 |

The v0.3.0 comparison is included only as a later-upstream reference. The
primary historical baseline for this Nexus-BS fork report remains
FlowStation `v0.2.7`.
