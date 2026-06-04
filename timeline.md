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
