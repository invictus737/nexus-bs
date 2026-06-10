# Nexus-BS Codex Bootstrap

When a Codex session starts in this repo, immediately load these memories before
doing work:

- `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`
- `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`
- `/Users/ctermure/.codex/memories/flowstation-aarch64-soapysdr-build.md`
- `/Users/ctermure/.codex/memories/nexus-bs-resume-2026-06-10.md`

Project laws:

- Build locally only. Never compile Rust/TETRA/Nexus-BS on
  `chris@192.168.1.179`.
- Use `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh` for
  fast field deploys unless the user asks for a fuller test run.
- Do not claim formal ETSI/TETRA certification without official conformance
  evidence.
- Before any protocol/RF/CMCE/UMAC/MM/SDS/WAP/parrot behaviour change, identify
  the relevant ETSI EN 300 392-2 clause scope and keep changes test-backed.
- Local ETSI/TETRA text cache lives in
  `Docs/tetra-standards/cache/`; refresh it with
  `Docs/tetra-standards/fetch_etsi_text.sh` if missing. Use this before
  web-searching for TS/EN 300 392 clause text.
- Inspect `git status --short` before edits and never revert user changes unless
  explicitly requested.

Current runtime checkpoint:

- Rollback checkpoint explicitly requested by user: commit `80b8d1e`
  (`fix: suppress unsupported private preemptive release cause`).
- Runtime version `v0.1.62-80b8d1eb` deployed to `chris@192.168.1.179`.
- If private P2P pre-emption work regresses service badly, use `80b8d1e` as the
  known rollback base before continuing analysis.
