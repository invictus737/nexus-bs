<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
SPDX-FileComment: See CHANGES-NEXUS.md for the central Nexus-BS change notice.
-->

# License Overview

This repository is a mixed-license, source-available project. This overview is
provided to make the repository's intended licensing structure easier to audit.
It is not legal advice.

## Short Version

- Upstream portions that were received under Apache-2.0 remain Apache-2.0.
  The Apache-2.0 text is preserved in `LICENSES/Apache-2.0.txt`.
- Nexus-BS modifications and additions are licensed by Chris YO3TCO /
  Nexus-BS Project under the PolyForm Noncommercial License 1.0.0 unless a
  file-level notice states otherwise. The PolyForm text is available in
  `LICENSE` and `LICENSES/PolyForm-Noncommercial-1.0.0.txt`.
- Some files may contain both upstream Apache-2.0 material and Nexus-BS-covered
  modifications. Follow the file-level SPDX notices, copyright notices, and
  this overview together.
- Commercial licensing applies only to Nexus-BS-covered material, and only to
  the extent Chris YO3TCO / Nexus-BS Project has the right to license that
  material. It does not remove, replace, or narrow rights granted directly by
  upstream copyright holders.

## What Is Upstream Material?

"Upstream material" means material inherited from the historical BlueStation,
FlowStation, osmocom-tetra, SXCEIVER/SDR ecosystem, and other credited upstream
lineages, dependencies, examples, or references, to the extent that material was
provided under Apache-2.0 or another upstream license notice.

Upstream Apache-2.0 portions remain under Apache-2.0. Any Apache-2.0 copyright,
notice, attribution, patent, redistribution, and disclaimer terms continue to
apply to those portions.

## What Is Nexus-BS-Covered Material?

"Nexus-BS-covered material" means modifications, additions, integration work,
packaging, dashboard work, deployment material, operational logic,
documentation, and other material added by Chris YO3TCO / Nexus-BS Project,
unless a file-level notice says otherwise.

Unless a file-level notice says otherwise, Nexus-BS-covered material is offered
for permitted noncommercial use under the PolyForm Noncommercial License
1.0.0. Commercial use of Nexus-BS-covered material requires a separate written
commercial license agreement before that use begins.

## File-Level Notices

File-level SPDX identifiers and copyright notices are the first place to look
for the license expression for a file. A file may use:

- `Apache-2.0` for upstream Apache-2.0 material;
- `PolyForm-Noncommercial-1.0.0` for Nexus-BS-covered material;
- an expression such as `Apache-2.0 AND PolyForm-Noncommercial-1.0.0` where
  upstream material and Nexus-BS modifications are both present; or
- another expression if a file-level notice states a different applicable
  license.

Per-file SPDX comments may reference `CHANGES-NEXUS.md` as the central
Nexus-BS change notice rather than repeating a long change summary in every
file.

## Notices To Preserve

Redistribution should preserve:

- the `Required Notice:` line in `NOTICE`;
- `NOTICE`;
- `CHANGES-NEXUS.md`;
- this overview;
- `LICENSES/Apache-2.0.txt`;
- `LICENSES/PolyForm-Noncommercial-1.0.0.txt`; and
- all applicable file-level SPDX, copyright, attribution, and license notices.

The root `LICENSE` file contains the PolyForm Noncommercial License 1.0.0 text
for Nexus-BS-covered material. It should not be read as a statement that every
file or every historical upstream portion in this repository is simply
PolyForm-licensed.
