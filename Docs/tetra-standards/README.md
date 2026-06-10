# TETRA Standards Local Text Cache

This folder is for fast local consultation of ETSI TETRA standards while
working on Nexus-BS.

Full ETSI deliverables are copyrighted. The repo keeps only this manifest and
fetch script. Generated `.txt` files live under `cache/` and are intentionally
ignored by git.

Generate or refresh the local text cache:

```sh
Docs/tetra-standards/fetch_etsi_text.sh
```

Useful generated files:

- `cache/ts_10039202v031001p.txt` - TETRA V+D Part 2 Air Interface
- `cache/en_3003921203v010301p.txt` - SS-TPI supplementary service
- `cache/en_30039207v030501p.txt` - Security
- `cache/ts_10039215v010401p.txt` - Frequency bands and channel numbering
- `cache/en_30039201v010201p.txt` - General network design

Before protocol/RF/CMCE/UMAC/MM/SDS changes, cite the exact standard and clause
scope in code comments/tests/timeline entries. Do not claim formal conformance
without official conformance evidence.
