# Nexus-BS Project Timeline

## 2026-06-04 10:22:33 EEST - PM orchestration checkpoint

Goal in force: clause-scoped ETSI EN 300 392-2 hardening for a robust Nexus-BS TETRA stack. The target remains practical engineering evidence for group call, private call simplex/duplex, SDS/status, MM attach/affiliation persistence, scan/group retention, WAP MVP, and long-running BS stability. This is not a formal certification claim; formal certification requires official conformance evidence.

Mandatory law reloaded before this checkpoint:

- `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`
- `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`
- `/Users/ctermure/.codex/memories/flowstation-aarch64-soapysdr-build.md`

Repo status:

- Branch: `nexus-bs-v0.1.55`
- Latest commit: `b18ed13 fix: allocate unique MLE handles for CMCE unitdata`
- Current dirty files:
  - `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - `crates/tetra-entities/tests/test_cmce_bs.rs`
  - `crates/tetra-entities/tests/test_umac_bs.rs`

Live critical defects still open:

- Private simplex call between `2260082` and `2260616` grants PTT but one direction produces static.
- Group call has the same static symptom when the other subscriber transmits.
- This points to the shared voice path, not only to private-call CMCE: CMCE floor grant -> UMAC circuit/timeslot routing -> LMAC TCH/S voice encode/decode.
- BS restart/long-run robustness remains a hard requirement: terminals must re-register/re-affiliate, groups must be retained coherently, and scan behavior must not be broken.

Component map in simple technical terms:

- CMCE: call control. It decides setup, floor/PTT permission, release, private/group call state.
- UMAC/MAC: radio scheduler. It maps signalling and voice to timeslots, STCH/FACCH/TCH, grants, and random access responses.
- LMAC: low MAC/physical framing. It decodes and encodes bursts, including TCH/S voice bits.
- MLE/LLC: reliable/unreliable signalling delivery, handles, ACK/report timers.
- MM: registration, attach, group affiliation, energy saving/EG behavior.
- SDS/status: short data and status messages over CMCE/SDS subentities.
- WAP: packet/page delivery track for the Nexus-BS terminal browser MVP.

Agent roster:

- Project Manager: `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`)
  - Owns execution order, timeline updates, anti-loop discipline, and ETSI law reminders.
- Review Agent: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`)
  - Reviews current diffs for regressions, missing tests, and unsupported compliance claims.
- Architecture Agent - voice path: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`)
  - Traces CMCE floor -> UMAC routing -> LMAC TCH/S for the static-audio defect.
- QA Agent: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`)
  - Maintains BASIC 24x7 test matrix: group, private simplex/duplex, SDS, attach, affiliation, scan, restart.
- MAC/UMAC Scheduling Architect: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`)
  - Focuses on TCH/S bit preservation, timeslot routing, FACCH/STCH stealing, and grants.
- MM/SDS Robustness Architect: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`)
  - Focuses on MM persistence, EG risk, group affiliation, SDS/status delivery.

Current WIP:

- CMCE pending group release timer hardening is implemented locally:
  - `check_call_timeout_expiry` skips calls already in `pending_group_releases`.
  - `check_hangtime_expiry` skips calls already in `pending_group_releases`.
  - Test extension asserts no repeated `D-RELEASE` and no early `NetworkCallEnd` while reporter completion is pending.
  - ETSI anchor: EN 300 392-2 clause 14.5.2.3 group call release. The patch prevents an internal retry storm; it does not change the standardized group release primitive.
- UMAC voice tests are partially prepared:
  - Helpers exist for private-call circuit opening, UL voice injection, ACELP test bits, and DL TCH/S bit collection.
  - The actual group/private voice-routing tests still need to be written before claiming this path is protected.

Verification already reported for the current CMCE timer WIP:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_cmce_bs network_group_hangtime_release --locked`
- `cargo test -p tetra-entities --test test_cmce_bs --locked`

Do not repeat those as proof for UMAC voice; they only cover CMCE release behavior.

Immediate next execution order:

1. Finish UMAC TCH/S voice-routing tests in `crates/tetra-entities/tests/test_umac_bs.rs`.
   - Group/local path: UL TCH/S bits on the active speaker timeslot must be scheduled back as DL TCH/S without bit corruption.
   - Private simplex cross-route: UL TCH/S from one party must be transmitted on the peer timeslot, not the wrong slot, without bit corruption.
   - Duplex path must be added after simplex routing is stable.
2. If UMAC tests fail, fix UMAC/LMAC routing or ACELP bit packing before any deploy.
3. Run focused local tests only, one Cargo command at a time:
   - `cargo fmt -p tetra-entities`
   - `cargo test -p tetra-entities --test test_umac_bs --locked`
   - `cargo test -p tetra-entities --test test_cmce_bs --locked`
   - `git diff --check`
4. Commit the narrow verified patch.
5. Build locally only with the Nexus-BS AArch64 command from build memory.
6. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs` on `chris@192.168.1.179`; do not compile on the Pi and do not create binary backups.
7. Restart the test BS and collect bounded logs around `2260082`, `2260616`, `2260618`, `U-TX DEMAND`, `D-TX GRANTED`, `TCH`, `UL inactivity`, `TMA-REPORT`, and `D-RELEASE`.

ETSI anchors for the next patches:

- Private simplex/floor: EN 300 392-2 clause 14.5.1.2.1.
- Group floor and calls: EN 300 392-2 clauses 14.5.2.1.3 and 14.5.2.2.1.
- Group release: EN 300 392-2 clause 14.5.2.3.
- MAC traffic/channel allocation/FACCH/STCH/TCH: EN 300 392-2 clauses 23.5, 23.5.4.1, 23.5.4.2.
- LLC/MLE report handles and ACK timing: EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1.
- Energy economy/EG: EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 16.10.35a, 22.3.2.3, 23.5.2.2.7, 23.7.6, and timer T.210.
- SDS/status: EN 300 392-2 clause 13.2 and U/D-STATUS tables 14.27 and 14.14.

Anti-loop rules:

- Every protocol change starts by naming the exact clause scope and the exact runtime symptom or failing test.
- Every hypothesis must end in one of three states: proven by test/log, disproven by test/log, or parked with a concrete missing artifact.
- Do not reopen solved MLE handle ambiguity unless `req_handle=0 ambiguous` reappears after `b18ed13`.
- Do not treat CMCE release tests as proof of voice path correctness.
- Do not deploy a protocol patch that has only dashboard/config tests.
- Do not claim `100% certified` in logs, commits, or user output. Use `clause-scoped ETSI-aligned` until official conformance evidence exists.
- Encryption TODOs are tracked but not the current focus.
- `call_preemptive` / transmission interruption must stay default off unless explicitly enabled in config.
- Energy saving default changes must be checked against the full EG scheduling path; `StayAlive` remains the safest baseline unless real EG is configured and verified.

Open project tracks after the voice blocker:

- Private duplex call setup and traffic tests.
- SDS/status field-level and routing tests.
- MM restart recovery: terminal reattach/re-affiliation and group retention evidence.
- Scan behavior validation with configured groups.
- WAP MVP: terminal browser home page must deliver the Nexus-BS greeting page; dynamic/flashing/color behavior is a UI/application layer feature and must not mask packet delivery failures.
- 24x7 soak harness: bounded log rotation, health metrics, terminal registry checks, call/SDS periodic probes.

## 2026-06-04 10:27:26 EEST - UMAC voice contract tests added

Changed files:

- `crates/tetra-entities/tests/test_umac_bs.rs`
- `timeline.md`

What was added:

- `test_group_ul_voice_loopback_preserves_tch_s_bits`
- `test_private_simplex_ul_voice_loopback_preserves_tch_s_bits`
- `test_private_duplex_ul_voice_cross_route_preserves_tch_s_bits`

Result:

- UMAC preserves the 274 ACELP TCH/S bits in the pure component model.
- Group/local loopback, private simplex same-channel loopback, and private duplex `peer_ts` cross-route all pass.
- The helper now checks both `blk1` and `blk2`, so FACCH/stealing placement does not hide a TCH/S frame in the second half-slot.

Verification:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_umac_bs voice --locked` -> 3 passed
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 115 passed
- `git diff --check`

Updated conclusion for static-audio defect:

- Do not keep looping on the simple UMAC bit-copy hypothesis; it is now covered by tests.
- Next investigation must target the live path:
  - actual CMCE circuit allocation used by radios `2260082` and `2260616`;
  - whether private simplex live call is opened as same-channel loopback or mistakenly as SwMI/media-suppressed path;
  - FACCH/STCH stealing around `D-TX GRANTED` and `D-TX CEASED`;
  - LMAC TCH/S encode/decode with real burst framing;
  - RF/PHY direction-specific decode quality.

## 2026-06-04 10:29:20 EEST - Agent review incorporated

Agent input:

- QA Agent produced a BASIC 24x7 matrix for group call, private simplex, private duplex, SDS/status/WAP, attach/affiliation persistence, scan/group retention, and BS restart recovery.
- Review Agent found one real coverage gap: the CMCE timer patch changed both hangtime and call-timeout paths, but only hangtime duplicate suppression was tested.
- MM/SDS Architect flagged restart recovery as ISSI-cache driven; active GSSI affiliation after restart must be rebuilt from terminal group reports, not assumed from disk as truth.

Action taken:

- Extended `test_network_group_call_timeout_reports_network_end_after_expiry_release_delivery`.
- After timeout-driven `D-RELEASE` enters `pending_group_releases`, two additional timer ticks now assert:
  - no duplicate `D-RELEASE`;
  - no early `NetworkCallEnd`;
  - no early UMAC traffic-circuit close.

Verification after agent review:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_cmce_bs test_network_group_call_timeout_reports_network_end_after_expiry_release_delivery --locked` -> 1 passed
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 115 passed
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed
- `git diff --check`

Next non-repeating execution:

- Commit this narrow batch.
- Build/deploy to the Pi test directory.
- Live-log focus must not repeat pure UMAC bit-copy tests; inspect real call circuit source:
  - whether private simplex is opened with `CircuitDlMediaSource::LocalLoopback`;
  - whether live CMCE sends the expected `CallControl::Open`/`FloorGranted` sequence for both `2260082` and `2260616`;
  - whether LMAC reports valid TCH/S uplink frames after each `D-TX GRANTED`;
  - whether static appears only when FACCH stealing occurs near speech start.

## 2026-06-04 10:34:05 EEST - Built and deployed test BS

Commit deployed:

- `21f2b4c test: track CMCE release timers and UMAC voice routing`

Local build:

- Built locally on macOS only.
- Command used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs` (`11M`).

Remote deploy:

- Host: `chris@192.168.1.179`
- Target path: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Deployed direct over the testing binary; no binary backup was created.
- Started from `/home/chris/nexus-bs-v0.1.55-test` with `bin/nexus-bs config.live.toml`.
- Active process after wrapper cleanup: `15161 bin/nexus-bs config.live.toml`
- Dashboard listens on `0.0.0.0:8080`.

Post-deploy observations:

- Terminals re-registered/re-affiliated shortly after restart:
  - `2260616` registered and affiliated to `[226333]`, RSSI about `-26 dBFS`.
  - `2260082` registered and affiliated to `[226333]`, RSSI about `-26 dBFS`.
  - `2260618` registered and affiliated to `[226333]`, RSSI about `-41 dBFS`, requested `Eg1`, BS allocated `Eg3`.
- Dashboard root returned HTML.
- After `10:31:00` there were no matches for:
  - `req_handle=0 ambiguous`
  - `Hangtime expired`
  - `group release already pending`
  - `D-RELEASE`
  - `UL inactivity`
  - `U-TX DEMAND`
  - `D-TX GRANTED`
  - `TCH`

Interpretation:

- Restart recovery did bring the observed terminals back after the new deploy.
- No post-deploy PTT test has run yet, so the static-audio defect is not proven fixed.
- Next live action: trigger private simplex and group PTT again, then inspect only post-deploy logs for CMCE `CallControl::Open`/`FloorGranted`, UMAC traffic mode, LMAC TCH/S uplink indications, and any FACCH stealing around speech start.

## 2026-06-04 10:42:58 EEST - PM agent orchestration reloaded

User directive:

- Add a Project Manager agent that orchestrates and delegates the work.
- Split agents into review, architecture, and QA responsibilities.
- Keep execution status in `timeline.md` so the next resume knows exactly what was done and what must happen next.
- Avoid loops and repeated work.
- Reload the ETSI law/status/project log before changing protocol behavior.

Law/status reload:

- Reloaded `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`.
- Reloaded `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`.
- Active goal remains clause-scoped ETSI EN 300 392-2 hardening. Do not claim formal certification without official conformance evidence.

Current repo state:

- Workdir: `/Users/ctermure/Work/basestion`.
- Branch: `nexus-bs-v0.1.55`.
- Latest commit observed: `0031716 docs: record Nexus-BS test deployment checkpoint`.
- Dirty file observed: `crates/tetra-entities/tests/test_mm_bs.rs`.
- Dirty test adds restart-recovery coverage for terminal `2260618`, group `226333`, DemandLocationUpdating, group affiliation rebuild, and EG3 assignment after restart recovery.

Project Manager setup:

- New spawn was attempted, but the agent thread limit was already reached.
- Existing agent `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`) is now the Project Manager.
- PM owns execution order, anti-loop discipline, timeline handoff quality, and ETSI-law reminders.
- PM must not edit protocol code directly unless explicitly re-tasked; it reports integration guidance back to the main worker.

Agent role split:

- PM / orchestration: `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`).
- Review agent: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`).
- Architecture agent, voice path: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`).
- QA agent: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`).
- MAC/UMAC scheduling architect: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`).
- MM/SDS robustness architect: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`).

Simple component meanings for this phase:

- PM: keeps the work ordered and makes sure each next step has evidence.
- Review: catches regressions, missing tests, and unsupported compliance claims.
- Architecture voice path: traces why live speech becomes static between CMCE, UMAC, LMAC, and PHY.
- QA: defines the proof matrix for BASIC functionality and 24x7 stability.
- MAC/UMAC: controls grants, timeslots, TCH/S traffic routing, FACCH/STCH stealing, and EG scheduling.
- MM/SDS: controls registration, affiliation, restart recovery, EG behavior, SDS/status, and packet/WAP dependencies.

Delegated tasks now in flight:

- PM: read law memories, `timeline.md`, and git state; return a non-repeating execution plan and log text.
- Review: inspect current diff and timeline for regressions/missing tests, especially the MM restart-recovery test and unsupported certification wording.
- Voice architecture: trace the static-audio defect in live private/group calls without repeating the already-passed pure UMAC bit-copy tests.
- QA: produce the next BASIC validation matrix for group call, private simplex/duplex, SDS/status, WAP, restart recovery, scan/group retention, and soak.
- MAC/UMAC architecture: identify next checks for traffic grants, TCH/S routing, FACCH/STCH stealing, EG/T.210 interactions, and PTT denial.
- MM/SDS architecture: review restart recovery, group rebuild, EG3 behavior, SDS/status routing, and WAP packet dependency.

Anti-loop state:

- Do not repeat pure UMAC TCH/S bit-preservation tests as the main static-audio hypothesis; those component tests already passed.
- Do not treat CMCE group release timer tests as proof of live voice correctness.
- Do not assume restart recovery is complete only from a cache entry. It must be rebuilt from confirmed terminal responses and group reports.
- Do not change encryption now; it is tracked but not current focus.
- Keep `call_preemptive` default off.
- Keep every protocol patch clause-scoped to EN 300 392-2 and backed by focused tests/logs.

Next execution order:

1. Finish and verify the dirty MM restart-recovery test in `crates/tetra-entities/tests/test_mm_bs.rs`.
2. Run focused local verification:
   - `cargo fmt -p tetra-entities`
   - `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked`
   - `cargo test -p tetra-entities --test test_mm_bs --locked`
   - `git diff --check`
3. Integrate agent feedback into this timeline before further protocol changes.
4. For the static live-audio defect, inspect post-deploy logs only around actual PTT attempts for `2260082`, `2260616`, and group `226333`.
5. If logs show wrong CMCE circuit source or floor state, patch CMCE with clause scope 14.5.1/14.5.2.
6. If logs show correct CMCE but bad TCH/S framing or slot routing, patch UMAC/LMAC with clause scope 23.5/23.5.4.
7. If logs show RF/PHY direction-specific decode failure, isolate PHY quality and do not mask it with CMCE/MAC guesses.
8. Only after local tests pass, build locally and deploy direct to testing on `chris@192.168.1.179`; never compile on the Pi and do not create binary backups.

Evidence required before calling BASIC paths robust:

- Private simplex: both `2260082 -> 2260616` and `2260616 -> 2260082` pass with intelligible speech and matching floor logs.
- Group call: at least two radios can alternately PTT on group `226333` with intelligible speech and no stale floor state.
- Private duplex: setup, media direction, release, and fallback behavior tested separately from simplex.
- SDS/status: ISSI and GSSI routing tested with expected L2 service.
- WAP: terminal browser reaches the Nexus-BS home page and receives the configured greeting page.
- Restart recovery: terminals re-register/re-affiliate after BS restart without fabricated stale state.
- Scan/group retention: terminals keep usable group state after restart and during normal idle/scan cycles.
- 24x7 stability: log rotation, subscriber registry health, periodic call/SDS/WAP probes, and bounded resource growth are measured.

## 2026-06-04 10:44:34 EEST - MM restart recovery WIP verified

PM feedback integrated:

- The PM confirmed that `crates/tetra-entities/tests/test_mm_bs.rs` is the only dirty protocol test file.
- The dirty WIP is the correct next item because it covers restart recovery from a cache seed without fabricating stale registration/group/EG state.
- The static-audio blocker still needs fresh live PTT logs; pure UMAC bit-copy tests must not be repeated as the main hypothesis.
- MAC/UMAC `generate_default_blks` frame-18 filler is a parked candidate, not the active patch, until the current MM WIP is resolved or explicitly assigned.

MM component meaning in this patch:

- MM is Mobility Management. It handles terminal registration, location update, group affiliation, detach, and energy economy negotiation.
- This test checks that after a BS restart, a cached terminal is only recovered after the terminal responds with a real `U-LOCATION UPDATE DEMAND`.
- It also checks that the BS rebuilds group affiliation and EG3 state from the terminal response instead of trusting stale cache state as active truth.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may command a terminal to perform location updating.
- EN 300 392-2 clause 16.9.3.4: U-LOCATION UPDATE DEMAND carries the terminal response.
- EN 300 392-2 clause 16.10.23: GroupIdentityLocationAccept carries the group-location accept result.
- EN 300 392-2 clause 16.7.1: energy economy negotiation/assignment is rebuilt from the active exchange.

Verification run locally:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 105 passed.
- `git diff --check` -> pass.

Current conclusion:

- Restart-recovery MM test coverage is now verified locally.
- This does not prove the live static-audio issue is fixed.
- Next non-repeating action remains live post-deploy PTT log collection for private simplex and group call, then patch the first failing layer proven by logs.

## 2026-06-04 10:46:53 EEST - MM/SDS architect review integrated

Agent feedback integrated from MM/SDS robustness architect:

- The restart-recovery test is valid for the clause-scoped objective.
- It needed stronger assertions for the L2 service of `D-LOCATION-UPDATE-COMMAND`.
- It needed explicit `GroupIdentityLocationAccept` assertions for the rebuilt GSSI affiliation.
- Its ETSI comment needed the precise GroupIdentityLocationAccept and EG-specific clauses.

Patch refinement:

- `test_restart_recovery_demand_location_update_restores_affiliation_and_eg3` now asserts that the recovery command uses `Layer2Service::Acknowledged`.
- The test now asserts that `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept`, lists GSSI `226333`, carries attachment lifetime/class-of-usage, and does not encode detachment for that group.
- The ETSI comment now names clauses 16.4.4, 16.9.3.4, 16.10.23, 16.10.35a, 16.7.1, 16.10.9, 16.10.10, and 23.7.6/T.210.
- Added helper `location_update_command_details` for tests that need address, handle, L2 service, and decoded PDU without changing existing helper call sites.

Verification after refinement:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_demand_location_update_restores_affiliation_and_eg3 --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 105 passed.
- `git diff --check` -> pass.

Remaining MM/SDS gaps after this patch:

- Restart recovery with group report complete/empty list is now the next targeted test to verify, proving stale groups are not restored.
- SDS/status ISSI/GSSI routing still needs focused tests and, if needed, patches.
- WAP over SDS Type 4 home page is separate from full SNDCP packet WAP; full WAP/SNDCP remains open unless explicitly implemented.

## 2026-06-04 10:49:30 EEST - QA BASIC/24x7 matrix integrated

QA feedback integrated:

- The immediate MM verification path is complete in this turn.
- UMAC pure voice bit-copy, CMCE timer, and deployed test evidence from earlier timeline entries must not be reused as proof of live audio correctness.
- Static audio and PTT denial require fresh live Pi evidence from real PTT attempts.
- SDS/WAP test names from QA must be checked for existence before running; do not burn time on non-existent filters.

Minimum BASIC pass evidence:

- Private simplex:
  - Test `2260082 -> 2260616`, then `2260616 -> 2260082`.
  - Required evidence: setup/connect, `CallControl::Open`, `D-TX GRANTED`, correct `FloorGranted source_issi`, intelligible audio both ways, no unjustified `PTT denied`, no static.
- Group call:
  - Test alternating PTT on GSSI `226333` with at least two terminals.
  - Required evidence: floor granted to active speaker, group listeners receive the speech path, no stale floor, no static.
- Restart recovery:
  - Local MM test now covers cache seed -> command -> confirmed DemandLocationUpdating -> register/affiliate/EG3.
  - Live test still required after BS restart with observed terminals.
- Group retention and scan:
  - Required evidence: terminal group state is rebuilt from terminal response, remains usable after idle/scan cycles, and PTT works on selected group.
- SDS/status:
  - Required evidence: ISSI route uses acknowledged delivery; GSSI route uses unacknowledged delivery; local status/SDS is delivered to intended target.
- WAP:
  - Required evidence: terminal browser reaches the Nexus-BS greeting page.
  - Current MVP is SDS Type 4/WAP-style page delivery; full WAP over SNDCP packet data remains an open implementation track unless explicitly completed.
- Private duplex:
  - Required evidence: duplex setup/media/release if terminal support exists; otherwise documented simplex fallback without wrong bearer setup.
- Long-run BS robustness:
  - Required evidence: 24h soak with dashboard health, subscriber registry stability, call/SDS/WAP probes, bounded logs/memory, no panic, no D-RELEASE storm, no `req_handle=0 ambiguous`, no unjustified PTT denial.

Recommended soak probes after the live blocker is fixed:

- Every 15 minutes: dashboard health, registry ISSI/GSSI snapshot, error log scan.
- Every 60 minutes: group PTT alternation and private simplex in both directions.
- Every 2 hours: SDS/status plus WAP home-page delivery.
- Once during soak: controlled BS restart followed by re-register/re-affiliate verification.

Current waiting agent feedback:

- Review, voice architecture, and MAC/UMAC architecture agents were still running at this checkpoint.
- Do not block on them for the verified MM test.
- Integrate their findings before the next protocol patch if they return concrete risks or a better live-log plan.

## 2026-06-04 10:52:29 EEST - Review feedback applied to MM restart recovery

Review feedback integrated:

- Tightened clause language: `GroupIdentityLocationAccept` is anchored to EN 300 392-2 clause 16.10.23.
- Kept 16.10.17 out of the restart-recovery group-location accept explanation except where mode=1 attach/detach semantics are explicitly under test elsewhere.
- Confirmed no formal certification claim was introduced; wording remains clause-scoped engineering evidence.

Additional test added:

- `test_restart_recovery_group_report_complete_keeps_groups_empty`
- Scenario: cache-seeded ISSI `2260618` gets `D-LOCATION-UPDATE-COMMAND`, then answers `U-LOCATION UPDATE DEMAND` with group-report-complete/empty list.
- Expected result: BS registers the terminal, does not advertise stale GSSI entries, does not affiliate GSSI `226333`, does not create EG state, keeps recovery cache, and stops further recovery probes.

Verification after review integration:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_group_report_complete_keeps_groups_empty --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 106 passed.
- `git diff --check` -> pass.

MM restart-recovery coverage now includes:

- Cache seed -> recovery command with `Layer2Service::Acknowledged`.
- DemandLocationUpdating response with GSSI report -> register, affiliate, EG3 assignment, no frame-18 EG start.
- DemandLocationUpdating response with complete empty group report -> register only, no stale group affiliation, no EG state.

Next non-repeating execution:

- Do not add more MM restart-recovery tests unless a new specific gap appears.
- Resume live static-audio investigation with post-deploy logs for private simplex and group PTT.
- First live log question: does CMCE open the correct circuit/media source and floor state for both `2260082` and `2260616` before UMAC/LMAC traffic starts?
- Second live log question: if CMCE is correct, does UMAC/LMAC preserve valid TCH/S traffic around `D-TX GRANTED`, FACCH/STCH stealing, and `D-TX CEASED`?

## 2026-06-04 10:57:25 EEST - MM committed and SDS/status/WAP evidence refreshed

Committed local evidence batch:

- Commit: `7f85bc0 test: harden MM restart recovery evidence`
- Scope: MM restart-recovery tests and project timeline only.
- Verification immediately before commit:
  - `cargo fmt -p tetra-entities` -> pass.
  - `cargo test -p tetra-entities --test test_mm_bs --locked` -> 106 passed.
  - `git diff --check` -> pass.

Live BS/log inspection:

- Remote test BS process still running on `chris@192.168.1.179`: `bin/nexus-bs config.live.toml`, pid `15161`.
- Active log: `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log`.
- No fresh post-checkpoint private/group PTT evidence was found in the current log tail.
- Older log entries contain `U-TX CEASED`, repeated `Hangtime expired`, and soft reattach markers before the current non-repeat checkpoint; do not use those as proof of the current static-audio defect without a fresh PTT attempt.

SDS/status/WAP component meaning:

- SDS is short data service: small user/application data over CMCE.
- STATUS is pre-coded SDS signalling: compact 16-bit status codes.
- WAP MVP currently rides on SDS Type 4/SDS-TL style delivery, not full SNDCP packet WAP.

SDS/status/WAP evidence refreshed:

- ETSI anchors checked locally:
  - EN 300 392-2 clause 13.2: individual/group SDS services.
  - EN 300 392-2 D-STATUS table 14.14 and U-STATUS table 14.27.
  - EN 300 392-2 clause 18.3.5.3.1: layer 2 service selection.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 112 passed.
- Passing coverage includes:
  - ISSI D-STATUS uses `Layer2Service::Acknowledged`.
  - GSSI D-STATUS uses `Layer2Service::Unacknowledged`.
  - local group SDS/status routes as GSSI.
  - all-ones broadcast SDS/status uses GSSI and unacknowledged delivery.
  - WAP MVP text variants preserve the requested Nexus-BS message and Type 4 payload budget.

Current conclusion:

- SDS/status/WAP component evidence is current locally.
- Live WAP terminal-browser validation still remains separate from component tests.
- Voice static/PTT denial remains the active live blocker and needs a fresh PTT trace.

## 2026-06-04 11:06:54 EEST - PM/review/architecture/QA orchestration checkpoint

User directive:

- Keep a Project Manager agent responsible for orchestration and delegation.
- Split work across review, architecture, and QA agents.
- Keep `timeline.md` current enough that the next resume knows exactly what has been done and what comes next.
- Reload the ETSI law/status/project log before protocol work.
- Avoid loops, repeated hypotheses, and unsupported certification claims.

Law/status reload completed before this checkpoint:

- `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`
- `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`
- `/Users/ctermure/.codex/memories/flowstation-aarch64-soapysdr-build.md`
- Active goal remains clause-scoped ETSI EN 300 392-2 hardening. Do not claim formal certification without official conformance evidence.

Current repo state:

- Workdir: `/Users/ctermure/Work/basestion`
- Branch: `nexus-bs-v0.1.55`
- HEAD: `3776497 chore: trace live voice circuit routing`
- Worktree: clean
- Latest relevant commits:
  - `3776497 chore: trace live voice circuit routing`
  - `6a13dd1 docs: refresh SDS and live validation evidence`
  - `7f85bc0 test: harden MM restart recovery evidence`

Agent orchestration state:

- A new PM spawn was attempted, but the sub-agent thread limit was already reached.
- Existing agent `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`) remains assigned as Project Manager.
- Review agent: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`).
- Voice-path architecture agent: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`).
- QA agent: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`).
- MAC/UMAC scheduling architect: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`).
- MM/SDS robustness architect: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`).

Agent feedback integrated:

- PM: keep execution ordered by evidence. Do not reopen solved UMAC bit-copy, MLE handle ambiguity, or MM restart-recovery coverage unless fresh logs/tests prove a new gap.
- Review: the MM restart-recovery evidence is now committed and no formal certification wording was introduced.
- QA: BASIC validation must still prove private simplex both directions, group call on `226333`, SDS/status, WAP terminal delivery, restart re-affiliation, scan/group retention, and long-run stability.
- Voice architecture: live symptom still looks like media starvation or lower-layer traffic handling after a valid floor grant. The next live proof must compare good and bad PTT directions using `CallControl::Open`, `FloorGranted`, UMAC circuit metadata, LMAC TCH/S decode, and PHY train/block logs.
- MAC/UMAC architecture: pure UMAC TCH/S bit preservation is already covered. The strongest non-repeating code suspect is BS LMAC handling of `NormalTrainSeq2` second half (`Block2`) when the first half is STCH and the second half is non-stolen TCH/S.
- MM/SDS architecture: MM restart recovery is closed for current scope; WAP MVP is SDS Type 4/WAP PID delivery, not full SNDCP/IP WAP.

Simple component meanings for next work:

- CMCE is call control: setup, PTT/floor, call release, and who is allowed to talk.
- UMAC/MAC is the scheduler: grants resources, maps speech/signalling to slots, and routes uplink voice to the right downlink slot.
- LMAC is burst framing: turns physical traffic blocks into TCH/S voice frames and drops bad CRC frames.
- PHY is the radio burst layer: train sequence, half-slot/block identity, RF decode quality, and timing.
- MM is terminal mobility: registration, restart recovery, group affiliation, and energy economy.
- SDS/status is short data/status messaging.
- WAP MVP is the terminal browser page path over SDS Type 4/WAP PID; full SNDCP/IP WAP remains a separate open track.

Immediate non-repeating execution order:

1. Do not add more MM restart-recovery tests unless a new gap appears; current MM batch is committed in `7f85bc0`.
2. Use the current observability commit `3776497` to capture fresh live PTT logs after a real test:
   - private simplex `2260082 -> 2260616`;
   - private simplex `2260616 -> 2260082`;
   - group PTT on GSSI `226333`.
3. In those logs, answer first:
   - does CMCE open the expected local `CircuitDlMediaSource::LocalLoopback` circuit for local private/group calls?
   - does `FloorGranted source_issi` match the radio pressing PTT?
   - does UMAC route UL TCH/S to the expected DL slot/peer slot?
   - does LMAC report valid TCH/S frames, CRC failures, or ignored partial blocks?
4. If logs prove CMCE opens the wrong circuit, patch CMCE under EN 300 392-2 clause 14.5.1/14.5.2 with a focused test.
5. If CMCE is correct but LMAC ignores or drops real TCH/S, patch LMAC under EN 300 392-2 clause 23.5.4 with tests around `NormalTrainSeq2`/STCH/TCH half-slot behavior.
6. If LMAC sees CRC failures or PHY train mismatch only for one terminal/direction, isolate RF/PHY quality before masking it with call-control changes.
7. After a focused local patch passes tests, build locally only and deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; do not compile on `chris@192.168.1.179` and do not create binary backups.

ETSI anchors for the next likely patch:

- EN 300 392-2 clause 14.5.1: private/individual call control and floor handling.
- EN 300 392-2 clause 14.5.2: group call control and floor handling.
- EN 300 392-2 clause 23.5 / 23.5.4: MAC traffic channel, FACCH/STCH/TCH behavior, and traffic block handling.
- EN 300 392-2 clause 13.2 and tables 14.14/14.27: SDS/status if the next patch touches data/status.
- EN 300 392-2 clauses 16.4.4, 16.7.1, 16.10.9, 16.10.10, 16.10.23, 16.10.35a, 23.7.6/T.210: MM/EG only if the next patch touches restart or energy economy.

Anti-loop rules from this checkpoint:

- Do not repeat UMAC pure bit-copy tests as the main static-audio investigation; they already pass.
- Do not treat SDS/WAP component tests as proof that the terminal browser live page works; live WAP validation remains required.
- Do not call the stack `100% certified`; only clause-scoped engineering evidence exists.
- Keep `call_preemptive` / transmission interruption default off.
- Encryption remains out of focus unless explicitly requested.

## 2026-06-04 11:14:01 EEST - UMAC idle traffic no longer emits all-zero speech

Patch scope:

- File changed: `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
- Component: UMAC/MAC downlink scheduler.
- Simple meaning: this scheduler decides what the BS transmits on an assigned traffic slot. If it has real uplink speech, it sends TCH/S speech. If it has call-control signalling, it uses FACCH/STCH stealing. If it has neither, it must keep the assigned channel alive without inventing invalid speech.

ETSI clause scope:

- EN 300 392-2 clause 23.8.5: when the BS does not receive data from the sending MS, it should still transmit on the downlink channel; examples include C-plane Null PDUs or substitution traffic.
- EN 300 392-2 clause 23.5 / 23.5.4: traffic channel and STCH/FACCH slot handling.
- This is clause-scoped engineering hardening, not a formal certification claim.

Behavior changed:

- Before: an active DL traffic circuit with no queued uplink voice produced a 274-bit all-zero TCH/S block as "silence".
- After: an active DL traffic circuit with no queued uplink voice transmits C-plane Null PDUs on STCH half-slots.
- FACCH/STCH with real queued voice still keeps first half = STCH and second half = TCH/S.
- FACCH/STCH without queued voice uses first half = STCH signalling and second half = STCH Null PDU.
- This avoids sending an all-zero ACELP frame as clean speech when that frame is not proven to be a valid TETRA silence/substitution frame.

Tests added/updated:

- Added `test_active_traffic_slot_without_voice_uses_stch_null_not_zero_tch`.
- Added `test_facch_without_voice_replaces_second_half_with_stch_null`.
- Added `test_facch_with_voice_keeps_second_half_tch_s`.
- Updated EG/FACCH tests that previously expected TCH/S zero filler while a FACCH item was deferred or pruned.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib active_traffic_slot_without_voice --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib facch_ --locked` -> 11 passed.
- `cargo test -p tetra-entities --lib --locked` -> 204 passed, 5 ignored.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 2 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Current conclusion:

- This patch removes one plausible static-audio source: synthetic all-zero TCH/S during active call gaps or missing uplink voice.
- It does not prove live private/group audio is fixed; live PTT validation is still required.
- Next live test must still capture `2260082 -> 2260616`, `2260616 -> 2260082`, and group `226333`, with logs for `CMCE opening UMAC circuit`, `FloorGranted`, `UMAC voice route`, `rx_blk_traffic`, CRC failures, and STCH/FACCH events.

## 2026-06-04 11:17:37 EEST - Deployed null-idle traffic patch to test BS

Commit deployed:

- `2201923 fix: transmit null traffic idle instead of zero speech`

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `bcd5bc2cff5e1f253f80b66c0009db7da5930de9117b348ea63b9bbf49006da5`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- First SCP failed while the old BS process still held the destination open.
- Stopped test BS/control-service via existing pidfile method.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `bcd5bc2cff5e1f253f80b66c0009db7da5930de9117b348ea63b9bbf49006da5`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `15798`
  - control-service pid `15801`
  - nexus-bs pid `15803`

Post-restart live checks:

- Dashboard root on `127.0.0.1:8080` returned Nexus-BS v0.1.55 HTML.
- `2260082` re-registered/re-affiliated to group `226333`; RSSI around `-22 dBFS`; EG3 allocated after registration.
- `2260616` re-registered/re-affiliated to group `226333`; RSSI around `-33 dBFS`; EG3 allocated after registration.
- `2260618` re-registered/re-affiliated to group `226333`; RSSI around `-38 dBFS`; requested EG1 and BS allocated EG3.
- No post-deploy PTT attempt was present yet in the checked log filters for `U-TX DEMAND`, `D-TX GRANTED`, `UMAC voice route`, `rx_blk_traffic`, `UL inactivity`, `PTT denied`, or `NotGranted`.

Next operator validation:

- Test private simplex `2260082 -> 2260616`, then `2260616 -> 2260082`.
- Test group PTT on `226333` with at least two radios alternating.
- If static remains, collect only post-`2201923` logs and decide by evidence:
  - wrong CMCE circuit/floor -> patch CMCE under EN 300 392-2 clause 14.5.1/14.5.2;
  - no valid `rx_blk_traffic` after grant -> isolate LMAC/PHY;
  - valid `UMAC voice route` but bad receive audio -> inspect downlink FACCH/STCH/TCH and RF path.

## 2026-06-04 11:23:47 EEST - PM orchestration refreshed and delegated

User directive:

- Add a Project Manager agent to orchestrate the work.
- Split work into review, architecture, and QA responsibilities.
- Reload ETSI law/status/project log before further protocol work.
- Keep execution state and next actions in `timeline.md` so the next resume does not loop.

Law/status reload completed before this checkpoint:

- `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`
- `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`
- `/Users/ctermure/.codex/memories/flowstation-aarch64-soapysdr-build.md`

Current repo state:

- Workdir: `/Users/ctermure/Work/basestion`
- Branch: `nexus-bs-v0.1.55`
- HEAD: `84a15d9 docs: record null-idle test deployment`
- Worktree: clean
- Active goal: clause-scoped ETSI EN 300 392-2 hardening only. This is not a formal certification claim.

PM agent status:

- A new PM agent spawn was attempted again, but the agent thread limit is reached.
- Existing agent `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`) remains assigned as Project Manager.
- PM role: orchestration only. PM owns execution order, anti-loop discipline, timeline handoff quality, and evidence gates.

Delegated agent roles:

- PM / orchestration: `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`).
- Review: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`).
- Voice architecture: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`).
- QA: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`).
- MAC/UMAC architecture: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`).
- MM/SDS robustness: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`).

Simple component meanings for operators and next handoff:

- PM: keeps the work ordered and blocks circular work without evidence.
- Review: looks for regressions, missing tests, and unsupported compliance wording.
- CMCE: call control. It handles call setup, floor/PTT grant, who may speak, and call release.
- UMAC/MAC: radio scheduler. It maps signalling and speech to slots and routes uplink voice to downlink users.
- LMAC: burst framing. It decodes/encodes traffic bursts, including TCH/S speech and CRC/BFI decisions.
- PHY: radio burst layer. It detects train sequence, block identity, timing, and RF decode quality.
- MM: mobility management. It handles registration, restart recovery, group affiliation, and energy economy.
- SDS/status: short data and status messaging.
- WAP MVP: terminal browser page delivery over SDS Type 4/WAP PID in the current stack. Full SNDCP/IP WAP is still a separate open implementation track.

Agent feedback integrated in this checkpoint:

- PM confirmed current state: `2201923` null-idle traffic patch is deployed and `84a15d9` records the deployment; no post-deploy live PTT result is yet recorded.
- QA produced a BASIC/24x7 validation matrix. Required live evidence remains private simplex both directions, private duplex if supported, group PTT on `226333`, SDS/status, WAP terminal page, restart re-affiliation, scan/group retention, and soak stability.
- MM/SDS confirmed restart-recovery and SDS/status component tests are strong for their current scope, but live WAP terminal-browser delivery and EG-window delivery evidence remain separate from unit tests.
- MM/SDS also confirmed current WAP is SDS Type 4/WAP MVP, not full SNDCP/IP WAP service advertising.
- Review, Voice Architecture, and MAC/UMAC agents were re-tasked and must be integrated before the next protocol patch if they return concrete risks.

Local technical observation for the next voice investigation:

- `crates/tetra-entities/src/lmac/lmac_bs.rs` classifies `NUB + NormalTrainSeq2 + Block2` as `LogicalChannel::TchS` when the burst is traffic and block 2 is not stolen.
- The same file's `rx_blk_traffic` currently forwards only `LogicalChannel::TchS` with `PhyBlockNum::Both`; it drops `Block2` as partial/unsupported.
- This is a credible non-repeating suspect for the live one-way/static-audio path, but it must not be patched by pretending a 216-bit half-block is a clean 274-bit ACELP frame.
- Clause scope if patched: EN 300 392-2 clause 23.5/23.5.4 for traffic channel STCH/TCH handling and clause 23.8.3 for bad/partial speech-frame handling. Any compatibility behavior must be labelled as such.

Immediate execution order:

1. Do not repeat pure UMAC TCH/S bit-copy tests as the main static-audio hypothesis; they already pass.
2. Collect fresh post-`2201923` live PTT evidence:
   - private simplex `2260082 -> 2260616`;
   - private simplex `2260616 -> 2260082`;
   - group PTT on GSSI `226333`, alternating radios.
3. For each attempt, map:
   - `U-TX DEMAND`;
   - `D-TX GRANTED`;
   - `FloorGranted source_issi`;
   - `CMCE opening UMAC circuit`;
   - `CircuitDlMediaSource`;
   - `UMAC voice route`;
   - `rx_blk_traffic`;
   - CRC/BFI;
   - FACCH/STCH/TCH placement;
   - operator audio result.
4. If CMCE circuit/floor is wrong, patch CMCE under EN 300 392-2 clause 14.5.1/14.5.2 with a focused test.
5. If CMCE is correct but LMAC drops valid traffic blocks, patch LMAC under EN 300 392-2 clause 23.5/23.5.4 and 23.8.3 with a focused unit test.
6. If LMAC sees CRC/PHY quality failures only in one direction, isolate PHY/RF before masking the issue in CMCE or UMAC.
7. After a focused local patch passes tests, build locally only and deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; do not compile on `chris@192.168.1.179` and do not create binary backups.

Evidence required before BASIC paths are called robust:

- Private simplex: `2260082 -> 2260616` and `2260616 -> 2260082` both have intelligible audio, correct floor owner, and no unjustified `PTT denied`.
- Group call: at least two radios alternate PTT on `226333` with intelligible audio and no stale floor.
- Private duplex: setup, media routing, and release are validated separately from simplex, or unsupported terminal behavior is documented without wrong bearer setup.
- SDS/status: ISSI route uses acknowledged delivery; GSSI route uses unacknowledged delivery; live delivery works.
- WAP: terminal browser reaches the Nexus-BS greeting page. Unit tests for SDS payload are not enough.
- Restart recovery: terminals re-register/re-affiliate after BS restart based on real terminal responses, not fabricated stale cache state.
- Scan/group retention: selected group remains usable after idle/scan cycles and after restart.
- 24x7 stability: process/dashboard/registry stay healthy, logs are bounded, periodic private/group/SDS/WAP probes pass, and there is no panic, log storm, stale floor, or repeated unjustified PTT denial.

Anti-loop rules:

- No broad refactor before the live voice blocker is classified by evidence.
- No certification wording. Use clause-scoped ETSI-aligned evidence only.
- `call_preemptive` / transmission interruption stays default off.
- Encryption remains out of focus.
- Do not advertise full SNDCP/IP WAP until that bearer is implemented and tested.

## 2026-06-04 11:26:10 EEST - Review/MAC agent feedback integrated

Agent feedback received:

- QA, PM, MM/SDS, MAC/UMAC, and Review agents returned read-only feedback.
- Voice Architecture was still running at the wait timeout; do not block on it unless it returns a concrete contradiction.

Review findings:

- High risk: `LMAC` can still drop the likely live half-slot speech path. `NormalTrainSeq2 + Block2` may be classified as `TchS`, but `rx_blk_traffic` only forwards `PhyBlockNum::Both`.
- High risk: there is still no post-`2201923` live PTT evidence, so null-idle traffic hardening is not proof that live static/audio is fixed.
- Medium risk: UMAC null-idle tests stop at scheduler output; add/keep LMAC boundary coverage for `STCH+STCH` and `STCH+TCH/S` through `NormalTrainSeq2`.
- Medium risk: CMCE and UMAC private simplex component coverage is good, but not end-to-end through LMAC/PHY for field ISSIs `2260082` and `2260616`.
- Low risk: SDS/WAP wording is currently safe because WAP is scoped as SDS Type 4 MVP, not full SNDCP/IP WAP.

MAC/UMAC architecture findings:

- Priority suspect is `LMAC` `NormalTrainSeq2` block semantics:
  - `determine_logical_channel_ul` may classify `Block2` as `TchS`;
  - `rx_blk_traffic` then drops non-`Both` traffic as partial/unsupported.
- This can bypass the existing UMAC voice tests because those tests inject `TmdCircuitDataInd` after LMAC.
- Do not forward a 216-bit half-block as clean 274-bit ACELP speech unless a correct ETSI/BFI-bearing path exists.
- Existing bad-CRC behavior is correct for the current SAP: since TMD cannot carry BFI/half-slot condition, corrupt/partial speech must fail closed instead of becoming clean speech/static.

Decision:

- Next local patch, if no fresher live logs contradict it, should be a focused LMAC evidence patch:
  - add a test proving `NormalTrainSeq2 + Block2` traffic is not silently treated as valid clean speech when only a half TCH/S block is available;
  - improve LMAC logging/guarding so live logs distinguish:
    - valid full TCH/S decoded and forwarded;
    - CRC/BFI drop;
    - partial `Block2` TCH/S drop due missing full speech frame support.
- This is an observability/safety patch unless a full ETSI-supported BFI/TMD SAP path is added.

ETSI clause scope for that patch:

- EN 300 392-2 clause 23.5 / 23.5.4: traffic-channel STCH/TCH/FACCH placement and block handling.
- EN 300 392-2 clause 23.8.3 / 23.8.3.2: bad/undecodable speech frame handling.
- Any current inability to carry BFI is local implementation limitation and must be labelled as such.

Focused local verification for the LMAC patch:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_lmac_bs --locked`
- `cargo test -p tetra-entities --lib facch_ --locked`
- `cargo test -p tetra-entities --test test_umac_bs voice --locked`
- `git diff --check`

Live validation still required after any patch:

- Test private simplex `2260082 -> 2260616`.
- Test private simplex `2260616 -> 2260082`.
- Test group PTT on `226333`.
- Log filter:
  - `2260082|2260616|2260618|226333|U-TX DEMAND|D-TX GRANTED|NotGranted|RequestQueued|GrantedToOtherUser|CallControl::Open|FloorGranted|FloorReleased|peer_ts|media_source|UMAC voice route|FACCH|STCH|TCH|NormalTrainSeq2|Block2|blk2_stolen|rx_blk_traffic|CRC fail|partial/unsupported|UL inactivity|T.210|energy`

## 2026-06-04 11:33:05 EEST - LMAC partial TCH/S guard and evidence tests

Patch scope:

- Files changed:
  - `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - `crates/tetra-entities/tests/test_lmac_bs.rs`
  - `timeline.md`
- Component: LMAC, the lower MAC burst-framing layer.
- Simple meaning: LMAC decides whether a received radio burst is control signalling, full TCH/S speech, or a partial/stolen traffic block. It must not present incomplete or bad speech as clean voice.

ETSI clause scope:

- EN 300 392-2 clause 23.5 / 23.5.4: traffic channel, STCH/FACCH/TCH placement, and burst/block handling.
- EN 300 392-2 clause 23.8.3 / 23.8.3.2: bad/undecodable speech frame handling.
- Current implementation limitation: the local TMD SAP does not carry BFI/half-slot condition, so LMAC fails closed for partial/bad speech rather than forwarding static as valid ACELP.

Behavior clarified:

- Before: `rx_blk_traffic` had one generic trace-level drop for every non-`Both` or non-`TchS` traffic block.
- After: LMAC explicitly distinguishes:
  - unsupported traffic channel -> trace and drop;
  - partial `TchS` block such as `NormalTrainSeq2 + Block2` -> debug log and drop because the TMD SAP cannot preserve BFI/half-slot condition;
  - full-slot `TchS` with good CRC -> decode and forward to UMAC;
  - full-slot `TchS` with bad speech CRC -> drop, as before.

Tests added/strengthened:

- Added `bs_lmac_forwards_valid_fullslot_tch_s_to_umac`.
  - Proves a valid 432-bit full-slot TCH/S frame decodes and reaches UMAC as `TmdCircuitDataInd`.
  - Also verifies the test vector round-trips through `encode_tp`/`decode_tp`.
- Added `bs_lmac_drops_normal_seq2_block2_tch_s_without_forwarding_clean_speech`.
  - Proves a `NormalTrainSeq2 + Block2` half TCH/S block is not forwarded as clean speech.
  - This protects against turning 216-bit partial speech into static/audio corruption.
- Strengthened `bs_lmac_drops_bad_crc_tch_s_instead_of_forwarding_static_speech`.
  - The harness now marks all four UL timeslots as traffic before injecting the corrupt TCH/S frame, so the test really exercises the traffic CRC path.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 4 passed.
- `cargo test -p tetra-entities --lib facch_ --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs voice --locked` -> 3 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Live log check during this checkpoint:

- Remote test BS on `chris@192.168.1.179` is still running:
  - wrapper/control pid `15798`;
  - control-service pid `15801`;
  - nexus-bs pid `15803`.
- Log still does not contain a complete post-`2201923` live PTT trace with `U-TX DEMAND`, `D-TX GRANTED`, `UMAC voice route`, and `rx_blk_traffic`.
- Several `rx_tpsap_prim got NormalTrainSeq2 in fullslot` entries exist after deploy. Without this LMAC patch, the log did not say whether those resulted in STCH control, partial TCH/S drop, or valid speech.

Conclusion:

- This patch does not claim live private/group audio is fixed.
- It makes the LMAC boundary safe and observable:
  - full valid TCH/S is proven to pass;
  - partial/bad TCH/S is proven not to become clean speech/static.
- Next step after commit/deploy is still a live private/group PTT test using the log filter from the previous checkpoint.

## 2026-06-04 11:36:35 EEST - Deployed LMAC guard build to test BS

Commit deployed:

- `bfc1960 test: guard LMAC partial speech handling`

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `b830daa3e18bcb478092e453a6be6618165b7110e109701c4a188cbf0865ae7c`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service using the existing pidfiles.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `b830daa3e18bcb478092e453a6be6618165b7110e109701c4a188cbf0865ae7c`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `16125`
  - control-service pid `16128`
  - nexus-bs pid `16130`

Post-restart checks:

- Dashboard root on `127.0.0.1:8080` returned HTML.
- `2260616` re-registered/re-affiliated to group `226333`; soft re-attach after restart was handled and EG3 assignment was attempted.
- `2260082` reappeared with RSSI/ACK activity and soft re-attach handling; EG assignment later timed out and fell back to `StayAlive`.
- `2260618` re-registered/re-affiliated to group `226333`; requested EG1 and BS allocated EG3.
- `T352 expired` appeared for BS-initiated EG assignment to `2260082` and `2260616`; current fail-safe behavior keeps `StayAlive`.
- One post-deploy `NormalTrainSeq2 in fullslot` was seen at PHY after restart, but no complete PTT trace was present yet.

Current live status:

- The deployed binary contains the LMAC partial TCH/S debug guard.
- No post-`bfc1960` private/group PTT attempt has been recorded yet.
- The next operator test must be:
  - private simplex `2260082 -> 2260616`;
  - private simplex `2260616 -> 2260082`;
  - group PTT on `226333`, alternating radios.
- Required log focus:
  - `U-TX DEMAND`, `D-TX GRANTED`, `FloorGranted`, `CMCE opening UMAC circuit`, `UMAC voice route`, `rx_blk_traffic`, `dropping partial TCH/S`, `CRC fail`, `NormalTrainSeq2`, `Block2`, `PTT denied`, `NotGranted`, `UL inactivity`.

## 2026-06-04 11:39:20 EEST - Voice architecture feedback integrated

Voice Architecture returned after the LMAC guard deploy.

Key feedback:

- Strongest non-repeating live suspect remains real-air LMAC TCH/S handling around `NormalTrainSeq2`, not UMAC bit-copy.
- Existing UMAC voice tests inject after LMAC, so they cannot prove that PHY/LMAC traffic bursts reach UMAC.
- The deployed `bfc1960` patch covers the safe fail-closed/logging side:
  - valid full-slot TCH/S passes;
  - partial `Block2` TCH/S is not forwarded as clean speech/static.
- It does not implement a BFI/half-slot-condition capable traffic SAP or explicit half-slot TCH/S decode. That remains a future LMAC design task if live logs prove terminals send speech primarily as `NormalTrainSeq2 + Block2`.

Additional patch candidate from Voice Architecture:

- CMCE private setup should not route a configured-local ISSI over Brew only because that ISSI is currently absent from `subscriber_groups`.
- For configured local SSI ranges, an unregistered callee should be rejected locally/recovered locally, not misclassified as external/Brew.
- ETSI scope if patched:
  - EN 300 392-2 clause 14.5.1.1.2 for first setup response/dummy call reference.
  - EN 300 392-2 clause 14.5.1.3.2 for unsupported/rejected individual-call release.
  - Local SSI range is a deployment policy guard, not an ETSI rule.

Next code task selected:

- Add a focused CMCE guard/test for local-range but unregistered private-call destination.
- Keep PBX/phone `called_ssi == 0` and non-local Brew-routable ISSIs on the existing Brew path.

## 2026-06-04 11:40:32 EEST - CMCE local unregistered private-call guard

Patch scope:

- Files changed:
  - `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - `crates/tetra-entities/tests/test_cmce_bs.rs`
  - `timeline.md`
- Component: CMCE private-call setup.
- Simple meaning: CMCE decides whether a private call is local, external/Brew, or rejected before setup.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.1.2: first SwMI response to individual-call setup.
- EN 300 392-2 clause 14.5.1.3.2: rejecting unsupported/unreachable individual call with D-RELEASE before a SwMI call identity exists.
- Configured `local_ssi_ranges` remains deployment policy, not an ETSI address rule.

Behavior changed:

- Before: if a called private ISSI was not in `subscriber_groups`, CMCE entered the Brew fallback path first. Later Brew routing checks could reject it, but logs/semantics misclassified a configured-local offline ISSI as an external routing candidate.
- After: if `called_addr.ssi` is inside `config.cell.local_ssi_ranges` and is not registered/affiliated locally, CMCE rejects locally with dummy-call-id `D-RELEASE` cause `CalledPartyNotReachable`.
- PBX/phone calls with `called_ssi == 0` still use the Brew path.
- Non-local unregistered ISSIs still use the existing Brew path if routable/configured.

Test added:

- `test_p2p_setup_to_configured_local_unregistered_issi_rejects_without_brew_fallback`
  - Configures local SSI range `2260000..2269999`.
  - Registers only the caller.
  - Calls local but unregistered `2260616`.
  - Asserts one dummy-call-id `D-RELEASE` with `CalledPartyNotReachable`.
  - Asserts no `NetworkCircuitSetupRequest` is sent to Brew and no UMAC traffic circuit opens.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_setup_to_configured_local_unregistered_issi_rejects_without_brew_fallback --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Conclusion:

- This is a private-call routing hardening patch. It does not claim live audio is fixed.
- It prevents one misleading local-vs-external call setup path for lab ISSIs such as `2260082`, `2260616`, and `2260618` when they are configured local but not currently registered.

## 2026-06-04 11:43:35 EEST - Deployed CMCE local setup guard to test BS

Commit deployed:

- `1b390c8 fix: reject local unregistered private setup locally`

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `57875e881f324b462a03f893aa705fdbcf2ae02bbf74a7a7729e5cb0a024253d`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service using existing pidfiles.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `57875e881f324b462a03f893aa705fdbcf2ae02bbf74a7a7729e5cb0a024253d`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `16325`
  - control-service pid `16328`
  - nexus-bs pid `16330`

Post-restart checks:

- Dashboard root returned `Nexus-BS v0.1.55 Dashboard`.
- `2260082` reappeared, re-affiliated to group `226333`, and showed RSSI/ACK activity around `-22 dBFS`.
- `2260616` re-registered/re-affiliated to group `226333`, RSSI around `-26 dBFS`, and EG3 assignment was attempted.
- `2260618` had post-restart retry/LLC activity in the sampled log tail; confirm full registration/affiliation again before using it for WAP/live call evidence.
- `T352 expired` appeared for BS-initiated EG assignment to `2260082`; fail-safe behavior kept `StayAlive`.

Current live status:

- The test BS now includes:
  - null-idle traffic patch `2201923`;
  - LMAC partial TCH/S guard `bfc1960`;
  - CMCE local-unregistered private setup guard `1b390c8`.
- No post-`1b390c8` private/group PTT attempt has been recorded yet.
- Next operator validation is unchanged:
  - private simplex `2260082 -> 2260616`;
  - private simplex `2260616 -> 2260082`;
  - group PTT on `226333`, alternating radios.
- Required live log filter:
  - `2260082|2260616|2260618|226333|U-SETUP|D-SETUP|D-CONNECT|U-CONNECT|U-TX DEMAND|D-TX GRANTED|FloorGranted|CMCE opening UMAC circuit|media_source|peer_ts|UMAC voice route|rx_blk_traffic|dropping partial TCH/S|CRC fail|NormalTrainSeq2|Block2|PTT denied|NotGranted|UL inactivity|CalledPartyNotReachable`

## 2026-06-04 11:53:57 EEST - Group floor handoff FACCH compacting patch

User live report:

- Group call still failed for alternating speakers; requirement is multiple MSs on one GSSI taking turns normally.
- Treat this as BASIC TETRA SwMI behavior, not a certification claim.

Live log evidence from `chris@192.168.1.179:/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log`:

- Group call `call_id=4`, GSSI `226333`, traffic slot `ts=2`.
- `2260616` had floor first and UMAC routed voice repeatedly:
  - `UMAC floor granted: call_id=4 source_issi=2260616 dest_gssi=226333 ul_ts=2 media_source=LocalLoopback`
  - repeated `UMAC voice route: UL ts=2 bits=274 -> DL ts=2`.
- After `U-TX CEASED`, `2260082` requested floor:
  - `U-TX DEMAND: ISSI 2260082 requests floor on call_id=4`
  - CMCE emitted `D-TX GRANTED`.
- Defect observed:
  - `D-TX GRANTED` with optional transmitting-party address serialized as a 61-bit CMCE SDU.
  - With the MAC-RESOURCE header it exceeded STCH capacity: `MAC-RESOURCE hdr 70 + SDU 61 bits > 124`.
  - UMAC therefore fell back to MCCH/SCH-F while the terminals were on the assigned traffic channel.
  - The preserved `RandomAccessAck` for `2260082` on `ts=2` was discarded by `dl_drop_all_except_stolen`.
  - No `UMAC voice route` followed for `2260082`; `UL inactivity timeout on ts=2` fired.

Component explanation:

- CMCE group floor control decides who may transmit next in the group.
- `D-TX GRANTED` is the CMCE message that gives one MS the microphone and tells the group that another user is transmitting.
- FACCH/STCH is the assigned traffic-channel signalling path. If the floor grant falls back to common-channel SCH/F while radios are listening on the assigned channel, the new speaker may never switch to transmit voice.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: during a group call, SwMI sends individual `D-TX GRANTED` to the granted MS and group-addressed `D-TX GRANTED` to the other MSs.
- EN 300 392-2 table 14.18: transmitting-party type/address IEs in `D-TX GRANTED` are optional/conditional.
- EN 300 392-2 clause 23.5 and 23.5 traffic-mode STCH/FACCH text: signalling may be stolen from the traffic channel for call-control messages during an over.
- Engineering decision: omit optional transmitting-party IEs in assigned-channel `D-TX GRANTED` so the mandatory floor state fits STCH/FACCH. This is clause-scoped ETSI-aligned behavior, not a full-stack certification claim.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - `fsm_send_d_tx_granted_individual` now emits compact `D-TX GRANTED` without optional transmitting-party IEs.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - `send_d_tx_granted_facch` now emits compact group-addressed `D-TX GRANTED`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `assert_compact_d_tx_granted_facch`.
  - Updated group floor queue, default-off preemption, enabled preemption grant, unaffiliated rejection, and queued handoff tests to require compact 25-bit `D-TX GRANTED`.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs group_tx_demand --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_hands_floor_to_queued_requester --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_preemptive --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_umac_bs facch --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_private_floor_grant_stch_carries_preserved_random_access_ack --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Current conclusion:

- The logged group-call failure is explained by an oversized `D-TX GRANTED` floor handoff that could not be transmitted on assigned-channel STCH/FACCH.
- The patch removes that cause and keeps preemption default-off.
- Live group audio is not yet proven fixed until this build is deployed and `2260616`/`2260082` alternate PTT on GSSI `226333` without `UL inactivity timeout`, `PTT denied`, or SCH/F fallback for `D-TX GRANTED`.

Next non-repeating execution:

1. Commit this narrow CMCE group FACCH compacting patch.
2. Build locally only with the Nexus-BS AArch64 command from build memory.
3. Deploy direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup.
4. Restart BS test.
5. Live test:
   - group PTT `2260616 -> 226333`, release;
   - group PTT `2260082 -> 226333`, release;
   - repeat at least three alternating turns.
6. Required pass evidence:
   - compact `D-TX GRANTED` does not log `does not fit STCH`;
   - no `dl_drop_all_except_stolen` discards the requester ACK needed for floor grant;
   - `UMAC floor granted` changes source ISSI on each turn;
  - `UMAC voice route` appears for each speaker;
  - no `UL inactivity timeout` during an active speaker over;
  - operator audio verdict confirms voice, not static.

## 2026-06-04 11:58:30 EEST - Deployed compact group floor grant build to test BS

Commit deployed:

- `419ce67 fix: compact group floor grants for FACCH`

Local verification before deploy:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_cmce_bs group_tx_demand --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_hands_floor_to_queued_requester --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_preemptive --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_umac_bs facch --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_private_floor_grant_stch_carries_preserved_random_access_ack --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `427626e5f9bffc708884aa77534fad5d63673ff9049ed489d0f6a383c1f16c12`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service via existing pidfiles before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `427626e5f9bffc708884aa77534fad5d63673ff9049ed489d0f6a383c1f16c12`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `16704`
  - control-service pid `16707`
  - nexus-bs pid `16709`
- Dashboard root returned HTML.

Post-restart terminal state:

- `2260082` registered and affiliated to `[226333]` twice during restart recovery; RSSI about `-22 dBFS`.
- `2260616` registered and affiliated to `[226333]`; it also deaffiliated/re-affiliated once during group update; EG assignment later timed out and stayed/fell back to `StayAlive`.
- `2260618` registered and affiliated to `[226333]`; RSSI about `-46 dBFS`; EG3 was allocated.

Current live status:

- No post-`419ce67` PTT test has been recorded yet.
- The BS is ready for physical group test on GSSI `226333`.
- Required test sequence:
  - `2260616` PTT on group `226333`, speak, release.
  - `2260082` PTT on group `226333`, speak, release.
  - Repeat at least three alternating turns.
- Required log filter after the test:
  - `2260082|2260616|226333|U-TX DEMAND|D-TX GRANTED|does not fit STCH|FACCH stealing|FloorGranted|UMAC floor granted|UMAC voice route|dl_drop_all_except_stolen|RandomAccessAck|UL inactivity|PTT denied|NotGranted`

Pass criteria for this patch:

- No `D-TX GRANTED ... does not fit STCH` on the group floor handoff path.
- No requester `RandomAccessAck` is discarded at the handoff.
- `UMAC floor granted` alternates `source_issi` between `2260616` and `2260082`.
- `UMAC voice route` appears after each grant.
- No `UL inactivity timeout` during an active over.
- Operator audio verdict: both directions are voice, not static.

## 2026-06-04 12:15:00 EEST - Preserved raw TCH/S Block2 for group call alternation

User-reported failure:

- Group call still produced bad audio/static when multiple local subscribers tried to speak in turn.
- Existing CMCE compact `D-TX GRANTED` patch fixed an STCH/FACCH size cause, but did not prove received voice after floor handoff.

Component meaning:

- CMCE: call-control and floor/PTT authority. It decides who is allowed to speak.
- UMAC/MAC scheduler: maps voice/signalling onto assigned traffic slots and routes uplink voice back to downlink listeners.
- LMAC: burst framing. It interprets PHY bursts as STCH signalling or TCH/S speech and encodes the downlink burst sent to PHY.

ETSI clause scope:

- EN 300 392-2 clause 23.8.4.1.4: on uplink `NormalTrainSeq2`, first half is STCH; if the second half is not stolen, the BS shall interpret the second half as TCH.
- EN 300 392-2 clause 23.8.5: BS should pass U-plane TCH onward while preserving timing, ordering and half-slot pairing; if replacing a stolen half-slot, it may use C-plane Null PDU or substitution traffic.
- EN 300 392-2 clause 23.5: STCH/FACCH permits signalling capacity to be stolen from a traffic channel during an over.
- Engineering decision: do not decode a raw `Block2` half-slot as a clean 274-bit ACELP frame, because the first half was STCH and the current SAP has no BFI field. Instead, tag the 216-bit type-5 `TCH/S Block2` as raw, route it locally through UMAC, and re-emit it in the same second-half position on downlink.

Patch:

- `crates/tetra-saps/src/tmd/mod.rs`
  - `TmdCircuitDataReq` and `TmdCircuitDataInd` now carry `raw_tch_s_block: Option<PhyBlockNum>`.
- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - Uplink `NormalTrainSeq2/Block2` `TchS` is forwarded as raw 216-bit type-5 TCH/S with `raw_tch_s_block=Some(Block2)`.
  - Downlink `TchS` `blk2` with 216 bits is treated as already type-5 encoded and sent unchanged to PHY.
  - Full-slot `TchS` still decodes normally and bad CRC full-slot speech is still dropped.
- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Circuit TX queue now distinguishes normal ACELP from raw `TCH/S Block2`.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Raw `TCH/S Block2` is emitted as `STCH + TCH/S`, using an existing FACCH/STCH first half if present or a C-plane Null first half otherwise.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Raw `TCH/S Block2` is routed locally to the group/simplex/peer downlink and is not forwarded to Brew as ACELP.
- `crates/tetra-entities/src/net_brew/entity.rs`
  - Brew ignores raw half-slot payloads if one reaches it defensively.

Focused tests:

- `test_lmac_bs::bs_lmac_forwards_normal_seq2_block2_tch_s_as_raw_halfslot`
- `test_lmac_bs::bs_lmac_preserves_preencoded_raw_tch_s_block2_on_downlink`
- `test_umac_bs::test_group_ul_raw_block2_loopback_preserves_tch_s_halfslot`

Verification:

- `rustfmt --edition 2024` on touched files -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 43 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Current conclusion:

- The local component path now preserves valid `NormalTrainSeq2/Block2` TCH/S across LMAC -> UMAC -> LMAC instead of dropping it before `UMAC voice route`.
- This is clause-scoped ETSI hardening for traffic-mode half-slot TCH preservation. It is not formal certification.
- Live RF/audio is still required before claiming the group conversation issue is fixed in the test BS.

Next non-repeating execution:

1. Commit the raw TCH/S Block2 patch.
2. Build locally only with the Nexus-BS AArch64 command from build memory.
3. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup.
4. Restart the test BS.
5. Live group test on GSSI `226333`:
   - `2260616` PTT, speak, release.
   - `2260082` PTT, speak, release.
   - `2260618` PTT if available, speak, release.
   - Repeat at least three alternating turns.
6. Required pass evidence:
   - `U-TX DEMAND` and compact `D-TX GRANTED` for each speaker.
   - `UMAC floor granted` source changes to the active speaker.
   - `rx_blk_traffic: forwarding raw TCH/S Block2` or `rx_blk_traffic: decoded valid TCH/S frame` appears after each grant.
   - `UMAC voice route` appears after each grant.
   - No `PTT denied`, `NotGranted`, `does not fit STCH`, or `UL inactivity timeout` during active overs.
   - Operator audio verdict confirms each subscriber can be heard by the others, not static.

## 2026-06-04 12:16:30 EEST - Deployed raw TCH/S Block2 build to test BS

Commit deployed:

- `d96db1c fix: preserve raw group traffic half slots`

Local verification before deploy:

- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 43 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `43063625f0bd962c17b67175a0dc2e8ce32a524b65efef1f6935eb556f32c5b7`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service from pidfile PIDs before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `43063625f0bd962c17b67175a0dc2e8ce32a524b65efef1f6935eb556f32c5b7`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `17009`
  - control-service pid `17012`
  - nexus-bs pid `17014`
- Dashboard root returned Nexus-BS HTML.

Post-restart terminal state:

- `2260082` registered and affiliated to `[226333]`; RSSI about `-22 dBFS`.
- `2260616` registered and affiliated to `[226333]`; RSSI about `-23 dBFS`.
- Both `2260082` and `2260616` had BS-initiated EG3 assignment started, but T352 expired and the BS kept/fell back to `StayAlive`, so group PTT testing is not blocked by EG sleep.
- `2260618` was not present in the final concise post-deploy affiliate summary yet.
- MM still logs the pre-existing mixed `U-ATTACH/DETACH GROUP IDENTITY` reject for `group_report_response len=1 data=0` plus a GSSI list. Do not patch this under the group-audio fix unless the next log proves it is the active blocker.

Current live status:

- No post-`d96db1c` physical group PTT attempt has been captured yet.
- The BS is running the raw half-slot preservation build and is ready for live group test on GSSI `226333`.

Required next physical test:

1. `2260616` PTT on group `226333`, speak, release.
2. `2260082` PTT on group `226333`, speak, release.
3. Repeat at least three alternating turns.
4. If `2260618` appears/reattaches, add it as a third speaker.

Post-test log filter:

- `2260082|2260616|2260618|226333|U-TX DEMAND|D-TX GRANTED|FloorGranted|UMAC floor granted|rx_blk_traffic: forwarding raw TCH/S Block2|rx_blk_traffic: decoded valid TCH/S frame|UMAC voice route|FACCH stealing|preserving raw TCH/S|UL inactivity|PTT denied|NotGranted|does not fit STCH`

Pass criteria:

- Every speaker receives `D-TX GRANTED`.
- `UMAC floor granted` follows the active speaker.
- Either decoded full-slot TCH/S or raw `Block2` TCH/S reaches UMAC after each grant.
- `UMAC voice route` appears after each grant.
- No `PTT denied`, `NotGranted`, `does not fit STCH`, or active-over `UL inactivity timeout`.
- Operator audio verdict: each speaker is intelligible to the other group members.
