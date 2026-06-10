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
- Inspect `git status --short` before edits and never revert user changes unless
  explicitly requested.

Current runtime checkpoint:

- Last deployed runtime commit is `74ce228` on branch `nexus-bs-v0.1.55`.
- Runtime version `v0.1.62-74ce2282` deployed to `chris@192.168.1.179`.
- Latest work was dashboard-only: fixed live scroll fighting, moved System
  Timeslots above Host/Carrier Plan, removed Traffic Call Control/Activity Log,
  consolidated Last Heard voice/SDS, and resolved Parrot `99999` locally.
- A later commit may exist only to add this Codex bootstrap file; do not treat
  that docs-only commit as a runtime deploy.
