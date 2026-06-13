# Nexus-BS Provenance

This is a practical provenance and license map for the Nexus-BS repository. It is
not formal legal advice, not a complete file-by-file copyright audit, and not a
substitute for review by counsel before commercial redistribution or other
high-risk use.

The detailed path map is in `LICENSE-MAP.tsv`. This document explains how to use
that map and records the repository context used to classify major path groups.

## Repository Licensing Model

Nexus-BS is published as source-available software for permitted noncommercial
use under the PolyForm Noncommercial License 1.0.0, with commercial use by
separate written agreement.

Upstream portions that were originally provided under Apache-2.0 retain their
upstream Apache-2.0 rights, notices, and attribution requirements. The Nexus-BS
licensing text does not remove or narrow rights granted directly by upstream
copyright holders under their original licenses.

Nexus-BS modifications, additions, packaging, dashboard work, deployment work,
operational logic, generated project assets, and the current Nexus-BS project
form are PolyForm-Noncommercial-1.0.0 unless file-level notices state otherwise.

In `LICENSE-MAP.tsv`, the expression `Apache-2.0 AND
PolyForm-Noncommercial-1.0.0` means the path is treated as mixed lineage:
preserve the Apache-2.0 position for inherited upstream portions, and apply the
Nexus-BS PolyForm Noncommercial terms to Nexus-BS modifications and additions.
It is a practical repository classification, not a legal conclusion for every
line of code.

## Credited Upstream Context

The current README and NOTICE credit the following upstream, historical, and
community sources. URLs are listed where they are already present in repository
README/dashboard context.

- BlueStation Project: https://github.com/MidnightBlueLabs/tetra-bluestation
- FlowStation Project: https://github.com/razvanzeces/flowstation
- Mihajlo YU4MSH and the misadeks/tetra-bluestation fork:
  https://github.com/misadeks/tetra-bluestation
- Harald Welte and the osmocom team for foundational osmocom-tetra work. The
  README/NOTICE credit this project, but do not record a URL.
- Tatu Peltola for his SXCEIVER project: https://sxceiver.com/
- Stichting NLnet for partial funding through the RETETRA3 grant.
- Dennis DB2OE for dashboard-theme inspiration.
- Historical testing, documentation, hardware, dashboard, integration, and
  operator-community contributors, including ON6RF, EA7KEN, BU2GQ, DK5RTA,
  DO5MF, ES4TIX, and others.

## Path Classification Rules

Use the most specific applicable row in `LICENSE-MAP.tsv`. If more than one row
applies, preserve all relevant notices and apply the stricter or more specific
handling until a file-level review says otherwise.

- `upstream-modified Apache lineage`: code or assets with BlueStation,
  FlowStation, osmocom-tetra, SXCEIVER/SDR ecosystem, or related historical
  lineage. Preserve Apache-2.0 notices and add/retain Nexus-BS change context.
- `Nexus-original PolyForm`: files treated as current Nexus-BS additions based on
  available repository context. License as PolyForm-Noncommercial-1.0.0 unless a
  file-level notice states otherwise.
- `generated/binary`: compiled outputs, release bundle copies, packages, caches,
  lockfiles, or local metadata. Regenerate from source where possible and do not
  infer source licensing from the generated artifact alone.
- `docs/package assets`: documentation, package templates, service units, config
  examples, release metadata, and project assets. Most are Nexus-BS PolyForm
  material, but some carry upstream context or generated/cache caveats.
- `needs-review`: available repository context is not enough to make a confident
  practical classification. Do not reuse externally without additional review.

## High-Level Provenance Summary

The `crates/tetra-*` protocol stack carries historical BlueStation/FlowStation
lineage and extensive Nexus-BS modification. The public README summarizes the
function-level comparison used for this release, including large changes in
UMAC/MAC, CMCE, MM, LLC, SDS, MLE, LMAC/PHY, Brew integration, and tests.
Treat those paths as mixed Apache-2.0 upstream lineage plus Nexus-BS PolyForm
modifications unless a more specific row or file-level notice applies.

The dashboard and service/deployment surfaces retain historical dashboard and
deployment lineage while also containing substantial Nexus-BS UI, operational,
control, telemetry, packaging, and release work. Preserve the README/NOTICE
credits when redistributing those parts.

The `Docs/tetra-standards/cache/` text files are generated local text caches
from ETSI standards source URLs listed in `Docs/tetra-standards/standards.tsv`.
They are not Nexus-BS code. Do not treat ETSI standard text as relicensed under
PolyForm or Apache by this repository.

Compiled binaries, Debian packages, and `compiled_distribution/` artifacts are
release outputs derived from Nexus-BS source plus third-party Rust and system
dependencies. Their exact redistributable licensing posture requires dependency
license review in addition to this repository map.

## Change Notice Policy

For upstream-derived paths:

- preserve `NOTICE`, the upstream/historical credit wording, and
  `LICENSES/Apache-2.0.txt`;
- keep material Nexus-BS changes traceable through git history, release notes,
  tests, or nearby documentation;
- do not remove upstream attribution when refactoring or moving code.

For Nexus-original paths:

- keep the repository-level PolyForm Noncommercial notice path intact;
- add file-level notices only when a file intentionally differs from the
  repository default;
- update `LICENSE-MAP.tsv` when adding a new major directory, generated artifact,
  vendored third-party file, or binary/package output.

For generated or binary paths:

- prefer regenerating from source instead of hand-editing;
- keep checksums/source metadata where present;
- run a dependency/license review before public binary redistribution.
