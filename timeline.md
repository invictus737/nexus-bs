# Nexus-BS Project Timeline

## 2026-06-05 17:11:23 EEST - CMCE shared registry fallback for restart-affiliated groups

User report:

- Group affiliations must survive BS restart/resync and remain usable for many terminals.
- Avoid the "No Group"/"no listeners" failure mode when the central MM registry knows the GSSI but CMCE's local mirror is not yet rebuilt.

Component explanation:

- MM is Mobility Management. It owns the shared subscriber registry: registered ISSIs and their GSSI affiliations.
- CMCE is Call Control. It keeps a local mirror of subscribers for fast call/floor decisions, but after restart recovery that mirror can lag the shared registry.
- A GSSI listener check asks "is anyone affiliated to this group?". A floor-affiliation check asks "is this ISSI allowed to request PTT on this GSSI?".

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: group call setup is addressed to a GSSI with locally affiliated listeners.
- EN 300 392-2 clause 14.5.2.2.1: SwMI floor control grants/queues/rejects PTT requests from affiliated group members.
- EN 300 392-2 clause 16.4.4: SwMI may initiate registration recovery.
- EN 300 392-2 clause 16.8.1: group attach/detach state must remain coherent.
- This is restart/resync robustness hardening, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - `has_listener(gssi)` now checks the shared `SubscriberRegistry` first and falls back to CMCE's local listener count.
  - `subscriber_affiliated_to_group(issi, gssi)` now checks the shared `SubscriberRegistry` first and falls back to CMCE's local `subscriber_groups`.
  - `handle_subscriber_update` syncs MM updates into the shared registry as a defensive reconciliation path.
  - Duplicate `Register` does not clear existing shared affiliations; this preserves current tolerant CMCE semantics.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_call_uses_shared_registry_when_cmce_listener_mirror_is_empty`.
  - Seeds only the shared registry, leaves CMCE's local mirror empty, starts a group call, and confirms a second shared-registry member gets `RequestQueued` rather than release/no-listener rejection.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_call_uses_shared_registry_when_cmce_listener_mirror_is_empty --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add large restart-recovery tests for thousands of cached ISSIs all affiliated to one GSSI.
2. Add UMAC scheduler queue depth caps/coalescing for non-critical downlink backlog while preserving call-control/FACCH priority.
3. Audit global `MessageQueue` boundedness/backpressure.

## 2026-06-05 17:06:39 EEST - Large-group UMAC/CMCE robustness for thousands of affiliates

User report:

- Make group operation robust for thousands of terminals on one GSSI, not just two or three radios.
- Continue clause-scoped ETSI EN 300 392-2 hardening and do not claim formal certification.

Component explanation:

- CMCE is call control. For a group call it keeps one current PTT floor owner and decides how the next requesting ISSI receives `D-TX GRANTED`, `RequestQueued`, or `NotGranted`.
- UMAC is the MAC scheduler. It turns CMCE decisions into MAC resources, FACCH/STCH signalling, random-access ACKs, and assigned-channel state on the radio timeslots.
- RA ACK is the MAC acknowledgement that a terminal's random access was heard. When hangtime cleanup has to preserve an ACK for the next STCH, that preserved state must stay bounded under mass access.
- Energy Economy/EG7 lets terminals sleep between receive windows. While a GSSI assigned channel is active, each affiliated EG member must be suspended exactly once and resumed with a T.210 awake guard after close.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: normal group calls are listener-signalled by the GSSI while floor responses can address the requesting ISSI.
- EN 300 392-2 clause 14.5.2.2.1: SwMI-controlled group floor procedure uses `D-TX GRANTED` states for granted, queued, or not-granted PTT requests.
- EN 300 392-2 clause 21.4.3.1: MAC random-access acknowledgement is carried by the random-access flag.
- EN 300 392-2 clause 23.5.1.3.3: random access acknowledgement and reserved access grant must remain coherent.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6/T.210: BS downlink scheduling and assigned-channel operation must account for Energy Economy receive windows.
- This is clause-scoped engineering hardening and test evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `MAX_PENDING_RA_ACKS_PER_TIMESLOT = 8192`.
  - Deduplicated deferred random-access ACKs by full `TetraAddress`.
  - Bounded deferred RA ACK retention during hangtime cleanup so repeated access from thousands of affiliates cannot grow scheduler memory without limit.
  - Reworked `dl_drop_all_except_stolen` from repeated middle `Vec::remove` to a linear drain/rebuild pass, preserving STCH/FACCH stealing items while discarding/reporting other queued signalling as before.
  - Switched dropped grant lookup to `HashSet<TetraAddress>` so ACK/grant coherence checks stay linear under mass access.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_large_eg7_group_call_open_suspends_all_members_once_and_resumes_after_close` with 4096 EG7 affiliates on one GSSI plus an unrelated ISSI.
  - Asserts each affiliate gets one assigned-channel EG suspension on group call open and resumes with T.210 awake guard on close.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Strengthened `test_large_group_floor_handoff_uses_one_gssi_listener_grant` from two-speaker ping-pong to 32 distinct speakers within a 2048-member GSSI.
  - Asserts each handoff emits one individual requester grant, one GSSI listener grant, one UMAC `FloorGranted`, and no release/close.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_large_eg7_group_call_open_suspends_all_members_once_and_resumes_after_close --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor_handoff_uses_one_gssi_listener_grant --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 134 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add large restart-recovery tests for thousands of cached ISSIs affiliated to the same GSSI, covering MM recovery cache and CMCE listener restoration.
2. Add a many-queued-GSSI-resources test that asserts queue length remains resource-count bounded across mixed StayAlive/EG7 batches.
3. Continue SDS/status and LLC field-level hardening after group-call scale tests remain green.

## 2026-06-05 16:26:47 EEST - CMCE bounded group floor queue stress coverage

User report:

- Continue toward robust TETRA group call behavior for thousands of terminals, not just a few radios.
- Keep changes clause-scoped to ETSI EN 300 392-2 and do not claim formal certification.

Component explanation:

- CMCE is call control. For group calls it decides which affiliated ISSI currently owns the PTT floor.
- A queued floor requester is an MS that pressed PTT while another MS is still speaking. This stack intentionally keeps one waiter for direct handoff; additional contenders receive an explicit busy/not-granted response instead of being stored in an unbounded queue.
- UMAC should only be notified when the actual floor owner changes. Busy contenders must not receive `FloorGranted` and must not replace the queued requester.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI group floor control uses `D-TX GRANTED` with `Granted`, `RequestQueued`, or `NotGranted` state for request-to-transmit handling.
- EN 300 392-2 clause 14.5.2.1: group call remains GSSI-scoped for listener signalling while individual floor responses go back to the requesting ISSI.
- This is an engineering stress/regression test for the local one-waiter policy, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_large_group_floor_queue_is_bounded_and_busy_requesters_are_not_granted`.
  - Fixture registers 2048 affiliated ISSIs on one GSSI.
  - First non-speaker PTT receives `RequestQueued`.
  - The remaining 2046 affiliated contenders receive individual `NotGranted` responses with no UMAC `FloorGranted`, no release, and no call close.
  - When the current speaker sends `U-TX CEASED`, only the first queued requester receives the `Granted` handoff; busy contenders do not replace it.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor_queue_is_bounded_and_busy_requesters_are_not_granted --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 134 passed.
- `git diff --check` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Continue UMAC large-GSSI hardening with a per-slot readiness cache only if stress/profiling shows repeated queued GSSI elements causing `Q * N` scans.
2. Continue SDS/status and LLC remaining audit gaps with clause-scoped field-level tests.

## 2026-06-05 16:22:09 EEST - Large GSSI group-call and UMAC scheduling scalability hardening

User report:

- Group call must be robust for thousands of terminals on one GSSI, not only two or three radios.
- Continue clause-scoped ETSI EN 300 392-2 hardening without claiming formal certification.

Component explanation:

- `SubscriberRegistry` is the shared ISSI/GSSI affiliation table. It answers "which ISSIs are attached to this GSSI?" for MM, CMCE, UMAC, SDS, dashboard, and restart recovery.
- MM is Mobility Management. It restores registration/group affiliation after restart/roaming and must not scan a full group list just to check one ISSI.
- UMAC is the MAC scheduler. It turns group signalling into real downlink MAC-RESOURCE/FACCH blocks and must repeat GSSI downlinks by actual EG receive batch, not by one PDU per terminal.
- CMCE is call control. It arbitrates PTT floor changes and must keep group floor signalling GSSI-scoped for listeners even when thousands of terminals are affiliated.
- RA ACK is the MAC acknowledgement of random access; a grant tells an MS where it may continue. Under mass access these must integrate into MAC-RESOURCEs without quadratic queue churn.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1: group call setup/floor control and group-addressed `D-TX GRANTED` listener notification.
- EN 300 392-2 clause 21.4.3.1: MAC random-access acknowledgement.
- EN 300 392-2 clause 23.5.2.2.2: slot-grant response handling.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6/T.210: BS downlink scheduling must account for energy-economy receive windows.
- This is clause-scoped engineering hardening and test evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-config/src/bluestation/state.rs`
  - Added reverse GSSI membership index `group_members_by_gssi`.
  - Added `contains_group_member(gssi, issi)` and `group_member_issis(gssi)` so callers can avoid allocating/scanning a full member list for one membership check.
  - Preserved tolerant duplicate affiliation semantics while preventing unknown ISSI phantom registration.
- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Replaced runtime `group_members(...).contains(...)` checks with indexed `contains_group_member(...)` in restart recovery and attach/detach paths.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Changed GSSI delivery `covered` and `active_batch` tracking from `Vec` to `HashSet`, removing quadratic coverage checks for large groups.
  - Added no-allocation readiness iteration for GSSI listeners where only "any target listens now?" is needed.
  - Reworked ready grant/RA-ACK extraction to partition the queue in one pass instead of repeated middle `Vec::remove`.
  - Reworked grant/ACK integration to index MAC-RESOURCEs by address, so mass ACK/grant bursts collapse to one resource per ISSI without repeated linear searches.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Uses the no-allocation GSSI member iterator when suspending EG for active group circuits.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added a 2048-member GSSI floor-handoff regression with 16 back-and-forth PTT cycles: requester receives `RequestQueued`, handoff emits one ISSI `Granted` plus one GSSI `GrantedToOtherUser`, and no release/close occurs.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs` tests
  - Added 2048-member StayAlive GSSI delivery test: one group resource, no per-member repeat.
  - Added 2048-member mixed StayAlive/EG7 GSSI delivery test: repeat by receive batch, not by member; sleeping EG7 members do not get T.210 from another batch.
  - Added 2048-ISSI mass RA ACK/grant integration test: one MAC-RESOURCE per ISSI with both ACK and grant integrated.

Verification:

- `cargo fmt --package tetra-config`
- `cargo fmt --package tetra-entities`
- `cargo test -p tetra-config --lib bluestation::state --locked` -> 18 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 53 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 133 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 133 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add a bounded test for CMCE one-deep floor queue under hundreds/thousands of simultaneous requesters: first waiter queued, later contenders explicitly `NotGranted`, no unbounded state growth.
2. Continue UMAC optimization with a per-slot GSSI readiness cache if profiling or stress tests show repeated `Q * N` scans with many queued GSSI elements.
3. Deploy only after a release build if live RF validation is requested; no formal certification claim without official conformance evidence.

## 2026-06-05 14:59:46 EEST - UMAC group speaker secondary tracking without P2P regression

User report:

- Patch must take P2P/private calls into account while continuing group-call floor hardening.
- Recent live group symptoms were static/no voice on first or returning PTT, so the group fix must not break the now-working private simplex/duplex path.

Component explanation:

- CMCE is call control. It creates group/private calls and tells UMAC when a circuit is open and which ISSI has the PTT floor.
- UMAC is the MAC scheduler. It maps the current floor holder to uplink/downlink TCH/S traffic on the assigned timeslot.
- P2P/private means ISSI-to-ISSI. The circuit primary active address is an ISSI, and `active_secondary_addrs` contains the peer ISSI for the same private bearer.
- Group means GSSI-scoped. The circuit primary active address is the GSSI. The current speaker ISSI may be tracked as secondary, but that must not make UMAC treat the group bearer as a private participant list.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: private/individual call floor control and participant-scoped transmit permission.
- EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1: group call setup and SwMI floor grant with `D-TX GRANTED`.
- EN 300 392-2 clause 21.4.5: STCH `MAC-U-SIGNAL` has no SSI field, so UMAC must inherit the active speaker identity from CMCE floor state.
- EN 300 392-2 clause 23.5.2.2.7: BS assigns and marks applicable uplink/downlink traffic usage.
- This is clause-scoped engineering hardening only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-saps/src/control/call_control.rs`
  - Added `Circuit::is_primary_issi_scoped()` so callers can distinguish private/P2P circuits from group circuits even when a group circuit carries a secondary speaker ISSI.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `ul_circuit_is_private_participant_scoped(ts)`.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - `FloorGranted` now applies the strict ISSI participant guard only when the UL circuit primary active address is ISSI. This preserves P2P non-participant rejection while allowing GSSI group handoff to any CMCE-authorized group speaker.
  - Energy-economy assigned-channel suspension now de-duplicates ISSI targets per circuit, so a group speaker already covered through the primary GSSI is not suspended a second time as a secondary speaker ISSI.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Local group setup now opens UMAC with primary GSSI plus initial speaker ISSI as secondary.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
  - Network-origin group setup now uses the same primary GSSI plus speaker ISSI secondary shape.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added a regression where group `active_addr=GSSI` and `active_secondary_addrs=[first_speaker ISSI]`, then `FloorGranted(second_speaker)` must be accepted and STCH attributed to the second speaker.
  - Added a regression proving a group secondary speaker ISSI does not double-count EG suspension when that ISSI is already an affiliated member of the primary GSSI.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Strengthened local group, numeric-collision group, network-origin group, and private simplex P2P circuit-shape assertions.

Verification:

- `cargo fmt --package tetra-saps --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_grant_accepts_new_speaker_when_initial_speaker_is_secondary --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_secondary_speaker_does_not_double_suspend_energy_saving --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_ignores_floor_granted_for_non_participant_private_speaker --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_setup_sends_proceeding_connect_and_group_setup_with_allocations --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 52 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_umac_bs group --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 53 passed.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment note:

- Committed and deployed direct to the Pi test instance after local release build packaging; no binary backup was created.
- Startup evidence after deploy showed the Nexus-BS process running and `2260616`, `2260082`, and `2260618` registered plus affiliated to GSSI `226333`.
- A post-restart error scan found no `PTT denied`, `Service unavailable`, `Unit Not Attached`, or `RequestedServiceNotAvailable` lines in the fresh test log.

Next non-repeating execution:

1. Operator live test: real GSSI `226333` alternating PTT across multiple radios, then private simplex between lab ISSIs.
2. If static persists, inspect whether the failing burst lacks `UMAC voice route` after `UMAC floor granted`; do not change P2P guard semantics unless a new clause-scoped reason is identified.

## 2026-06-05 14:08:45 EEST - UMAC rejects invalid traffic timeslots before clean Pi redeploy

User report:

- Clear the BS log and restart the test BS with the latest deployed updates.
- Keep ETSI clause-scoped hardening and do not claim formal certification.

Component explanation:

- UMAC is the MAC scheduler. It turns MM/CMCE/LLC requests into TETRA downlink and uplink slot usage.
- The circuit manager is UMAC's table of active assigned traffic channels.
- In this single-carrier Nexus-BS scheduler, TS1 is the MCCH/SCH-F common-control carrier. Assigned voice traffic circuits are modelled on TS2..TS4. TS1 may still carry reserved uplink access through ACCESS-ASSIGN, but it must not be converted into an assigned voice traffic channel by a bad request.

ETSI clause scope:

- EN 300 392-2 clause 21.4.6.5: SCH/F/common-control channel context.
- EN 300 392-2 clause 23.5.2.2.7: BS slot granting and energy-economy-aware scheduling.
- This patch is fail-closed local robustness for invalid UMAC requests. It is clause-scoped engineering evidence only, not formal TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Added bounds checks for UMAC circuit timeslot access so TS0/TS5+ return/log instead of indexing out of bounds.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - `BsChannelScheduler::create_circuit` now rejects invalid timeslots and rejects TS1 traffic circuits before AACH generation.
  - Added `test_ts1_traffic_circuit_request_is_rejected_without_panic` to prove TS1 remains common control and invalid TS0 does not panic.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib ts1_traffic_circuit --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment note:

- A temporary dirty build `v0.1.55-6e783b4e-modified` was deployed direct to the Pi and restarted once to confirm the patch boots.
- Fresh restart evidence after log clear showed `2260616`, `2260082`, and `2260618` affiliated to GSSI `226333`; no `PTT denied`, `Service unavailable`, `Unit Not Attached`, or `T353` rollback appeared in the filtered startup evidence.
- Next action: commit this patch and redeploy once more so the Pi runs a clean commit build ID, then clear the log and restart again for the operator test.

## 2026-06-05 13:48:26 EEST - MM coverage-return group snapshot hardening for restart `No Group` visibility

User report:

- After BS restart, radios can appear attached but with `No Group`.
- Current live cache and latest restart log still show `2260082`, `2260616`, and `2260618` affiliated to GSSI `226333`, so the remaining hardening target is a status/dashboard-visible group gap rather than a reproduced missing CMCE listener in this restart.

Component explanation:

- MM is Mobility Management. It owns ISSI registration, remembered group affiliations, restart recovery, and the `D-LOCATION-UPDATE-COMMAND` / `U-LOCATION UPDATE DEMAND` flow.
- CMCE is call control. It consumes MM register/affiliate events so group PTT has listeners.
- Dashboard telemetry is observability. It must receive the final group list, not depend on an incremental event that may be absent when MM reuses an already cached client group.

ETSI clause scope:

- EN 300 392-2 clauses 16.9.2.8 and 16.9.3.4: a BS-commanded location update can be answered by `U-LOCATION UPDATE DEMAND` and accepted with the same update type.
- Clause 16.8.0: previously accepted persistent group identities remain valid until a real detach/replacement.
- Clause 16.4.4: SwMI may command location update/group reporting after restart.
- This patch is clause-scoped engineering hardening and dashboard consistency evidence only; it is not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - When a known MS returns after a local `D-LOCATION-UPDATE-COMMAND` without a fresh group report, MM already replays cached groups to CMCE/Brew. It now also emits a full current-group telemetry snapshot.
  - This prevents a dashboard/status `No Group` state when `client_mgr` still has the group and CMCE listener state was restored, but no new `MsGroupAttach` telemetry was generated.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added a telemetry-enabled MM regression for group-less coverage return after periodic command.
  - The test proves the final dashboard replay sees `groups=[3002]` instead of an empty group list.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_group_less_coverage_return_publishes_dashboard_group_snapshot --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 25 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to the Pi test instance; build locally only and do not create binary backups.
2. Restart test BS and read fresh log plus runtime cache.
3. Verify dashboard/WebSocket state and radio-side group state after restart for `2260082`, `2260616`, `2260618` on `226333`.
4. If a terminal screen still says `No Group` while dashboard shows `groups=[226333]`, capture exact ISSI/time and inspect whether that terminal received/ACKed the group accept/refresh on air.

## 2026-06-05 13:37:22 EEST - Deployed SDS local TSI hardening to Pi test instance

Deployment:

- Deployed direct to `chris@192.168.1.179` test instance with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Built locally only; no build was performed on the Pi and no binary backup was created.
- Running build: `Nexus-BS v0.1.55`, build `v0.1.55-29de6e15`.
- Deployed commit: `29de6e15`.
- Deployed binary SHA256: `365b08eee4e073cf23f8741e77009a888c0587ba69fe4f5c3176de1744e48838`.
- Running processes:
  - `nexus-bs-control-service --listen 127.0.0.1:9002`
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Post-deploy restart evidence:

- Runtime restart cache still contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the new build marker contains 416 lines in the startup window.
- `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
- `2260082`: `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept` for `226333`; CMCE logs `subscriber affiliate issi=2260082 groups=[226333]`.
- `2260618`: `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept` for `226333`; CMCE logs `subscriber affiliate issi=2260618 groups=[226333]`.
- `2260616`: `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept` for `226333` with EG7 information; CMCE logs `subscriber affiliate issi=2260616 groups=[226333]`.
- No `No Group`, `Unit Not Attached`, `T353`, failed transfer, `PTT denied`, service-unavailable, `FUNCTION NOT SUPPORTED`, or `TSI extension` strings appear in the new startup window.

Remaining live validation:

- No new `U-SDS-DATA` occurred after this deploy in the captured startup window, so the local-MNI TSI SDS fix is validated by local tests and ready for live WAP/browser trigger.
- Next live action: open the terminal WAP/browser home page again and confirm the previous `TSI extension addressing not supported` log does not recur for local MNI.

## 2026-06-05 13:34:45 EEST - SDS local TSI routing hardening and live restart `No Group` re-audit

User report:

- After BS restart, terminals can appear attached with `No Group`.
- Continue broader clause-scoped ETSI EN 300 392-2 hardening without claiming formal certification.

Component explanation:

- SDS is the TETRA short data/status service inside CMCE.
- SSI is the 24-bit local subscriber/group identity. TSI is the full TETRA identity: SSI plus network extension/MNI.
- MNI is MCC+MNC. In this lab the local MNI is `901/9999`, encoded as `(901 << 14) | 9999 = 14771983`.
- A local TSI with our MNI can be routed as a local SSI/GSSI. A foreign TSI must not be collapsed onto a local numeric SSI/GSSI.

ETSI clause scope:

- EN 300 392-2 clause 13.2: SDS includes individual and group user-defined/predefined messages.
- Clauses 13.3.2.1 and 13.3.2.3: TNSDS status/unitdata primitives carry called party SSI and optional called party extension; if absent at the service boundary the current network MNI is assumed.
- Clause 14.7.2.7/table 14.27: `U-STATUS` CPTI=2 carries called party SSI plus called party extension.
- Clause 14.7.2.8/table 14.28: `U-SDS-DATA` CPTI=2 carries called party SSI plus called party extension.
- Clause 14.7.3.2/table 14.33: unsupported SDS/status address forms are rejected with `CMCE FUNCTION NOT SUPPORTED`.
- Clause 18.3.5.3.1: ISSI delivery uses acknowledged L2; GSSI delivery uses unacknowledged unitdata.
- This is clause-scoped engineering evidence only, not formal TETRA certification.

Live restart audit from `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log` build `v0.1.55-2b334a02`:

- Runtime restart cache still contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Latest restart log from `13:21:03 EEST` shows `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
- `2260082`, `2260618`, and `2260616` each sent location update with GSSI `226333`; MM returned `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate ... groups=[226333]` for all three.
- No `No Group`, `Unit Not Attached`, `T353`, group refresh reject, failed transfer, `PTT denied`, or service-unavailable string was present in the latest restart log.
- Live config still has `energy_saving_mode = "eg7"` and `call_preemptive = false`.
- Conclusion: the current live evidence does not reproduce a BS-side persistent restart `No Group` state. If a terminal display still shows `No Group`, capture exact ISSI and timestamp so the terminal UI/over-air refresh path can be correlated.

Live SDS issue found:

- The same live log shows three `U-SDS-DATA` attempts rejected as `unimplemented: SDS: TSI extension addressing not supported`, followed by `CMCE FUNCTION NOT SUPPORTED`.
- That rejection is wrong for local TSI when the extension is the configured local MNI.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/sds_bs.rs`
  - Added local MNI decoding from configured MCC/MNC.
  - Replaced blanket TSI-extension rejection with a common called-party address check for `U-SDS-DATA` and `U-STATUS`.
  - Accepts TSI only when called-party extension matches the local MNI.
  - Keeps SNA, external subscriber number, DM-MS address, reserved CPTI, malformed fields, out-of-range SSI, and foreign TSI fail-closed with `CMCE FUNCTION NOT SUPPORTED`.
  - Preserves existing ambiguous ISSI/GSSI numeric collision drop behavior.
- `crates/tetra-entities/tests/test_sds_bs.rs`
  - Added local-MNI TSI tests for ISSI `D-SDS-DATA` and GSSI `D-SDS-DATA`.
  - Added local-MNI TSI tests for ISSI `D-STATUS` and GSSI `D-STATUS`.
  - Converted old TSI tests to prove foreign-MNI TSI is not rewritten to a local registered ISSI.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_sds_bs tsi --locked` -> 6 passed.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 25 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit the SDS local-TSI patch.
2. Deploy direct to the Pi test instance, building locally only and without binary backups.
3. Re-test the SDS/WAP/browser action that produced local TSI addressing; expected result is no `TSI extension addressing not supported` for local MNI.
4. If any station still visually shows `No Group`, capture exact ISSI and wall-clock time immediately.

## 2026-06-05 13:23:30 EEST - Live deploy validation for restart `No Group` hardening

Deployment:

- Deployed direct to `chris@192.168.1.179` test instance with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- No build was performed on the Pi and no binary backup was created.
- Running build: `Nexus-BS v0.1.55`, build `v0.1.55-2b334a02`.
- Deployed binary SHA256: `822514d2ac772e127c66e0f91c519c9de91e9dd1d1f9752c8bf5f9b64a16f43c`.
- Running processes:
  - `nexus-bs-control-service --listen 127.0.0.1:9002`
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live restart evidence:

- Runtime cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the new build marker showed:
  - `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
  - `2260082` sent `RoamingLocationUpdating` with `GroupIdentityLocationDemand` for GSSI `226333`; MM returned `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept`; CMCE logged `subscriber register` and `subscriber affiliate groups=[226333]`.
  - `2260618` registered and CMCE logged `subscriber affiliate groups=[226333]`.
  - `2260616` registered and CMCE logged `subscriber affiliate groups=[226333]`.
- Dashboard WebSocket snapshot after restart:
  - `2260082`: `groups=[226333]`, `energy_saving_mode=0` after T352 fallback.
  - `2260616`: `groups=[226333]`, `energy_saving_mode=7`, EG frame/multiframe present.
  - `2260618`: `groups=[226333]`, `energy_saving_mode=0` after T352 fallback.

Conclusion:

- The deployed build has all three lab terminals affiliated to `226333` after restart at MM/CMCE/dashboard state level.
- The specific group-less LU interleaving fixed in code did not occur in this fresh live restart because the observed terminals reported GSSI `226333` directly; unit tests cover the missing interleaving.
- Next live action is operator PTT validation on group and private call. If a terminal still displays `No Group`, capture the exact ISSI and timestamp immediately.

## 2026-06-05 13:18:48 EEST - MM restart group refresh survives group-less LU without masking explicit clear

User report:

- After BS restart, terminals reattached but could appear with `No Group`.
- Current hard live config uses EG7, and lab ISSIs are expected to restore GSSI `226333`.

Component explanation:

- MM is Mobility Management: it registers ISSIs, owns group affiliation state, restart recovery cache, SwMI `D-ATTACH/DETACH GROUP IDENTITY`, ACK/T353, and EG negotiation.
- CMCE is call control: it consumes MM register/affiliate events so group PTT has listeners.
- Dashboard renders telemetry cache. The render path cannot show an empty group if the browser state has `groups=[226333]`; an empty row means telemetry/group state was empty or later cleared.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may command location update and group reporting during restart recovery.
- Clauses 16.8.0 and 16.8.1: accepted/persistent group identities and SwMI-initiated group attach refresh.
- Clause 16.8.6: group attach/detach/report collision handling with other MM procedures.
- Clauses 16.8.5 and 16.11.1.3: acknowledged SwMI group refresh remains bounded by T353.
- Clause 16.10.27a: explicit group-report-complete is authoritative.
- EG7 remains covered by clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6.
- This is clause-scoped engineering evidence only, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - A group-less accepted `U-LOCATION UPDATE DEMAND` now preserves a pending restart-recovery SwMI group refresh instead of deleting it before the terminal ACKs.
  - Preservation is deliberately narrow: only restart refresh transactions with rollback/reprobe semantics, only for already-known clients, and not during hard re-registration cleanup.
  - Explicit group state in `U-LOCATION UPDATE DEMAND` still abandons the older pending SwMI refresh, so an empty complete group report can clear stale cache.
  - Rejected LU paths now abandon pending SwMI group transactions before returning: mismatched SSI/MNI, migration reject, disabled MS reject, unsupported feature reject, and whitelist reject.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added restart recovery interleaving tests for group-less LU before SwMI ACK and before T353.
  - Added explicit complete report, hard roaming re-registration, and rejected LU tests proving stale ACKs cannot re-affiliate cleared groups.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 25 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 132 passed.

Next non-repeating execution:

1. Commit and deploy direct to the Pi test instance; do not build on the Pi and do not make binary backups.
2. After restart, capture the fresh BS log from the new process only and the dashboard WebSocket snapshot.
3. Verify live cache still contains `2260082/2260616/2260618` with `226333:0:4`, the log shows SwMI group refresh/ACK or explicit group report, and dashboard state shows `groups=[226333]`.
4. Run the first group PTT/private PTT check after all three ISSIs are attached; if a station still displays `No Group`, capture exact ISSI and timestamp immediately.

## 2026-06-05 13:07:20 EEST - Live restart `No Group` audit under EG7, no protocol patch required

User report:

- After BS restart, stations appeared attached but with `No Group`.
- The live config was intentionally harder: `energy_saving_mode = "eg7"`.

Component explanation:

- MM is Mobility Management: it registers ISSIs, receives/accepts reported GSSIs, persists restart recovery cache, and assigns EG modes.
- CMCE is Call Management and Control Entity: it consumes MM `Register` and `Affiliate` events so group PTT knows which subscribers listen on a GSSI.
- Dashboard is observability: it renders MM telemetry. It is not the protocol source of truth, so live WebSocket state was checked against MM/CMCE logs.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: the SwMI may command location update and request group identity reporting after restart.
- Clauses 16.8.0, 16.8.1, 16.8.4, 16.8.5, and 16.10.27a: group identities are valid when reported/accepted or refreshed by SwMI attach/detach, with T353 bounding acknowledged SwMI group refresh.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6 remain the EG7 scheduling scope.
- This is live engineering evidence for the affected clauses, not formal TETRA certification.

Live evidence:

- Running Pi build: `Nexus-BS v0.1.55`, build `v0.1.55-113f2a91`.
- Runtime cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` contained:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the latest restart showed:
  - `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
  - `2260618` sent `U-LOCATION UPDATE DEMAND` with GSSI `226333`; MM replied with `D-LOCATION UPDATE ACCEPT` containing `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate issi=2260618 groups=[226333]`.
  - `2260082` sent `DemandLocationUpdating` with GSSI `226333`; MM replied with `D-LOCATION UPDATE ACCEPT` containing `226333`; CMCE logged `subscriber affiliate issi=2260082 groups=[226333]`.
  - `2260616` sent `ItsiAttach` with requested EG7 and GSSI `226333`; MM replied with `D-LOCATION UPDATE ACCEPT` containing EG7 start `F1/MF20` and `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate issi=2260616 groups=[226333]`; LLC/MLE later reported successful transfer for the tracked accept handle.
- Live dashboard WebSocket snapshot showed all three local stations with `groups:[226333]`:
  - `2260616`: EG7, `groups=[226333]`
  - `2260082`: StayAlive after T352 expiry, `groups=[226333]`
  - `2260618`: StayAlive after T352 expiry, `groups=[226333]`

Verification:

- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 22 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Conclusion:

- Current deployed build did not reproduce dashboard/MM/CMCE `No Group`; the active BS state has all three lab ISSIs affiliated to `226333`.
- No protocol patch was made in this checkpoint because changing MM semantics without a reproduced failing path would risk breaking the clause-scoped restart recovery behavior that is currently passing tests.

Next non-repeating execution:

1. If a terminal screen still shows `No Group` while dashboard WebSocket shows `groups=[226333]`, capture the exact ISSI and timestamp immediately after restart and inspect whether the terminal received/ACKed the `D-LOCATION UPDATE ACCEPT` carrying `GroupIdentityLocationAccept`.
2. Add a focused regression only if a real failing path is found. Candidate edges already identified:
   - EG7 response arriving before cached SwMI group-refresh ACK.
   - Group-less `U-LOCATION UPDATE DEMAND` arriving while a cached SwMI refresh is pending.
   - Cached selected GSSI beyond the first 12-GSSI scan-list refresh batch.
3. Continue broader stack hardening with private/group call, SDS, WAP, and 24/7 robustness tests; do not claim formal certification.

## 2026-06-05 12:03:36 EEST - MM segmented cached restart scan-list refresh

Context:

- Previous restart recovery patch refreshed one cached group over air, but a large cached scan-list could restore more local GSSIs than were carried in one `D-ATTACH/DETACH GROUP IDENTITY`.
- That could split BS and MS state: CMCE would think the terminal is affiliated to unsent groups while the terminal never received those group attachments after restart.

Component explanation:

- MM is Mobility Management: it owns restart recovery, group affiliation, SwMI group attach/detach, and the restart recovery cache.
- A scan-list here means multiple cached GSSIs for one ISSI. It must be refreshed in bounded over-air batches without declaring unsent groups active locally.
- CMCE is call control and depends on MM group affiliation events before it allows group PTT.

ETSI clause scope:

- EN 300 392-2 clause 16.8.0: attached group identities are valid when attached by SwMI and accepted by the MS, or when previous valid attachments remain in force.
- Clause 16.8.1: infrastructure-initiated `D-ATTACH/DETACH GROUP IDENTITY` may add groups using amendment mode and ACK request.
- Clause 16.8.5 / 16.11.1.3: each attach/detach transaction is bounded by T353.
- Clause 16.8.6: avoid colliding group-report and attach/detach procedures.
- Clauses 16.10.13, 16.10.14, 16.10.17, and 16.10.19: ACK request/type, amendment mode, lifetime, and class-of-usage fields.
- This is clause-scoped engineering hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Cached restart restore now restores only the batch that will be sent in the current SwMI `D-ATTACH/DETACH GROUP IDENTITY`.
  - Remaining cached groups are held in the pending SwMI transaction and preserved in the recovery cache while the batch is waiting for ACK.
  - When ACK arrives for a batch, MM restores and sends the next batch. If a batch is rejected or T353 expires, MM keeps the failure rollback/reprobe behavior and does not continue with remaining groups.
  - `GroupIdentityDownlink` for restart refresh now uses the cached `GroupAttachmentInfo` directly, preserving lifetime/class per group.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added a 13-GSSI cached scan-list restart test: first refresh carries 12 groups, ACK triggers the final group refresh, unsent groups are not locally restored early, and the cache retains the full scan-list across the pending transaction.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 20 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 123 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_ --locked` -> 50 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this segmented scan-list restart refresh patch.
2. Next MM restart hardening target: multi-ISSI restart recovery integration into CMCE group PTT, proving `2260082`, `2260616`, and `2260618` all re-affiliate to `226333` before first group call.
3. Remote deploy remains dependent on SSH reachability to `chris@192.168.1.179`.

## 2026-06-05 11:52:03 EEST - MM cached restart group refresh over air, EG7 ordering, and failure rollback

User report:

- After BS restart, terminals attach but display `No Group`.
- Current hard test shape includes cached group `226333`, terminals such as `2260616`/`2260618`, and BS energy saving configured as EG7.

Component explanation:

- MM is Mobility Management: it owns ISSI registration, cached restart recovery, group affiliation, SwMI group attach/detach, and energy economy negotiation.
- CMCE is call control: it depends on MM `Register`/`Affiliate` events to decide whether group/private PTT is valid.
- EG7 is an energy-saving mode: the terminal may sleep for long cycles, so group refresh must be queued before BS-initiated EG7 sleep assignment is activated.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate registration and request group identity reporting.
- Clause 16.8.0: previously accepted group identities remain valid while their lifetime remains valid.
- Clause 16.8.1: SwMI-initiated `D-ATTACH/DETACH GROUP IDENTITY` must use `group identity report = not report request`, no group-report-response IE, and may use amendment mode.
- Clause 16.8.5 and 16.11.1.3: T353 bounds attach/detach response waiting; expiry is treated as failed refresh in this implementation.
- Clauses 16.8.6, 16.10.13, 16.10.14, 16.10.17, 16.10.19, and Annex G: ACK request/type, amendment mode, attachment lifetime/class, rejected attachment handling, and procedure collision handling.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6 remain the EG scheduling scope. This is clause-scoped engineering hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Cached restart group restore now returns the restored GSSIs and sends a separate acknowledged SwMI `D-ATTACH/DETACH GROUP IDENTITY` refresh after `D-LOCATION UPDATE ACCEPT`.
  - The refresh uses amendment mode and a non-zero local downlink handle; handle `0` is accepted only as an unrouted uplink ACK fallback for this restart-refresh transaction.
  - Cached refresh no longer immediately collides with a fresh `D-LOCATION UPDATE COMMAND(group_identity_report=1)` in the same unsolicited ITSI attach cycle.
  - If the MS rejects the refreshed group or T353 expires, MM rolls back the provisional cached affiliation, persists the bare ISSI cache, and requests a fresh group report.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs` and group FSM/timers
  - Real group floor grants send group-addressed FACCH/STCH `D-INFO` with reset T310 while preserving `UlDlAssignment::Both`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 19 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 122 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_ --locked` -> 50 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 129 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM/CMCE patch.
2. Deploy direct when `chris@192.168.1.179` is reachable: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. After restart, verify terminals `2260082`, `2260616`, and `2260618` show group `226333`, not `No Group`.
4. In fresh logs, confirm cached restart restore is followed by `D-ATTACH/DETACH GROUP IDENTITY`, ACK handling, and no duplicate affiliate event.
5. Retest group PTT under EG7: first PTT after restart should not be denied because CMCE should already have the restored MM affiliate.

## 2026-06-05 11:18:02 EEST - Restart `No Group` remains deploy/log blocked; LMAC fail-closed verified locally

User report:

- After BS restart, stations attach but show `No Group`.
- The test setup is intentionally harsher now because the BS config is expected to run `energy_saving_mode = "eg7"`.

Component explanation:

- MM is Mobility Management: it owns terminal registration, group affiliation, restart recovery, and energy economy negotiation.
- CMCE is call control: it consumes MM `Register`/`Affiliate` events so group calls know which ISSIs are valid listeners.
- Dashboard is observability: it must display MM/CMCE state without losing group events that race registration events.
- LMAC is the lower MAC/channel-coding edge: it turns logical MAC blocks into PHY channel-coded bits and must not encode unsupported logical channels as the wrong channel type.

Current local finding:

- The restart `No Group` fix is already present in local history:
  - `f02371a fix: recover restart candidate groups before eg`
  - Current HEAD is `a5d8b9e docs: record restart no group validation`.
- The relevant local MM/dashboard behavior is already covered:
  - restart candidates are captured before registration removes them from the recovery map;
  - group-less restart candidate self-attach restores cached GSSI locally when present;
  - `D-LOCATION UPDATE COMMAND(group_identity_report=1)` is queued before configured BS-initiated EG7 request;
  - explicit empty complete group reports remain authoritative and clear cached groups;
  - dashboard preserves group attach/snapshot events that arrive before registration events.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate registration and request group identity report after restart.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a: reported groups and explicit complete empty reports are authoritative for affiliation state.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7 and 23.7.6/T.210: EG7 scheduling must not make the terminal sleep before the BS requests the group report.
- LMAC fail-closed work is scoped to implemented channel coding only: unsupported TCH/2.4, TCH/4.8, TCH/7.2 and linearization channels are dropped with warnings instead of being encoded through TCH/S or ordinary C-plane paths.
- This is clause-scoped engineering evidence, not formal TETRA certification.

Verification run:

- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 16 passed.
- `cargo test -p tetra-entities --lib dashboard_ --locked` -> 8 passed.
- `cargo test -p tetra-entities --lib test_unsupported_logical_channels_fail_closed --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/log status:

- Commit created after verification:
  - `e7afd48 fix: fail closed unsupported lmac channels`
- `ssh chris@192.168.1.179 ...` timed out on port 22 while trying to read `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` and the full `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log` from the latest restart.
- Live restart validation and direct deploy are still blocked by SSH reachability, not by local code/test failures.

Next non-repeating execution:

1. When SSH returns, deploy directly with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`; do not compile on the Pi and do not create binary backups.
2. Confirm remote build id is current HEAD and read the cache:
   - expected: `2260082 226333:0:4`, `2260616 226333:0:4`, `2260618 226333:0:4` or equivalent class values.
3. Read the fresh full log from the latest restart and compare terminal/dashboard `No Group` with MM/CMCE `subscriber affiliate` evidence before making another MM patch.

## 2026-06-05 09:49:05 EEST - Group restart grant and raw Block2 same-burst floor-release hardening

Live problem targeted:

- Latest restart log showed group PTT/service-unavailable symptoms after a previous same-GSSI group call was still draining `D-RELEASE`.
- Concrete log pattern: new `U-SETUP` for GSSI `226333` from `2260616` arrived while the stale call release was pending; old CMCE logic rejected it with `RequestedServiceNotAvailable`.
- User also reported intermittent static/no voice on the other station during group/PTT transitions. UMAC/LMAC audit found a narrow race where `NormalTrainSeq2` Block1 STCH can carry `U-TX CEASED` while Block2 is still accepted as raw TCH/S before CMCE floor release reaches UMAC.
- EG7 note: the current latest restart log shows `2260616` requested `Eg7`, but Nexus-BS accepted/configured `Eg3` on air. True EG7 field testing requires the BS energy-saving config to assign EG7; terminal-side preference alone is not what the current SwMI advertises.

Components, simple technical meaning:

- CMCE: call control and PTT/floor logic. It decides `D-CALL PROCEEDING`, `D-CONNECT`, `D-SETUP`, `D-TX GRANTED`, `D-TX CEASED`, and `D-RELEASE`.
- UMAC: MAC scheduler/media router. It receives voice from LMAC, applies active circuit/floor state, and schedules DL TCH/S or FACCH/STCH signalling.
- LMAC: lower MAC burst classifier. It decides whether a burst half is `STCH` signalling or `TCH/S` speech before passing it upward.
- MM/EG: mobility/energy-economy negotiation. It assigns StayAlive/EG1..EG7 and the frame/multiframe where sleeping terminals should listen.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: normal group call setup after a later `U-SETUP`.
- EN 300 392-2 clause 14.5.2.3: old group call release with `D-RELEASE` must drain independently.
- EN 300 392-2 clause 23.5: FACCH/STCH stealing and traffic/signalling half-slot distinction.
- EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5: valid non-stolen TCH/S half-slot timing/position must be preserved; stale or floor-released media must not be fabricated as clean speech.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, 23.7.6, and T.210 remain the active scope for EG3/EG7 scheduler safety. This is engineering hardening, not formal certification evidence.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Same-GSSI `U-SETUP` now ignores stale pending-release calls when deciding whether an active call already exists.
  - If only a pending-release call exists for the same GSSI, CMCE starts a fresh group call instead of sending service-unavailable `D-RELEASE`.
  - The stale pending release remains tied to its original assigned channel and closes only when its reporter/guard completes.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Raw `NormalTrainSeq2` Block2 TCH/S is now deferred in UMAC until same-burst STCH/CMCE floor-control has drained.
  - `FloorReleased`, `FloorGranted`, `CallEnded`, circuit close, and replacement open discard deferred stale raw media before it can enter the DL scheduler.
  - Valid raw Block2 is still preserved and emitted after the deferral window when the UL/DL circuit is active, not in hangtime, and speaker/peer routing still matches.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added/regressed same-GSSI pending-release restart coverage with fresh call id, fresh traffic allocation, no service-unavailable `D-RELEASE`, and old release closing only its old slot.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs raw_block2 --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_release_pending --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 129 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_mm_bs energy --locked` -> 29 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Run final focused tests once more after commit/deploy build.
2. Deploy direct with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest group GSSI `226333`: first PTT immediately after prior call release/hangtime must grant on first try, not second try, and no `RequestedServiceNotAvailable` should appear for same-GSSI replacement setup.
4. Retest static/no-voice: alternating group PTT should not transmit raw Block2 after `U-TX CEASED`/`D-TX CEASED`.
5. If true EG7 field test is desired, set the BS config to assign EG7 and verify attach accept advertises `EnergySavingInformation { energy_saving_mode: Eg7, ... }`; otherwise current on-air behavior remains EG3 despite terminal request.

## 2026-06-05 01:23:50 EEST - LMAC first private-simplex TCH/S recovery before traffic marker

Live problem targeted:

- User reported current private simplex P2P is broken.
- Fresh live grep on running build `v0.1.55-bcc5e08b` did not show any decoded private-call CMCE sequence (`U-SETUP`, `D-CONNECT`, `U-TX DEMAND`, `D-TX GRANTED`) after the report.
- The same log did show uplink `NormalTrainSeq` fullslot bursts without higher-layer P2P decode, which is consistent with first traffic bursts arriving while the BS-side lower MAC has not yet marked that UL timeslot as `Tp`, or with a terminal still transmitting on a traffic slot the current BS state has not opened.

Components, simple technical meaning:

- LMAC: lower MAC classifier. It receives demodulated bursts from PHY and decides whether they are control signalling (`SCH/F`, `SCH/HU`, `STCH`) or speech traffic (`TCH/S`).
- UMAC: upper MAC circuit/router. It accepts speech only when CMCE has opened a matching circuit, so an LMAC fallback cannot create a call by itself.
- CMCE private simplex: call-control state machine that opens the private call and owns PTT floor permission. This patch does not change CMCE grants or release rules.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: private simplex calls use SwMI-controlled transmit permission; initial transmit permission from setup must still allow valid speech once the assigned channel is opened.
- EN 300 392-2 clause 23.5.2.2.1: MAC resource/channel allocation transitions the MS to the assigned physical channel.
- EN 300 392-2 clauses 23.8.3, 23.8.3.2 and 23.8.5: TCH/S speech frames must not be converted from bad CRC or partial conditions into clean audio; a non-stolen second half-slot must preserve its half-slot timing/position.
- This is clause-scoped engineering hardening and RF-retest preparation, not formal ETSI/TETRA certification evidence.

Patch implemented:

- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - `rx_blk_control` now returns whether a valid control block was actually forwarded.
  - If a `NUB` on an as-yet `Unallocated` UL slot fails as control, LMAC retries only the TCH/S-compatible cases:
    - `NormalTrainSeq1 + Both` full-slot TCH/S.
    - `NormalTrainSeq2 + Block2` non-stolen raw TCH/S half-slot.
  - Full-slot fallback still requires the TCH/S speech CRC to pass; bad CRC remains dropped so static is not forwarded as clean speech.
  - Raw Block2 fallback is still handed to UMAC, where it is dropped unless a matching active circuit exists.
- `crates/tetra-entities/tests/test_lmac_bs.rs`
  - Added first-burst fallback coverage for full-slot TCH/S before the UL traffic marker is present.
  - Added first-burst fallback coverage for raw `NormalTrainSeq2` Block2 TCH/S before the UL traffic marker is present.
  - Added bad-CRC unknown-channel fallback regression so corrupt speech is not emitted as audio.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added private simplex initial-floor media regression proving UMAC routes the first TCH/S burst after `CallControl::Open` without requiring an extra `FloorGranted`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 63 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Run `git diff --check`, commit, and deploy direct to `/home/chris/nexus-bs-v0.1.55-test`.
2. Retest private simplex `2260616 <-> 2260618`.
3. Expected live evidence for the targeted race: if the terminal sends immediate speech, log should now show `LMAC: retrying undecoded NUB as candidate TCH/S`, then `rx_blk_traffic: decoded valid TCH/S frame` or raw Block2 forwarding, followed by `UMAC voice route`.
4. If no `U-SETUP`/`U-TX DEMAND` appears and only raw `NormalTrainSeq` continues, diagnose stale terminal traffic-channel state/recovery separately instead of changing CMCE floor logic.

## 2026-06-04 23:34:48 EEST - LLC inbound duplicate guard bounded by T.251/N.252

Live problem targeted:

- User reported group-call tests with visible terminal-side `PTT denied`.
- Live BS log review did not show CMCE sending a floor denial in the sampled window; repeated group `U-TX DEMAND` events were answered with `D-TX GRANTED`.
- The same log did show a lower-layer signalling fault:
  - `23:28:32.289`: `LLC: suppressing duplicate inbound BL-DATA/BL-ADATA N(S)=1 for SSI 2260618 endpoint 0; ACK remains scheduled`.
  - This occurred many seconds after prior `2260618` signalling, so the old unbounded receive duplicate memory could suppress a new CMCE PDU before it reached call control.

Components, simple technical meaning:

- LLC: logical link control between MAC and MLE/CMCE/SDS. It acknowledges BL-DATA/BL-ADATA and prevents duplicate service-user delivery when a peer retransmits because its ACK was lost.
- `N(S)`: one-bit basic-link sequence number for acknowledged data.
- `inbound_receive_seq`: per-terminal/per-endpoint memory of the last valid inbound `N(S)`.
- Patch scope: keep duplicate suppression for short retransmission windows, but stop treating the same `N(S)` as a duplicate forever.

ETSI clause scope:

- EN 300 392-2 clause 22.3.2.3: acknowledged BL-DATA/BL-ADATA uses `N(S)`/`N(R)` and must ACK valid inbound data; BL-ADATA handles ACK first and DATA second.
- EN 300 392-2 clause 22.3.2.3 note 3: numbering alone does not guarantee safe duplicate suppression.
- EN 300 392-2 Annex A.1: T.251 is counted in downlink signalling frames; default T.251 is 4 signalling frames.
- EN 300 392-2 Annex A.2: N.252 defines the maximum retransmissions; local configured value is 3, giving a conservative duplicate guard of `(3 + 1) * 4 = 16` downlink signalling frames.
- This is clause-scoped robustness hardening, not formal ETSI certification evidence.

Patch implemented:

- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - `ReceiveSeqState` now stores `last_ns`, `received_at`, and `ack_timeslot`.
  - Added a duplicate-suppression horizon based on `(N.252 + 1) * T.251`.
  - Prunes expired receive-sequence entries before duplicate comparison.
  - Does not refresh the duplicate window when suppressing a duplicate, so repeated stale frames cannot extend suppression indefinitely.
  - Keeps duplicate BL-DATA/BL-ADATA ACK scheduling intact.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_llc_bs inbound --locked` -> 6 passed.
- `cargo test -p tetra-entities llc::llc_bs_ms::tests::inbound_duplicate_guard_expires_after_full_retransmission_horizon --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 80 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment result:

- Committed as `8452f9b fix: bound LLC inbound duplicate suppression`.
- Deployed direct to testing with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Running BS build: `v0.1.55-8452f9b2`.
- Deployed binary SHA-256: `f385e880db8df5cd5e79541d004c616cf9583f1b8d4027a424eadf6fac01cc08`.
- Post-start log showed `2260082`, `2260618`, and `2260616` registered and affiliated to `226333`.
- Post-start log also showed the new bounded behaviour:
  - `LLC: expiring inbound duplicate guard for SSI 2260082 endpoint 0 N(S) 1`.
- No immediate `PTT denied`, `RequestedServiceNotAvailable`, `Service unavailable`, or `Unit Not Attached` lines appeared in the post-deploy filter.

Next non-repeating execution:

1. Retest group alternating PTT with `2260616`, `2260618`, and `2260082` on GSSI `226333`.
2. Expected live evidence: no stale `LLC: suppressing duplicate inbound ...` for new group-call control after the T.251/N.252 horizon; CMCE should receive the control PDU and either grant or explicitly log any real floor denial reason.

## 2026-06-04 23:21:39 EEST - Dashboard CPU model detection across boards

Problem targeted:

- Dashboard System tab showed `unknown (4 cores)` on the live test board.
- Live board evidence:
  - `/proc/cpuinfo`: `CPU implementer=0x41`, `CPU part=0xd03`, `CPU architecture=8`, 4 processors.
  - `/proc/device-tree/model`: `Raspberry Pi Zero 2 W Rev 1.0`.
  - `/proc/device-tree/compatible`: `raspberrypi,model-zero-2-w`, `brcm,bcm2837`.
  - `cpuinfo_max_freq`: `1000000`.
  - `uname -m`: `aarch64`.

Components, simple technical meaning:

- Dashboard `/api/system`: HTTP endpoint that feeds the System tab.
- CPU descriptor parser: converts Linux kernel CPU identity fields into a readable hardware string such as `Broadcom Cortex-A53 1GHz 64-bit`.
- This is observability/dashboard work, not TETRA air-interface signalling and not formal conformance evidence.

Patch implemented:

- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Replaced single-line `/proc/cpuinfo` lookup with a board-aware parser.
  - Uses `/proc/cpuinfo`, device-tree `model`/`compatible`, `cpufreq` max frequency, and `uname -m`.
  - Maps ARM implementer/part IDs to core names including Cortex-A53/A55/A72/A76/A78/A510/A710, Neoverse, Broadcom Brahma, Qualcomm Kryo/Krait/Falkor, and NVIDIA Denver/Carmel.
  - Keeps x86 `model name` intact while adding architecture width when available.
  - Added tests for the live Raspberry Pi Zero 2 W case and x86 model-name preservation.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities net_dashboard::server::tests::cpu_descriptor --locked` -> 2 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment result:

- Committed as `5d4b888 fix: detect dashboard CPU model across boards`.
- Deployed direct to testing with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Running BS build: `v0.1.55-5d4b888c`.
- Deployed binary SHA-256: `a251ba3a82d5b5e02054254dd1e27dfe1820efe6c93ae8c4f39a1de39e2bc244`.
- Live `/api/system` verification:
  - `cpu_model`: `Broadcom Cortex-A53 1GHz 64-bit`.
  - `cpu_cores`: `4`.
  - UI should render `Broadcom Cortex-A53 1GHz 64-bit (4 cores)`.
- Post-start radio state still showed `2260082`, `2260618`, and `2260616` registered and affiliated to `226333`.

## 2026-06-04 23:09:56 EEST - Private simplex MXP600 last-speaker release guard

Live problem targeted:

- User retested `2260616 -> 2260618`, let `2260618` speak last, then hung up with red key on `2260616`.
- Deployed test BS was already `Build: v0.1.55-5f03000c`.
- Live log evidence:
  - `22:54:54` private simplex `call_id=4` opened from `2260616` to `2260618`; initial floor holder `2260616`.
  - `22:54:59` `2260618` obtained the private simplex floor.
  - `22:55:04` `2260618` sent `U-TX CEASED`; CMCE tail-drained and sent `D-TX CEASED`.
  - `22:55:05` `2260616` sent `U-DISCONNECT`; because `floor_holder` had already been cleared to `None`, the previous code fell back to peer `D-DISCONNECT`.
  - `22:55:05.339` `2260618` sent `U-RELEASE`, then `22:55:23` re-attached, matching the reported MXP600 soft reboot.

Components, simple technical meaning:

- CMCE/CC-BS: private-call control. It tracks who is in the call, who has PTT floor, and which release PDU is sent.
- `floor_holder`: current simplex private speaker, if someone is actively transmitting.
- `last_floor_holder`: new retained memory of the last simplex private speaker after `U-TX CEASED`.
- UMAC: assigned-channel bearer/router. It stays open until CMCE release reporters prove the release messages were sent.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: private simplex transmit permission is controlled by SwMI with `D-TX GRANTED`/`D-TX CEASED`.
- EN 300 392-2 clause 14.5.1.3.1: either user may initiate disconnection; the MS sending `U-DISCONNECT` waits for `D-RELEASE`; the SwMI may inform the other MS by either `D-DISCONNECT` or `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: `D-DISCONNECT` expects `U-RELEASE`; `D-RELEASE` expects no response.
- EN 300 392-2 clause 23.8.5 still motivates the bounded speech-bearer tail drain before peer clear signalling. This is clause-scoped engineering hardening plus a Motorola compatibility guard, not formal certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Added `last_floor_holder` to `IndividualCall`.
  - Added `set_floor_holder`, `clear_floor_holder`, and `peer_is_current_or_last_floor_holder` helpers so floor transitions retain the last simplex speaker.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex `U-DISCONNECT` now sends peer `D-RELEASE` if the peer is the current or most recent floor holder, even if it already sent `U-TX CEASED`.
  - Passive-peer cases may still use the existing `D-DISCONNECT -> U-RELEASE` path after tail drain.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Replaced direct floor mutations with the new helpers so queued handoff, normal floor grant, inactivity recovery, and setup activation all keep `last_floor_holder` coherent.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added field regression test for exact live sequence: `2260616 -> 2260618`, peer gets floor, peer sends `U-TX CEASED`, caller hangs up, peer receives tail-drained `D-RELEASE` and no `D-DISCONNECT`.

Verification:

- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_releases_mxp600_peer_after_peer_ceased_last_floor --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 63 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 125 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment:

- Commit: `89404b9 fix: release last private simplex speaker`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot path; no build on `chris@192.168.1.179`.
- Remote build banner: `Build: v0.1.55-89404b98`.
- Remote binary SHA-256: `fdd966e670a3bd1895880537566e7ae930fb0688a377b59e2ea8de18b4746fcf`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Post-start log showed `2260618`, `2260616`, and `2260082` registered and affiliated to GSSI `226333`.

Next non-repeating execution:

1. Retest the exact field case: `2260616 -> 2260618`, let `2260618` speak last and release PTT, then hang up from `2260616`.
2. Expected live evidence: prompt `D-RELEASE` to `2260616`, no `D-DISCONNECT` to `2260618`, tail-drained `D-RELEASE` to `2260618`, no `U-RELEASE` required from `2260618`, no MXP600 reboot.
3. If reboot still occurs, capture whether any peer-directed `D-DISCONNECT` remains in the release window; do not patch further without fresh log evidence.


## 2026-06-04 22:32:27 EEST - Private simplex peer-floor hangup tail-drain

Problem targeted:

- User report: private simplex `2260616 -> 2260618` works for voice, but when `2260616` hangs up with the red key, the peer Motorola MXP600 `2260618` soft reboots.
- The current deployed build after the previous patch is `Build: v0.1.55-a3bc4078`; the sampled post-deploy log did not contain a fresh private-call attempt, only startup/register/affiliate activity.
- Audit gap found in the current code: peer-facing `D-DISCONNECT` was tail-drained only when the disconnecting MS was the current simplex floor holder. If the peer MXP600 was the current/last floor holder and the caller hung up, Nexus-BS could still send `D-DISCONNECT` to that peer immediately.

Components, simple technical meaning:

- CMCE/CC-BS: call-control state machine. It decides private-call setup, PTT floor ownership, `D-RELEASE`, `D-DISCONNECT`, and when the traffic circuit may close.
- UMAC: assigned traffic-channel owner/router. It keeps the simplex bearer open while CMCE drains/release-confirms the call.
- Tail-drain guard: a short bounded wait before peer-facing clear signalling so the speech bearer is not cleared in the same instant as recent TCH/S traffic.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex individual calls use controlled `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and `D-TX CEASED`; no unsolicited peer grant is introduced.
- EN 300 392-2 clauses 14.5.1.3.1 and 14.5.1.3.3: the MS that sends `U-DISCONNECT` receives `D-RELEASE`; the peer leg is cleared with `D-DISCONNECT` and answers with `U-RELEASE`.
- EN 300 392-2 clause 14.7.1.6: `D-DISCONNECT` expects `U-RELEASE`.
- EN 300 392-2 clause 23.8.5 gives an N-1 traffic-slot tail-bit rule for N=4/8 circuit-mode data. Applying the same short N=4-equivalent guard to TCH/S speech remains a bounded Motorola/bearer compatibility guard, not a formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex `U-DISCONNECT` now tail-drains peer-facing `D-DISCONNECT` whenever any simplex floor holder is active, not only when the disconnecting MS is the floor holder.
  - Prompt `D-RELEASE` to the MS that pressed red is still immediate.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added reusable helpers for field-ISSI private-call setup.
  - Added `LAB_ISSI_MXP600 = 2260618`.
  - Added regression test for `2260616 -> 2260618`: 2260618 obtains the private simplex floor, 2260616 hangs up, `D-RELEASE` goes promptly to 2260616, and `D-DISCONNECT` to 2260618 appears only after tail-drain.
  - Updated the mirrored called-party disconnect test so the floor-holding peer is also tail-drained before `D-DISCONNECT`.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_tail_drains_when_mxp600_peer_holds_floor --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 62 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 124 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment:

- Commit: `c01572f fix: tail-drain private peer-floor disconnect`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote build banner: `Build: v0.1.55-c01572fb`.
- Remote binary SHA-256: `92ca23ac132c508a12776c3759bbbf1603899c782600b4326419b922f0e67f31`.
- Post-start log: `2260082`, `2260618`, and `2260616` registered and affiliated to group `226333`.

Next non-repeating execution:

1. Retest exact Motorola case: `2260616 -> 2260618`, make 2260618 talk last if possible, then hang up on `2260616`.
2. Expected live log: prompt `D-RELEASE` to 2260616, no immediate `D-DISCONNECT` to 2260618, delayed `D-DISCONNECT` after tail-drain, peer `U-RELEASE`, no fallback timeout, no MXP600 reboot.
3. If a reboot still happens, inspect whether `2260618` was the `D-DISCONNECT` recipient or whether it sent/failed to send `U-RELEASE` before changing sequencing again.

## 2026-06-04 21:54:29 EEST - Patched private simplex hangup No Answer release acknowledgement

User symptom:

- Motorola showed `No answer` at the end of a private simplex call.
- The live test BS was already running `Build: v0.1.55-a26e3a23`, which had `D-DISCONNECT` response capability and required peer `U-RELEASE`, but the current post-restart log sample did not contain a fresh private-call release attempt.

Component in simple technical terms:

- CMCE/CC-BS is the call-control state machine for private and group calls.
- `U-DISCONNECT` is the terminal request to end a private call.
- `D-RELEASE` is the BS response expected by the terminal that requested the end of the call.
- `D-DISCONNECT` is the BS request to clear the other terminal; that peer answers with `U-RELEASE`.
- UMAC stays responsible for keeping the assigned traffic channel open until the required release messages are reported transmitted.

ETSI clause scope checked:

- EN 300 392-2 clause 14.5.1.3.1 says an MS that sends `U-DISCONNECT` waits for `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3 says an MS receiving `D-DISCONNECT` responds with `U-RELEASE`, while `D-RELEASE` expects no response.
- EN 300 392-2 clause 14.7.1.6 defines `D-DISCONNECT` with response expected `U-RELEASE`.
- EN 300 392-2 clause 14.7.1.9 defines `D-RELEASE` as the infrastructure release message with no response expected.
- This is clause-scoped engineering alignment only, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - A valid active private-call `U-DISCONNECT` now sends prompt `D-RELEASE` to the requesting MS before/alongside peer clearing.
  - Peer clearing still uses `D-DISCONNECT` and waits for peer `U-RELEASE`; peer `U-DISCONNECT` is still not treated as the acknowledgement.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added pending prompt-release ACK tracking so the traffic circuit closes only after the initiator `D-RELEASE` is transmitted and peer clearing completes.
  - Fallback paths for lost/discarded `D-DISCONNECT` now avoid duplicating `D-RELEASE` to the initiator; they release only the remaining peer leg when the prompt ACK was already sent.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Timer drain now includes pending private disconnect release ACKs.
  - Peer `U-RELEASE` timeout uses the new peer-leg fallback release path.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated private-call release tests to assert prompt initiator `D-RELEASE`, peer `D-DISCONNECT` with `UlDlAssignment::Both`, no duplicate initiator release after peer `U-RELEASE`, and no UMAC close before release reporters complete.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 61 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 123 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `5acff30 fix: ack private disconnect initiator promptly`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `a74b39670e1af2bd0f09e7a2fbfd518c2c1375b69fc06632d11fe8db01bf5607`
- Startup banner reports `Build: v0.1.55-5acff30d`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Post-restart register/affiliate observed for `2260082`, `2260618`, and `2260616` on GSSI `226333`.

Next live validation:

- Retest private simplex between `2260082` and `2260616`.
- Expected live evidence: after one terminal hangs up, logs show `U-DISCONNECT`, prompt `D-RELEASE` to that ISSI, `D-DISCONNECT` to the peer with assigned channel response capability, peer `U-RELEASE`, and no `Pending individual D-DISCONNECT timed out`.

## 2026-06-04 21:01:37 EEST - Fixed private simplex first-PTT floor inversion for hook setup

User symptom:

- Private simplex call first PTT did not pass voice while PTT was held; a second PTT worked.
- Live private-call log showed the concrete bad sequence:
  - `2260616 -> 2260618` `U-SETUP` with `hook_method_selection=true`, `simplex_duplex_selection=false`, and `request_to_transmit_send_data=true`.
  - Nexus-BS then sent `D-CONNECT` to the caller with `transmission_grant=GrantedToOtherUser`.
  - Nexus-BS sent `D-CONNECT-ACKNOWLEDGE` to the called MS with `transmission_grant=Granted`.
  - CMCE recorded `initial floor_holder = ISSI 2260618`.
  - Later `2260616` sent `U-TX DEMAND`, Nexus-BS granted floor to `2260616`, and voice route became active.

Component in simple technical terms:

- CMCE/CC-BS is the call-control brain. In a private simplex call it decides which terminal gets the first transmit floor in `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE`.
- UMAC is the traffic-channel router. It uses the CMCE `CallControl::Open.active_addr` speaker to decide whose uplink TCH/S speech is valid and should be looped to the listeners on the shared simplex bearer.
- The bug was in CMCE's interpretation of one setup bit, not in encryption, WAP, or RF.

ETSI clause scope checked:

- EN 300 392-2 clause 14.5.1.2.1 says the SwMI fully controls private-call transmit permission.
- For on/off-hook signalling, normal operation gives the called MS permission to transmit, but if the calling MS sets the `request to transmit` bit in `U-SETUP`, the calling MS is asking for transmit permission.
- For direct setup signalling, normal operation gives the calling MS permission to transmit; the same bit allows the called user application to request permission first, but it is not an automatic grant to the called MS.
- EN 300 392-2 table 14.80 defines `transmission_grant`: granted, not granted, queued, or granted to another user.
- This is clause-scoped engineering alignment only, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Initial private simplex floor selection now interprets `request_to_transmit_send_data` by setup method:
    - hook/on-off signalling with bit set -> caller receives initial floor;
    - hook/on-off signalling with bit clear -> called MS receives normal initial floor;
    - direct setup -> caller remains initial floor; the bit only permits called-first request flow, not automatic called grant.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Updated the field comment so future patches do not reintroduce the inverted interpretation.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added/updated focused tests for hook setup with and without request-to-transmit:
    - `hook=true, request=true` keeps the calling MS as initial UMAC speaker and sends caller `D-CONNECT Granted`;
    - `hook=true, request=false` keeps the called MS as normal initial speaker and sends caller `D-CONNECT GrantedToOtherUser`.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_hook_setup_request_to_transmit_keeps_calling_ms_initial_floor --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 121 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `9469dd2 fix: align private simplex initial floor`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `7eafdfa7df28472caee5f776411b56fdeffb39b0d0b7dd0e173605bf7f2f95cb`
- Startup banner reports `Build: v0.1.55-9469dd20`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Post-restart register/affiliate observed for `2260618`, `2260082`, and `2260616` on GSSI `226333`.

Next live validation:

- Deploy this patch to the test BS.
- Retest private simplex while holding PTT on the calling terminal.
- Expected evidence for the first call attempt:
  - `D-CONNECT` to caller has `transmission_grant=Granted`;
  - `D-CONNECT-ACKNOWLEDGE` to called MS has `transmission_grant=GrantedToOtherUser`;
  - `Simplex P2P initial floor_holder` matches the caller;
  - `UMAC voice route` appears during the first held PTT, without waiting for a second `U-TX DEMAND`.

## 2026-06-04 20:08:31 EEST - Extended assigned-channel usage marker to group floor cease/interrupt

Live evidence after `33ef3ca`:

- The deployed build no longer showed `PTT denied`, `NotGranted`, or `RequestedServiceNotAvailable` in the sampled group PTT window.
- Group floor grants were accepted at CMCE/UMAC level:
  - `2260082` and `2260618` each received `FSM -> D-TX GRANTED (individual, Granted)` on `call_id=4`.
  - `UMAC floor granted` followed the active speaker ISSI.
- Remaining defect moved lower: repeated `UL inactivity timeout on ts=2` showed the BS granted floor but did not receive/accept valid uplink traffic consistently after some grants.
- The live log also showed `D-TX CEASED` FACCH/STCH still carried `chan_alloc.usage=None`, while `D-TX GRANTED` had already been fixed to carry `usage=Some(4)`.

Component in simple technical terms:

- `D-TX CEASED` tells group listeners that the current speaker stopped and the floor is released.
- `D-TX INTERRUPT` withdraws a current speaker during supported pre-emption.
- Both are CMCE floor-control messages carried by UMAC as MAC-RESOURCE/STCH on the same assigned traffic channel.
- If their STCH wrapper lacks the active usage marker, a terminal may treat the signalling as not belonging to the traffic circuit it is monitoring.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 defines SwMI group floor control, including granting, queueing, denial, interruption, and floor release while MSs remain in `CALL-ACTIVE`.
- EN 300 392-2 clause 23.8.1 says TCH/STCH on the assigned channel uses the corresponding traffic usage marker.
- EN 300 392-2 clause 23.8.2.3.1 says transmit traffic needs both CC authorization and an applicable uplink traffic usage marker.
- EN 300 392-2 clause 23.8.4.2 permits downlink C-plane signalling on STCH using MAC-RESOURCE.
- This remains clause-scoped engineering alignment, not formal conformance certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - `send_d_tx_ceased_facch` now accepts `usage` and sends DL-only FACCH/STCH with `chan_alloc.usage=Some(usage)`.
  - `send_d_tx_interrupt_facch` now accepts `usage` and sends DL-only FACCH/STCH with `chan_alloc.usage=Some(usage)`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Passes active call usage into floor cease and pre-emption interrupt paths.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - UL-inactivity-forced floor release now preserves the call usage marker in `D-TX CEASED`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - `D-TX CEASED` and `D-TX INTERRUPT` tests now assert the actual circuit timeslot, usage marker, and DL-only direction.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 118 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `dcb542d fix: preserve group floor signalling usage marker`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `3a075b027925d89c986032ca82ab06514e0be38cb9ef652fae2e1b49578901b1`
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-dcb542dd`.
- `2260616` and `2260082` registered and affiliated to `226333`.
- Operator clarified that the active radios are now both on GSSI `226333`, so the next defect isolation must stay on group floor/traffic handling instead of treating the current issue as a group mismatch.

Next live validation:

- Test BS is already running `v0.1.55-dcb542dd`.
- Retest alternating group PTT on GSSI `226333`, preferably first between `2260616` and `2260082`.
- Required evidence: no terminal `PTT denied`, no BS `NotGranted` unless another MS is truly active, `D-TX CEASED` FACCH/STCH shows `usage=Some(4)`, no repeated `UL inactivity timeout` immediately after floor grants, and audio is intelligible.
- If CMCE grants floor but audio is static or one-way, move immediately to UMAC/LMAC traffic evidence: valid TCH/S uplink frames after `UMAC floor granted`, direction by direction, before changing call-control again.
- Parallel audit agents are active for CMCE group/private control, UMAC/MAC traffic path, MM restart affiliation/EG, QA tests, and project-log continuity.

## 2026-06-04 20:01:36 EEST - Patched group return-PTT alias to pure floor control

User symptom:

- Live group-call validation still reported terminal-side `PTT denied` on return PTT.
- Previous logs showed CMCE/UMAC sending `D-TX GRANTED` and `FloorGranted`, so the remaining risk was not a BS-side `NotGranted` decision, but inconsistent active-call signalling around the grant.

Components in simple technical terms:

- CMCE/CC is call control. It decides whether a group call is being set up or whether an already active group call is only changing the speaker/floor.
- `D-TX GRANTED` is the CMCE floor response that tells a terminal whether it may transmit now, is queued, or is not granted.
- MLE/UMAC wraps that CMCE message into MAC-RESOURCE/STCH on the assigned traffic channel.
- The traffic usage marker is the MAC label for the active assigned channel. Without it, a terminal can receive signalling but still not treat it as valid permission for that traffic circuit.

ETSI clause scope checked:

- EN 300 392-2 clause 14.5.2.1 covers group-call setup with `D-CALL PROCEEDING`, `D-CONNECT`, and `D-SETUP`.
- EN 300 392-2 clause 14.5.2.2.1 covers active group-call floor control with `U-TX DEMAND` / `D-TX GRANTED`; queued/not-granted/granted floor responses keep the MS in `CALL-ACTIVE`.
- EN 300 392-2 clause 23.8.1 says the BS allocates a traffic usage marker for the assigned channel and that TCH/STCH traffic uses the corresponding usage marker.
- EN 300 392-2 clause 23.8.2.3.1 says an MS shall not transmit traffic unless authorized by CC and unless it has an applicable uplink traffic usage marker.
- EN 300 392-2 clause 23.8.4.2 allows downlink C-plane signalling on STCH using MAC-RESOURCE.
- This is clause-scoped engineering alignment only. It is not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - A compatible repeated `U-SETUP` to an already maintained same-GSSI call is now treated as a floor-request alias.
  - It no longer emits setup-phase `D-CALL PROCEEDING` or `D-CONNECT` before the floor response.
  - It still rejects releasing, unaffiliated, or incompatible same-GSSI attempts with the existing release path.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Individual `D-TX GRANTED` FACCH/STCH wrappers now preserve the active traffic `usage` marker.
  - `Granted` uses `UlDlAssignment::Both`; `RequestQueued` and `NotGranted` use `UlDlAssignment::Dl`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Group-addressed `D-TX GRANTED GrantedToOtherUser` FACCH/STCH now also preserves the active traffic `usage` marker and remains DL-only.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Repeated same-GSSI active-call, current-speaker, and hangtime tests now assert no setup-phase `D-CALL PROCEEDING`/`D-CONNECT`.
  - Tests now assert compact `D-TX GRANTED` plus channel allocation with the actual circuit timeslot, usage marker, and UL/DL direction.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 118 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `33ef3ca fix: treat repeated group setup as floor control`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `06b913c5f3254330596034b5a821cb874b3bc20a694932f7376a75df5e831a09`
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-33ef3ca8`.
- `2260082`, `2260616`, and `2260618` registered and affiliated to `226333`.
- No complete post-deploy group PTT attempt was present in the checked log sample yet, so terminal-side RF validation remains required.

Next live validation:

- Test BS is already running `v0.1.55-33ef3ca8`.
- Validate alternating group PTT on GSSI `226333` with `2260082`, `2260616`, and `2260618`.
- Required field evidence: no terminal `PTT denied`, no BS `NotGranted` unless another MS is truly still transmitting, `UMAC floor granted` follows the active speaker, and audio is intelligible rather than static.

## 2026-06-04 19:47:00 EEST - Patched first-PTT retry caused by missing floor reassert

User symptom:

- Group PTT intervention no longer shows the earlier `Service unavailable`, but first PTT sometimes only takes effect on the second attempt.

Live evidence:

- Logs after `4693056` showed repeated same-GSSI `U-SETUP` from the MS that Nexus-BS already considered the current speaker, for example:
  - `CMCE: mapping repeated U-SETUP ... call_id=5 state=Transmitting`
  - `DConnect { transmission_grant: Granted ... }`
- In that current-speaker path, the existing floor FSM returned `FromCurrentSpeaker` and did not emit a fresh `D-TX GRANTED` or UMAC `FloorGranted`.

Component in simple technical terms:

- CMCE tells the terminal whether it may transmit using `D-TX GRANTED`.
- UMAC needs `FloorGranted` to keep the traffic-channel uplink speaker mapped to the correct ISSI.
- `D-CONNECT Granted` is call setup/connection signalling; for repeated PTT inside an active group call it was not enough for this field behavior.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1.2 covers `D-CONNECT` as group-call setup/through-connect.
- EN 300 392-2 clause 14.5.2.2.1 b) defines `D-TX GRANTED` as the SwMI response that grants, queues, or denies group transmit permission, while MSs remain in `CALL-ACTIVE`.
- The reassert path is a compatibility handling of a repeated same-GSSI `U-SETUP` accepted as existing-call re-entry/floor intent. It is explicitly responsive to the received PDU, not an unsolicited random grant. This is clause-scoped engineering alignment only, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Added `fsm_group_reassert_current_speaker_floor`.
  - Validates the call exists, is transmitting, and the requester is the current speaker and affiliated to the GSSI.
  - Sends individual `D-TX GRANTED Granted`, group FACCH `D-TX GRANTED GrantedToOtherUser`, and UMAC `FloorGranted`.
  - Resets the local call timeout clock when the floor is reasserted.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Repeated same-GSSI `U-SETUP` from current speaker now uses the reassert path instead of the duplicate `U-TX DEMAND` path that ignored `FromCurrentSpeaker`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_repeated_group_u_setup_from_current_speaker_reasserts_existing_floor`.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 118 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `deefa8d fix: reassert group floor on repeated setup`
- Deployed with the new one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `ddbb38afa84973c83b9f727ede434fa571423c7ef94255f9780f23f0513b81b6`
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-deefa8d4`.
- `2260082`, `2260616`, and `2260618` registered and affiliated to `226333`.
- Repeated same-GSSI `U-SETUP` from current speaker now emits:
  - individual `D-TX GRANTED (Granted)`;
  - group FACCH `D-TX GRANTED (GrantedToOtherUser)`;
  - UMAC `FloorGranted` on the same `call_id`.
- Concrete post-deploy examples:
  - `19:50:48` `2260618` repeated `U-SETUP` on `call_id=4` -> `D-TX GRANTED Granted` + `UMAC floor granted`.
  - `19:50:55` `2260616` repeated `U-SETUP` on `call_id=4` -> `D-TX GRANTED Granted` + `UMAC floor granted`.
- No `rejecting colliding`, `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached` appeared in the filtered post-deploy group-call log sample.

Next non-repeating action:

- Operator validates first-try group PTT audio from `2260616` and `2260618`.
- If first-try floor now works but audio remains static/silent, move to UMAC/TCH-S uplink media evidence; do not reopen the CMCE setup-collision hypothesis.

## 2026-06-04 19:37:30 EEST - Patched repeated group PTT U-SETUP service-unavailable path

User symptom:

- Repeated PTT in group call reports `Service unavailable`.
- Live logs showed `CMCE: rejecting colliding U-SETUP ... active gssi=226333` followed by `DRelease { call_identifier: 0, disconnect_cause: RequestedServiceNotAvailable }`.

Components in simple technical terms:

- CMCE is call control. It owns `U-SETUP`, `D-CONNECT`, `D-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, and group-call release decisions.
- UMAC is the lower MAC scheduler. It does not decide call policy; it receives CMCE `FloorGranted`/`FloorReleased` commands and maps the current speaker onto the traffic channel.
- Hangtime is Nexus-BS local call retention after a speaker releases PTT. The call is still maintained so the next PTT can reuse the same `call_id` and traffic circuit.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1.2 defines group call setup, `D-CALL PROCEEDING`, `D-CONNECT`, and the SwMI call identifier used for subsequent PDUs.
- EN 300 392-2 clause 14.5.2.1.3 covers same-group setup collision handling and does not require creating a parallel group call.
- EN 300 392-2 clause 14.5.2.2.1 says the SwMI controls group transmit permission with `U-TX DEMAND` / `D-TX GRANTED`; queued/not-granted floor responses keep the MS in `CALL-ACTIVE`.
- EN 300 392-2 clause 14.5.2.3.2 uses `D-RELEASE` when the SwMI cannot support a call/request. A compatible repeated setup for the same active GSSI is now not treated as that failure case.
- This is clause-scoped engineering alignment, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Same-GSSI `U-SETUP` while a group call is active or in hangtime no longer receives dummy-call `D-RELEASE RequestedServiceNotAvailable`.
  - If the call is already pending release, the old rejection remains; a releasing traffic circuit must not be reused or duplicated.
  - The repeated requester must be affiliated to the GSSI and request a compatible service before rejoining the existing call.
  - Nexus-BS responds with existing-call `D-CALL PROCEEDING` and `D-CONNECT`, including the active traffic allocation and existing `call_id`.
  - The repeated setup is then routed through the existing group floor FSM:
    - while another MS is transmitting, the requester receives `D-TX GRANTED RequestQueued`;
    - during hangtime, the requester receives floor grant on the existing call and UMAC gets one `FloorGranted`;
    - no second group circuit is allocated.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Replaced the old regression that expected service-unavailable rejection.
  - Added active-speaker coverage: no `D-RELEASE`, no second `D-SETUP`, no second UMAC open, queued floor response on the active call id.
  - Added hangtime coverage: no `D-RELEASE`, no second circuit, requester gets floor on the active call id.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 117 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `4693056 fix: reuse active group call for repeated setup`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Local and remote deployed binary SHA-256:
  - `0c9f655795931d9dfd924e40b6f16b63ff8c58baa5b2b35abd052e681bbc3eaa`
- Deployed direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- No binary backup was created.
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-46930568`.
- MM armed restart recovery for cached/configured ISSIs `{2260082, 2260616, 2260618}`.
- `2260618` registered/affiliated to `226333` at about 4 s after restart.
- `2260616` registered/affiliated to `226333` at about 5 s after restart.
- `2260082` registered/affiliated to `226333` at about 6 s after restart, then repeated its attach/affiliation once.
- Initial post-restart log filter found no `rejecting colliding`, `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached`.
- No live PTT attempt had occurred in the sampled post-restart log yet, so the field validation step remains required.

Next non-repeating actions:

- Restart test BS and validate group PTT on `226333`, especially repeated PTT from `2260616` and `2260082` during active speaker and hangtime.
- If static audio persists after this service-unavailable fix, the next non-repeating investigation is UMAC/TCH-S voice-direction evidence, not another CMCE setup-collision hypothesis.

## 2026-06-04 19:28:03 EEST - Deployed MM attach-confirmation hardening to test BS

User symptom:

- After BS restart, terminals can show `Unit Not Attached` even while Nexus-BS is rebuilding its local MM/CMCE state.

Components in simple technical terms:

- MM (Mobility Management) owns terminal registration, location update, group affiliation state, and restart recovery.
- MLE/LLC carry MM downlink PDUs over the air and report whether acknowledged transfers succeeded or failed.
- CMCE consumes MM register/affiliate updates so group/private call control knows which ISSIs are usable listeners.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4 permits SwMI-initiated registration at any time using `D-LOCATION UPDATE COMMAND`, including a group identity report request.
- EN 300 392-2 clause 16.4.4 also permits/defines the `U-LOCATION UPDATE DEMAND` response to that command.
- EN 300 392-2 clause 16.4.4 says an MS that supports listed extended capabilities includes the extended capabilities IE in `U-LOCATION UPDATE DEMAND`; Nexus-BS now accepts that IE as non-fatal but does not yet act on the capability bits.
- LLC/MLE transfer reports are local SAP evidence, not over-air certification. No formal ETSI/TETRA certification is claimed.

Patch implemented:

- Commit: `c38b13b fix: harden MM restart attach confirmation`
- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Restart recovery now sends the first probe after 1 s, spaces cached ISSIs by 1 s, retries every 2 s, and keeps the same long total window with 150 attempts.
  - `U-LOCATION UPDATE DEMAND.extended_capabilities` is accepted/logged instead of rejecting registration.
  - `D-LOCATION UPDATE ACCEPT` now uses a tracked local MLE handle.
  - If MLE/LLC reports `FAILED_TRANSFER` for that accept, MM treats the registration as unconfirmed, fails open to `StayAlive`, withdraws shared CMCE registration/affiliation, and sends a fresh `D-LOCATION UPDATE COMMAND(group identity report=1)`.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added coverage for standards-permitted `extended_capabilities`.
  - Added coverage for failed `D-LOCATION UPDATE ACCEPT` transfer causing MM to re-probe instead of leaving BS/MS state split.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs failed_location_update_accept --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo test -p tetra-entities --test test_mm_bs extended_capabilities --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 112 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Local and remote test binary SHA-256:
  - `80f72b4d226c5da7e83e42d525188848a80adb02d8bf094a2f1758cfe690e01c`
- Deployed direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- No binary backup was created.
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-c38b13ba`.
- MM armed restart recovery for cached/configured ISSIs `{2260082, 2260616, 2260618}`.
- `2260616` registered/affiliated to `226333` at about 4 s after restart.
- `2260082` registered/affiliated to `226333` at about 5 s after restart.
- `2260618` needed repeated 2 s recovery probes and then registered/affiliated to `226333` at about 22 s after restart.
- No `D-LOCATION UPDATE ACCEPT failed transfer` reprobe occurred in this sampled live run; the new failed-accept path remains unit-tested.
- No literal `Unit Not Attached` appeared in `nexus-bs.log` or `control.log`.

Residual observations:

- `2260618` is still slower to answer restart recovery than the other terminals; this now retries more frequently and remains long-lived, but operator screen verdict is still required.
- `2260616` later hit BS-initiated EG3 T352 timeout and stayed `StayAlive`, which is safe for PTT validation.
- Next non-repeating action is live group/private PTT validation, not another MM hypothesis unless a terminal remains unattached after the recovery window.

## 2026-06-04 13:17:51 EEST - Deployed long-lived MM restart recovery to test BS

Deployed commit:

- `4588590 fix: extend MM restart recovery`

Build/deploy:

- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Local/remote deployed binary SHA-256:
  - `dd061b3bf5169c5ff0ff45a5505cc0d3dca1a7b30f21584f39a74a8ea1722bda`
- Deployed direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- No binary backup was created.
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence:

- Startup banner reports `Build: v0.1.55-4588590b`.
- MM armed restart recovery for cached/configured ISSIs `{2260082, 2260616, 2260618}`.
- `2260082` self re-registered and CMCE received:
  - `subscriber register issi=2260082`
  - `subscriber affiliate issi=2260082 groups=[226333]`
- `2260616` self re-registered and CMCE received:
  - `subscriber register issi=2260616`
  - `subscriber affiliate issi=2260616 groups=[226333]`
- `2260618` missed early recovery commands, then the new 5 s long-lived retry sequence ran:
  - attempts `1/60`, `2/60`, `3/60`, `4/60`
  - then `2260618` re-registered and CMCE received `subscriber affiliate issi=2260618 groups=[226333]`.
- After waiting past the prior 60 s group-report window, no `solicited group report window expired` or `re-requesting group report` appeared after the group-bearing `U-LOCATION UPDATE DEMAND`.
- No literal `Unit Not Attached` appeared in `nexus-bs.log` or `control.log`.

Residual observations:

- BS-initiated EG3 assignment still timed out for `2260082` and `2260616`; fail-safe behavior kept `StayAlive`.
- That EG fallback is not a blocker for group/private PTT validation, but EG3 default should be revisited after the voice path is stable because terminals are not confirming the BS-initiated EG assignment in this live run.

Next non-repeating actions:

- Test group PTT on `226333` in sequence:
  - `2260616` PTT/speak/release.
  - `2260082` PTT/speak/release.
  - `2260618` PTT/speak/release if available.
- Capture logs for `U-TX DEMAND`, `D-TX GRANTED`, `FloorGranted`, `UMAC floor granted`, `rx_blk_traffic`, `TCH/S`, `UL inactivity`, `PTT denied`, and `NotGranted`.
- Then repeat private simplex `2260082 <-> 2260616`.
- Do not re-open the pure UMAC bit-copy hypothesis unless new evidence contradicts the existing component tests.

## 2026-06-04 13:09:59 EEST - MM restart recovery made long-lived and group-report pending cleared

User symptom:

- After BS restart, some terminals still show `Unit Not Attached` during the recovery window.

Component in simple terms:

- MM restart recovery is the BS-side procedure that asks locally known/cached terminals to perform location update again after the Nexus-BS process restarts.
- LLC carries those MM commands with ACK/retry. If recovery commands are retried too aggressively during SDR startup, they can exhaust while the radio/air path is still settling.
- The solicited group-report window is a local MM bookkeeping window opened after `D-LOCATION-UPDATE-COMMAND(group identity report=1)`.

Live evidence from `chris@192.168.1.179` before this patch:

- Running binary was current deployed build `Nexus-BS v0.1.55`, `Build: v0.1.55-acbba6d5`.
- Recovery cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` contained `2260082`, `2260616`, and `2260618`.
- Startup log:
  - `2260082` self re-registered and affiliated to `226333` almost immediately.
  - `2260616` needed repeated restart-recovery `D-LOCATION-UPDATE-COMMAND` attempts before registration/affiliation recovered.
  - `2260618` recovered later, but the local solicited group-report window still expired even though its later `U-LOCATION UPDATE DEMAND` carried group `226333`.
- Conclusion: current cache/recovery path works eventually, but early retries are too compressed and one local pending flag remains misleading after a late group-bearing location update.

ETSI scope:

- EN 300 392-2 clause 16.4.4 / figure 16.6 permits SwMI-initiated registration using `D-LOCATION UPDATE COMMAND`, including group identity report request.
- EN 300 392-2 clauses 16.9.3.4, 16.10.17, 16.10.23, and 16.10.35a define the accepted location-update and group identity response handling used by the existing MM path.
- This patch does not change standardized PDU fields. It changes local Nexus-BS retry cadence and local pending-window cleanup only.
- No formal certification claim is made.

Patch:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Recovery command spacing changed from 18 timeslots to `18 * 4` timeslots (about 1 s).
  - Recovery retry cadence changed from `18 * 4` timeslots (about 1 s) to `5 * 18 * 4` timeslots (about 5 s).
  - Max recovery attempts changed from 5 to 60, making recovery long-lived for several minutes instead of giving up in a few seconds.
  - A group-bearing `U-LOCATION UPDATE DEMAND` now clears the local solicited group-report window even when the optional group-report-response IE is absent.
  - If a terminal recovers registration but still has no attached groups when the solicited group-report window expires, MM re-requests the group report with `D-LOCATION UPDATE COMMAND` instead of leaving CMCE with no group listener.
  - Added a debug-only MM test accessor for the solicited group-report pending flag.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated restart recovery pacing assertions for 1 s inter-ISSI spacing.
  - Added assertion that reported group identities complete the solicited restart group-report window.
  - Added `test_restart_recovery_re_requests_group_report_when_recovered_without_groups`.
  - Added `test_restart_recovery_retries_are_long_lived_and_paced`.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 110 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating actions:

- Commit this MM patch.
- Build locally for AArch64 with the Nexus-BS SoapySDR sysroot command.
- Deploy direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; do not compile on the Pi and do not create binary backups.
- Restart test BS and verify:
  - build hash changed from `acbba6d5`;
  - recovery attempts are spaced about 1 s per ISSI and about 5 s per retry;
  - `2260082`, `2260616`, and `2260618` all re-register/re-affiliate to `226333`;
  - no stale `solicited group report window expired` appears after a group-bearing location update already restored the terminal.

## 2026-06-04 13:00:32 EEST - MM restart recovery pacing for post-restart Unit Not Attached window

User symptom:

- After BS restart, terminals can show `Unit Not Attached`.

Component in simple terms:

- MM is Mobility Management. It owns terminal registration/location update and group affiliation rebuild after restart.
- Restart recovery is the MM procedure that asks still-camped radios to re-register after the Nexus-BS process has lost in-memory subscriber state.

Live evidence from `chris@192.168.1.179`:

- Active process: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- Recovery cache exists at `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` and contains `2260082`, `2260616`, `2260618`.
- At `12:49:26` Nexus-BS armed restart recovery for all three ISSIs and immediately sent `D-LOCATION-UPDATE-COMMAND` to all three in the same startup window.
- The live log also showed startup PHY timing loss/late TX warnings, then LLC retransmission exhaustion for some initial recovery commands.
- All three terminals later recovered:
  - `2260082`: `D-LOCATION UPDATE ACCEPT`, CMCE Register, CMCE Affiliate `[226333]`, solicited group report accepted.
  - `2260618`: `DemandLocationUpdating`, `D-LOCATION UPDATE ACCEPT`, EG3 allocation, CMCE Register/Affiliate `[226333]`.
  - `2260616`: slower recovery after repeated `D-LOCATION-UPDATE-COMMAND`, then Register/Affiliate `[226333]`.
- No literal `Unit Not Attached` appears in `nexus-bs.log` or `control.log`; the label is likely the radio/Brew symptom during the temporary not-yet-recovered MM state.

ETSI scope:

- EN 300 392-2 clause 16.4.4 / figure 16.6 permits infrastructure-initiated registration using `D-LOCATION UPDATE COMMAND`, optionally with group report request.
- This patch does not change the standardized PDU or registration semantics. It changes only local Nexus-BS retry timing so acknowledged MM commands are not blasted during SDR startup.
- No formal certification claim is made.

Patch:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Added a 2-second TDMA startup guard before restart recovery probes.
  - Added 250 ms inter-ISSI spacing for cached/configured recovery ISSIs.
  - Added one-command-per-tick deferral so multiple due probes cannot burst together.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated restart recovery tests to assert no command during startup guard.
  - Asserted cached/configured ISSIs are paced instead of sent in the same tick.
  - Preserved existing tests proving that actual registration/group/EG state is rebuilt only from terminal responses.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 6 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 108 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating actions:

- Commit, build locally for AArch64, deploy direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- Restart test BS and verify post-restart logs show spaced recovery attempts, not three immediate commands at `t=0`.
- After this MM patch is live, return to CMCE group queued `U-TX CEASED` withdrawal and live group/private audio validation. Do not reopen the solved pure UMAC bit-copy hypothesis.

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

## 2026-06-04 12:37:58 EEST - MM restart recovery accepts solicited complete group reports

Problem observed:

- After BS restart, terminals could show `Unit Not Attached`.
- Live logs showed `U-ATTACH/DETACH GROUP IDENTITY` from `2260082`/`2260616` carrying:
  - `group_identity_attach_detach_mode=true`
  - `group_report_response len=1 data=0`
  - `group_identity_uplink=[226333]`
- MM rejected that as a malformed mixed standalone request, which prevented coherent group re-affiliation after restart.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: `D-LOCATION UPDATE COMMAND` may request a group report; the MS may report/re-attach groups either in `U-LOCATION UPDATE DEMAND` or following `U-ATTACH/DETACH GROUP IDENTITY`.
- EN 300 392-2 clause 16.8.3: for SwMI-initiated group report, `U-ATTACH/DETACH GROUP IDENTITY` uses `not report request`, detach-all-then-attach for the first report PDU, and includes `group report complete` when all reported groups fit.
- EN 300 392-2 clause 16.8.2 remains enforced for unsolicited MS-initiated attach/detach: `group report response` must not be present.
- EN 300 392-2 clause 16.10.27a: `group_report_response` length 1 value 0 means complete; value 1 is reserved.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Added a per-ISSI local pending window for group reports solicited by `D-LOCATION UPDATE COMMAND(group_identity_report=true)`.
  - `U-LOCATION UPDATE DEMAND` now accepts `GroupIdentityLocationDemand` plus `group_report_response(1,0)` only for `DemandLocationUpdating`.
  - `U-ATTACH/DETACH GROUP IDENTITY` still rejects unsolicited mixed report-response + group-list PDUs, but accepts the same shape when a solicited group report is pending.
  - A BS-commanded DemandLocationUpdating response with no groups no longer immediately triggers a duplicate `D-LOCATION UPDATE COMMAND` while a follow-up group report is pending.
  - Registration is still not synthesized from standalone group attach; unknown ISSIs must pass the location-update path.

Focused tests:

- `test_restart_recovery_demand_location_update_accepts_complete_group_report_with_groups`
- `test_restart_recovery_accepts_solicited_attach_detach_group_report_completion`
- Existing unsolicited mixed reject tests still pass:
  - `test_mixed_group_report_response_and_attach_list_rejects_without_affiliation`
  - `test_mixed_group_report_response_and_mode_one_preserves_existing_groups`

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/mm/mm_bs.rs crates/tetra-entities/tests/test_mm_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 108 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM restart recovery patch.
2. Build Nexus-BS AArch64 locally only with the SoapySDR sysroot command from build memory.
3. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup.
4. Restart the test BS and verify logs no longer show `Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY` for the solicited restart-recovery report.
5. Confirm `2260082`, `2260616`, and any visible `2260618` register and affiliate to `226333` after restart.
6. Resume group-call audio hardening next: UMAC must gate raw/decoded TCH/S media by current floor owner/floor epoch to prevent stale-speaker static.

## 2026-06-04 12:41:48 EEST - Deployed MM solicited group-report fix to test BS

Commit deployed:

- `8981c33 fix: accept solicited restart group reports`

Local verification before deploy:

- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 108 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `c103827eb5bce81ad5e340766178b130fdb9b54dbf90532b0df89a10fec8cf72`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the previous test BS/control-service from pidfile PIDs before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `c103827eb5bce81ad5e340766178b130fdb9b54dbf90532b0df89a10fec8cf72`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `17434`
  - control-service pid `17437`
  - nexus-bs pid `17439`
- Dashboard root responds on configured port `8080`.

Live restart evidence:

- `2260082`:
  - `U-LOCATION UPDATE DEMAND(DemandLocationUpdating)` accepted.
  - Group `226333` accepted in `D-LOCATION UPDATE ACCEPT`.
  - Later solicited `U-ATTACH/DETACH GROUP IDENTITY` with `group_report_response len=1 data=0` and `gssi=226333` was accepted, ACKed, and re-affiliated.
- `2260616`:
  - Same solicited mixed group-report completion was accepted and ACKed.
  - CMCE received final `Affiliate` for `226333`.
- `2260618`:
  - Roaming update accepted with EG3 allocation and group `226333`.
  - CMCE received `Register` then `Affiliate`.
- `grep "Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY"` returned no post-deploy entries.

Remaining observations:

- Startup still had short LLC retransmission bursts for `2260082`, `2260616`, and `2260618` before the final ACK/affiliate state settled.
- One startup PHY warning appeared: `Too late to produce TX block ...`; do not chase RF until a live RF symptom repeats after attach stability.
- Next protocol hardening target remains group/private call audio static: UMAC media should be gated by current CMCE floor owner/floor epoch, and stale queued raw TCH/S should be purged on floor transitions.

## 2026-06-04 12:47:05 EEST - UMAC purges stale group-call media on floor transitions

Problem targeted:

- Group call audio could become static when speakers alternate.
- Read-only UMAC/CMCE reviews identified that queued TCH/S media was per-timeslot only and was not purged on CMCE floor transitions.
- TMD media indications still do not carry source ISSI/floor epoch, so this patch does not claim full speaker-source validation.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group request/grant/cease controls who may transmit.
- EN 300 392-2 clause 14.5.2.4: CMCE and MAC/UMAC must synchronize U-plane switching with traffic permission state.
- EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5 remain the raw TCH/S half-slot preservation scope.

Patch implemented:

- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Added `clear_tx_data(ts)` to drop queued DL media blocks for a traffic slot.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `clear_dl_media_queue(ts, reason)` wrapper with logging.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Drops UL TMD media when no UL circuit is active.
  - Drops UL TMD media during hangtime before refreshing `last_ul_voice` or routing to DL/Brew.
  - Drops media if the DL target timeslot is in hangtime.
  - Clears queued DL media on `FloorReleased`, `FloorGranted`, and `CallEnded`.

Focused tests:

- `test_group_ul_raw_block2_is_dropped_during_hangtime`
- `test_group_floor_release_purges_queued_raw_block2_media`
- `test_group_floor_grant_purges_stale_raw_block2_but_allows_new_media`

Verification:

- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit UMAC floor-transition media purge patch.
2. Build/deploy to test BS only after commit.
3. Run live alternating group PTT on GSSI `226333`:
   - `2260616` PTT/speak/release.
   - `2260082` PTT/speak/release.
   - `2260618` if available.
   - Repeat at least three turns.
4. Watch for `U-TX DEMAND`, `D-TX GRANTED`, `FloorGranted`, `FloorReleased`, raw/decoded TCH/S route, no `PTT denied`, and no stale/static audio.
5. If static persists, next required design change is extending TMD/CircuitTxBlock metadata with source/floor epoch; the current SAP cannot prove late media belongs to the current floor holder.

## 2026-06-04 12:50:07 EEST - Deployed UMAC stale-media purge build to test BS

Commit deployed:

- `32ee733 fix: purge stale UMAC media on floor changes`

Local verification before deploy:

- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `d228a02bfbceee2e8ce4bb2975c39932f574152f0c241031b9e269a7bb7a98b1`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service from pidfile PIDs before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `d228a02bfbceee2e8ce4bb2975c39932f574152f0c241031b9e269a7bb7a98b1`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `17696`
  - control-service pid `17699`
  - nexus-bs pid `17701`

Post-restart terminal state:

- `2260082`: Register/Affiliate to `226333`, RSSI about `-23 dBFS`.
- `2260618`: Register/Affiliate to `226333`, RSSI about `-46 dBFS`.
- `2260616`: Register/Affiliate to `226333`, RSSI about `-25 dBFS`.
- Solicited group-report completion was accepted for all three:
  - `2260082`
  - `2260618`
  - `2260616`
- `grep "Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY"` returned no post-deploy entries.

Required next physical test:

1. On group `226333`, have `2260616` PTT/speak/release.
2. Then `2260082` PTT/speak/release.
3. Then `2260618` PTT/speak/release if available.
4. Repeat at least three alternating turns.
5. Post-test log filter:
   - `2260082|2260616|2260618|226333|U-TX DEMAND|U-TX CEASED|D-TX GRANTED|D-TX CEASED|FloorGranted|FloorReleased|UMAC floor granted|UMAC voice route|dropped .*queued DL media|rx_blk_traffic: forwarding raw TCH/S Block2|rx_blk_traffic: decoded valid TCH/S frame|PTT denied|NotGranted|UL inactivity`
6. Pass requires operator audio verdict: each speaker intelligible to the other group members, not static.

## 2026-06-04 20:42:01 EEST - CMCE group D-SETUP speaker refresh before redeploy

Live symptom:

- User again reported static/no intelligible audio when the other station entered with PTT on group call.
- Current group context remains GSSI `226333`; recent logs include radios `2260082`, `2260616`, and stale `2260618`.

Log evidence:

- Live test BS still showed group traffic where voice was sometimes routed correctly:
  - `UMAC voice route: UL ts=2 bits=274 -> DL ts=2`
  - `UMAC voice route: UL ts=2 raw TCH/S Block2 bits=216 -> DL ts=2`
- The same live log also showed stale late-entry group setup:
  - `DSetup ... calling_party_address_ssi: Some(2260618)` while the active test context had moved to `2260082`/`2260616` on GSSI `226333`.
- This points at CMCE late-entry/back-up D-SETUP speaker coherence during floor handoff, not encryption.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.2.1.1 and 14.5.2.1.2: group `D-SETUP` carries the group-call setup/late-entry context.
- EN 300 392-2 clause 14.5.2.2.1: `D-TX GRANTED` moves transmit permission/floor.
- EN 300 392-2 clauses 23.5 and 23.8: assigned-channel TCH/S media must remain traffic, including FACCH/STCH stealing cases.
- This is clause-scoped hardening only; no formal certification claim.

Patch status:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added cached group D-SETUP speaker refresh helper.
  - Added immediate group D-SETUP refresh with channel allocation after floor changes.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Repeated U-TX DEMAND from current speaker now reasserts the existing floor.
  - Floor handoff paths now refresh late-entry D-SETUP with the new speaker.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `226333` alternating PTT regression.
  - Added hangtime retake/queued handoff assertions that D-SETUP refresh uses the new speaker.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added UMAC/LMAC boundary regression proving that after `FloorReleased` then `FloorGranted`, UMAC schedules `ul_phy_chan=Tp` and LMAC decodes a valid full-slot TCH/S frame to `TmdCircuitDataInd`.

Verification so far:

- `cargo fmt` -> pass.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 120 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit the CMCE D-SETUP speaker refresh and UMAC/LMAC boundary test.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Ask the operator to retest alternating group PTT on `226333` with `2260082` and `2260616`.
4. If static persists after this deploy, the next patch target is not cached D-SETUP; inspect whether repeated FACCH/STCH refresh steals too much audio and whether late `NormalTrainSeq2` arrivals occur while no active group floor is present.

## 2026-06-04 20:45:30 EEST - Deployed CMCE D-SETUP speaker refresh build to test BS

Commit deployed:

- `d518c03 fix: refresh group setup speaker on floor handoff`

Local verification before deploy:

- `cargo fmt` -> pass.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 120 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Deploy:

- Command: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Build was local on macOS only; no Rust/TETRA compile was done on `chris@192.168.1.179`.
- Remote target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- No binary backup was created.
- Build line: `Build: v0.1.55-d518c03c`
- Remote SHA256: `cf022002aabcafcf85ee4299cfb3686020707bd9928d61e5f338f5bc5ac19143`
- Remote processes after restart:
  - control wrapper pid `21076`
  - control-service pid `21079`
  - nexus-bs pid `21081`

Post-restart terminal state:

- `2260082`: CMCE register then affiliate to `226333`.
- `2260618`: CMCE register then affiliate to `226333`.
- `2260616`: CMCE register then affiliate to `226333` twice during startup settling.

Required next physical test:

1. Retest alternating group PTT on `226333` between `2260082` and `2260616`.
2. Verify each entry gets first-try transmit permission and intelligible audio, not static.
3. If static repeats, capture the 30 seconds around the event and inspect:
   - stale `DSetup ... calling_party_address_ssi`
   - `FACCH stealing ... speech_present=false` density during talk spurts
   - `rx_tpsap_prim got NormalTrainSeq2` without `UMAC voice route`
   - any `PTT denied`, `NotGranted`, `Service unavailable`, or `UL inactivity`

## 2026-06-04 20:58:20 EEST - Group UL inactivity grants queued requester

Problem targeted:

- Post-deploy logs showed `UL inactivity timeout on ts=2` during group-call testing.
- Before this patch, the group-call timeout path forced `D-TX CEASED`/hangtime even when another MS already had a queued U-TX DEMAND. That can make the waiting MS need a second PTT attempt.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group floor request/grant/cease controls who may transmit.
- The local UL inactivity guard is treated as the BS-side cease event. If a valid requester was already queued, SwMI grants that requester immediately instead of requiring another demand.
- Clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Late-entry timer D-SETUP resend now re-derives group `calling_party_address_ssi` from `active_calls[call_id].source_issi` before serialization.
  - Group UL inactivity timeout now:
    - filters queued requester by current group affiliation,
    - grants queued requester with individual `D-TX GRANTED`,
    - sends group FACCH `D-TX GRANTED`,
    - refreshes group D-SETUP with the new speaker,
    - emits UMAC/Brew `FloorGranted`.
  - If no valid requester is queued, old behavior remains: enter hangtime, send `D-TX CEASED`, emit `FloorReleased`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Made the existing compact group individual-grant helper visible inside the CC-BS module for timer reuse.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_ul_inactivity_hands_floor_to_queued_requester` for GSSI `226333`.

Verification:

- `cargo fmt` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_ul_inactivity_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 121 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this timeout handoff patch.
2. Deploy direct to test BS with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest alternating group PTT on `226333`; expected behavior is first queued PTT becomes speaker after timeout/cease without a second press.

## 2026-06-04 21:21:02 EEST - Private-call shutdown hardening for Motorola restart symptom

Problem targeted:

- Field report: private call voice now works, but when the opposite party closes the private call, a Motorola terminal restarts and re-attaches.
- Live log evidence showed `U-DISCONNECT` for private `call_id=4`, then `D-DISCONNECT` to the peer, followed by a very short local fallback to `D-RELEASE` and circuit close while FACCH/STCH repeats were still in flight.
- CMCE auditor also found a spec-order risk: after peer receives `D-DISCONNECT`, EN 300 392-2 expects peer `U-RELEASE` and local call clearing, so sending final `D-RELEASE` again to that same peer can double-clear the call context.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: an MS initiating disconnection sends `U-DISCONNECT` and waits for `D-RELEASE`.
- EN 300 392-2 clauses 14.5.1.3.2/14.5.1.3.3 and 14.7.1.6/14.7.1.9: BS uses `D-DISCONNECT` to request peer release, peer responds with `U-RELEASE`, and `D-RELEASE` informs that the connection has been released.
- EN 300 392-2 clause 14.5.1.2.1: simplex private floor control must not race call clearing with new `D-TX GRANTED` / `D-TX CEASED` signalling.
- This is clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - `DisconnectPending` now tracks both `awaiting_release_from` and `release_to_issi`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Active-call `D-DISCONNECT` now uses assigned-channel `UlDlAssignment::Dl`.
  - Local delivery guards for private `D-DISCONNECT` and `D-RELEASE` increased from 16 timeslots to 2 seconds, so FACCH/STCH repeats are not cut short.
  - Added targeted final release: after peer `U-RELEASE`, final `D-RELEASE` is sent only to the original `U-DISCONNECT` initiator, not to the peer that already cleared after `D-DISCONNECT`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Peer `U-RELEASE` response guard increased from 16 timeslots to 5 seconds before fallback `D-RELEASE`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Suppresses private `U-TX DEMAND` / `U-TX CEASED` while `D-DISCONNECT` delivery is pending.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated caller/called private disconnect assertions for one-leg final `D-RELEASE`.
  - Added `test_p2p_pending_disconnect_delivery_suppresses_floor_pdus`.
  - Hardened fallback timeout tests to prove no early close before the longer delivery guards.

Verification:

- `rustfmt --edition 2024` on touched CMCE/test files -> pass.
- `git diff --check` on touched files -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_disconnect_delivery_suppresses_floor_pdus --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_u_disconnect_delivery_guard_falls_back_to_release_without_peer_wait --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_disconnect_closes_after_bounded_timeout --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_active_p2p_discarded_release_reporters_do_not_close_before_guard_timeout --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 122 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.

Next non-repeating execution:

1. Commit this private-call shutdown patch.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex call between `2260082` and `2260616`/`2260618`:
   - voice in both directions,
   - close from caller,
   - close from called party,
   - no Motorola restart/re-attach after remote hangup.
4. If restart persists, capture the 20 seconds around hangup and inspect `D-DISCONNECT`, peer `U-RELEASE`, final `D-RELEASE`, and circuit close order.

## 2026-06-04 21:29:51 EEST - Private simplex hangup `No answer` follow-up

Problem targeted:

- Field report after build `v0.1.55-78d4644a`: Motorola showed `No answer` at the end of a private simplex call.
- Live log around `21:24:00` showed `U-DISCONNECT` from `2260616`, BS `D-DISCONNECT` to peer `2260618`, but no peer `U-RELEASE`.
- At `21:24:06` the local pending-disconnect guard fired and sent fallback `D-RELEASE` to both legs. This explains a timeout-like terminal UI instead of a clean release handshake.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: the MS initiating disconnection sends `U-DISCONNECT` and waits for `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: an MS receiving `D-DISCONNECT` shall respond with `U-RELEASE`; an MS receiving `D-RELEASE` sends no response.
- EN 300 392-2 clause 14.7.1.6: `D-DISCONNECT` response expected is `U-RELEASE`.
- EN 300 392-2 clause 14.7.1.9: `D-RELEASE` response expected is none.
- This is clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Active private-call `D-DISCONNECT` now uses assigned-channel `UlDlAssignment::Both`, because that PDU explicitly expects the MS uplink `U-RELEASE` response.
  - Final private-call `D-RELEASE` remains downlink-only (`UlDlAssignment::Dl`) through the existing release path, because no MS response is expected.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated the three private-call disconnect tests to assert response-capable `D-DISCONNECT` channel allocation in both caller-hangs-up and called-party-hangs-up directions.
  - Existing helper coverage still asserts final `D-RELEASE` FACCH/STCH remains `Dl`.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 122 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this private simplex hangup patch.
2. Deploy direct with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex between `2260082` and `2260616`/`2260618`:
   - peer should answer `D-DISCONNECT` with `U-RELEASE`,
   - initiator should receive final one-leg `D-RELEASE`,
   - no `Pending individual D-DISCONNECT timed out`,
   - no Motorola `No answer` at normal hangup.
4. If the peer still does not send `U-RELEASE`, inspect on-air MAC STCH allocation bits and raw decoded `MacResource` for the `D-DISCONNECT` slot.

## 2026-06-04 21:34:15 EEST - Private disconnect collision semantics tightened

Problem targeted:

- Agent CMCE audit found that BS accepted peer `U-DISCONNECT` as if it were the `U-RELEASE` acknowledgement to a pending private-call `D-DISCONNECT`.
- That is too loose for ETSI call clearing: it can hide a missing `U-RELEASE` and complete the wrong state transition.

ETSI clause scope:

- EN 300 392-2 clause 14.7.1.6: `D-DISCONNECT` response expected is `U-RELEASE`.
- EN 300 392-2 clause 14.7.2.4: `U-DISCONNECT` is an MS request to disconnect a call and expects `D-DISCONNECT`/`D-RELEASE`; it is not the acknowledgement to `D-DISCONNECT`.
- EN 300 392-2 clause 14.7.2.9: `U-RELEASE` is the acknowledgement to `D-DISCONNECT`.
- EN 300 392-2 clause 14.5.1.3.5: in colliding disconnection, the MS shall respond to incoming `D-DISCONNECT` as in clause 14.5.1.3.3, i.e. with `U-RELEASE`.
- This is clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Removed the two branches that allowed `U-DISCONNECT` from the awaited peer to complete pending `D-DISCONNECT` delivery or pending `DisconnectPending` state.
  - Pending private disconnect now ignores peer `U-DISCONNECT` and continues waiting for real `U-RELEASE` or bounded fallback timeout.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_p2p_peer_u_disconnect_does_not_ack_pending_d_disconnect`.
  - The test proves peer `U-DISCONNECT` does not trigger final `D-RELEASE`; peer `U-RELEASE` remains the completing PDU.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_peer_u_disconnect_does_not_ack_pending_d_disconnect --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 123 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to test BS.
2. Retest private simplex hangup. Expected clean log is `U-DISCONNECT` -> `D-DISCONNECT` -> peer `U-RELEASE` -> final one-leg `D-RELEASE`, with no fallback timeout and no terminal `No answer`.
3. If the peer sends `U-DISCONNECT` instead of `U-RELEASE`, treat that as a remaining terminal/protocol collision signal to inspect on-air delivery, not as a successful acknowledgement.

## 2026-06-04 22:19:43 EEST - Private simplex bearer-tail drain before peer clear

Problem targeted:

- Field test after build `v0.1.55-5acff30d`: private simplex call `2260616 -> 2260618` worked, but ending the call caused peer `2260618` Motorola MXP600 to soft reboot.
- Live log showed private-call teardown tightly adjacent to recent speech/floor signalling:
  - `U-TX CEASED` from a private-call participant was followed immediately by `D-TX CEASED` to both legs.
  - Later `U-DISCONNECT` was followed immediately by prompt initiator `D-RELEASE` plus peer `D-DISCONNECT`.
- The previous prompt `D-RELEASE` fix is still required to avoid terminal `No answer`; the remaining risk is peer-facing clear/cease signalling being sent before the traffic bearer has drained.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex individual floor control uses `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and `D-TX CEASED`; unsolicited peer grants remain forbidden.
- EN 300 392-2 clauses 14.5.1.3.1/14.5.1.3.3: a private-call disconnect initiator waits for `D-RELEASE`; the peer is cleared by `D-DISCONNECT -> U-RELEASE`.
- EN 300 392-2 clause 23.8.5: for N=4/8 circuit-mode data, after `U-TX CEASED` or `U-DISCONNECT` from the transmitting MS, BS should issue N-1 traffic slots containing tail bits before `D-TX CEASED`, `D-RELEASE`, or `D-DISCONNECT` to receiving MS(s).
- The implemented guard applies the same short N=4-equivalent drain to current simplex speech as a conservative Motorola/bearer-tail compatibility guard because CMCE does not yet expose bearer interleaving depth. This is clause-scoped hardening, not formal certification evidence.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added pending tail-drain state for simplex private `U-TX CEASED` and `U-DISCONNECT`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `INDIVIDUAL_SIMPLEX_TAIL_DRAIN_TIMESLOTS = 12` (`N=4`, `N-1=3` traffic-frame recurrences).
  - Added drain handlers that delay peer-facing `D-TX CEASED` and peer `D-DISCONNECT`.
  - If `U-DISCONNECT` arrives while a same-speaker `U-TX CEASED` drain is pending, the pending `D-TX CEASED` is cancelled and peer clear uses the original drain start time.
  - Prompt `D-RELEASE` to the disconnecting MS remains immediate.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - `U-TX CEASED` with no queued requester now starts tail drain before sending `D-TX CEASED`.
  - If a requester was already queued before `U-TX CEASED`, the ETSI handoff path with `D-TX GRANTED` remains immediate.
  - Floor requests and duplicate disconnects are suppressed while private disconnect clear is pending.
  - `U-DISCONNECT` from the current simplex floor holder sends initiator `D-RELEASE` promptly, then delays peer `D-DISCONNECT` until the tail drain expires.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - CMCE tick now drains private simplex tail queues before normal release/delivery timeout handling.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated private simplex workflow tests to assert prompt initiator `D-RELEASE`, no immediate peer `D-DISCONNECT`, and tail-drained peer clear.
  - Updated idle-floor tests to wait for delayed `D-TX CEASED`.
  - Kept queued floor-handoff tests immediate via `D-TX GRANTED`.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 61 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 123 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Retest exact Motorola case: private simplex `2260616 -> 2260618`, talk both ways, then close with red button.
3. Expected live log:
   - no immediate peer `D-DISCONNECT` in the same tick as floor-holder `U-DISCONNECT`,
   - prompt `D-RELEASE` to the disconnecting ISSI,
   - peer `D-DISCONNECT` after the short tail drain,
   - peer `U-RELEASE`,
   - no fallback timeout,
   - no MXP600 soft reboot and no `No answer`.
4. If MXP600 still reboots, inspect whether it is the `D-RELEASE` recipient or the peer `D-DISCONNECT` recipient in that exact trace before changing sequencing again.

## 2026-06-04 22:22:30 EEST - Private simplex tail-drain test build deployed

Deployment:

- Committed patch: `a3bc407 fix: tail-drain private simplex clear`.
- Deployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `33d97d5e722fb357423c5c5baa355ad3de060f4926ec95d39dbe6aa59f37eea1`.
- Remote build banner: `Build: v0.1.55-a3bc4078`.
- Test service path: `/home/chris/nexus-bs-v0.1.55-test`.

Post-start live state:

- `nexus-bs` running with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- `nexus-bs-control-service` running on `127.0.0.1:9002`.
- Post-start log showed:
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.
  - `2260618` registered and affiliated to `226333`.

Next non-repeating execution:

1. User retests private simplex `2260616 -> 2260618`.
2. Watch logs for `U-TX CEASED`, tail-drain debug/info, prompt `D-RELEASE`, delayed peer `D-DISCONNECT`, peer `U-RELEASE`, and absence of fallback timeout.
3. If MXP600 still soft reboots, capture exact recipient of the last downlink PDU before reattach.

## 2026-06-04 22:49:59 EEST - Private simplex peer-floor clear uses D-RELEASE

Problem targeted:

- Field retest: private simplex `2260616 -> 2260618` voice worked, but when `2260616` ended the call with the red key, peer `2260618` Motorola MXP600 soft rebooted.
- The risky case is peer-floor shutdown: the peer may still be the current simplex floor holder while the disconnecting MS receives prompt `D-RELEASE`.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: after `U-DISCONNECT`, the disconnecting MS waits for `D-RELEASE`; the SwMI should inform the other MS of call clearance either by `D-DISCONNECT` or by `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: `D-DISCONNECT` requires `U-RELEASE`, while `D-RELEASE` requires no response.
- EN 300 392-2 clause 23.8.5: after `U-TX CEASED` or `U-DISCONNECT` from a transmitting MS, BS should drain `N-1` traffic slots before sending `D-TX CEASED`, `D-RELEASE`, or `D-DISCONNECT` to receiving MSs.
- This is clause-scoped hardening plus a bounded Motorola compatibility guard; it is not formal certification evidence.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added `IndividualDisconnectPeerClear::{Disconnect, Release}` and peer-clear reporter state for pending private disconnects.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Tail-drained private disconnect can now complete peer clear with `D-RELEASE` instead of `D-DISCONNECT`.
  - Peer `D-RELEASE` is reporter-tracked; the traffic circuit closes only after both the prompt initiator `D-RELEASE` and peer `D-RELEASE` transmit, or after the bounded local delivery guard.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - If a simplex private peer is the current floor holder and the other MS disconnects, BS now tail-drains then sends peer `D-RELEASE`.
  - Duplicate `U-DISCONNECT`, `U-TX DEMAND`, and `U-TX CEASED` are suppressed while the peer-release clear is pending.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated MXP600 field regression to require zero peer `D-DISCONNECT` and a tail-drained peer `D-RELEASE`.
  - Updated symmetric called-party disconnect test for floor-holder peer release.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_tail_drains_when_mxp600_peer_holds_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_called_party_u_disconnect_waits_for_caller_release_before_circuit_close --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 62 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 124 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Retest private simplex `2260616 -> 2260618`; make `2260618` talk last, then close from `2260616`.
3. Expected live behavior: prompt `D-RELEASE` to `2260616`, no `D-DISCONNECT` to `2260618`, tail-drained `D-RELEASE` to `2260618`, no MXP600 soft reboot.
4. Continue SDS/LLC hardening next: SDS status-preserving Brew forward and bounded LLC duplicate suppression.

## 2026-06-04 22:52:49 EEST - Private peer-floor D-RELEASE build deployed

Deployment:

- Committed patch: `5f03000 fix: release private peer floor holder`.
- Deployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `cc58d4a85adb9cc096b16d885b41d2b6ed9fef8f7947007dcea3ed31cb0f2b3f`.
- Remote build banner: `Build: v0.1.55-5f03000c`.
- Test service path: `/home/chris/nexus-bs-v0.1.55-test`.

Post-start live state:

- `nexus-bs` running with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- `nexus-bs-control-service` running on `127.0.0.1:9002`.
- Post-start log showed:
  - `2260618` registered and affiliated to `226333`.
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.

Next non-repeating execution:

1. User retests private simplex `2260616 -> 2260618`.
2. Expected log around hangup: `U-DISCONNECT` from `2260616`, prompt `D-RELEASE` to `2260616`, no `D-DISCONNECT` to `2260618`, tail-drained peer `D-RELEASE` to `2260618`, circuit close only after D-RELEASE reporter completion or bounded local guard.
3. If MXP600 still reboots, inspect the last 20 seconds of `2260618` downlink and registration log before making another protocol change.

## 2026-06-04 23:53:42 EEST - P2P/group floor-control BL-UDATA repeat guard

Problem targeted:

- Live private simplex `2260618 -> 2260616` at `23:37:36` reached `U-CONNECT` and opened the traffic bearer.
- First speaker `2260618` sent voice normally on ts=2.
- At `23:37:49.500`, `2260618` sent `U-TX CEASED`; CMCE tail-drained and sent `D-TX CEASED` to both MSs.
- At `23:37:49.952`, `2260616` sent `U-TX DEMAND` and CMCE granted the floor, but stale `D-TX CEASED` BL-UDATA repetitions were still interleaved after the new `D-TX GRANTED`.
- Result: no TCH/S voice followed from `2260616`, UMAC timed out at `23:37:53.012`, and later `U-TX CEASED` from `2260616` was ignored because CMCE had already cleared the floor.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b/e: simplex individual-call floor control uses `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and `D-TX CEASED`; a queued handover may grant the next MS without a separate `D-TX CEASED`.
- EN 300 392-2 clause 14.5.2.2.1: group-call floor control uses the same request/grant/cease pattern for one speaker at a time.
- EN 300 392-2 clause 22.3.2.4.1 and Annex A.2: for unacknowledged BL-UDATA, `N.253 + 1` complete transmissions are sent; an explicit `N.253=0` means one complete transmission.
- This patch is clause-scoped hardening of time-sensitive floor-control delivery; it is not formal ETSI certification evidence.

Patch implemented:

- `crates/tetra-saps/src/lcmc/mod.rs`
  - Added optional `LcmcMleUnitdataReq.unacked_bl_repetitions`.
- `crates/tetra-entities/src/mle/mle_bs.rs` and `crates/tetra-entities/src/mle/mle_ms.rs`
  - Pass CMCE's explicit unacknowledged BL repetition request through to LLC as `n_tlsdu_repeats`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex `D-TX GRANTED` and `D-TX CEASED` now request `N.253=0`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs` and `shared.rs`
  - Group FACCH `D-TX GRANTED`, `D-TX INTERRUPT`, and `D-TX CEASED` now request `N.253=0`.
- Other CMCE/SDS/MM-originating LCMC messages keep `None`, so setup/release/status retain the existing LLC default repetition behavior.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_mle_bs lcmc_unacknowledged --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 49 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 125 passed.
- `cargo test -p tetra-entities --test test_mle_bs --locked` -> 27 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 80 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this floor-control repeat guard.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex both directions on `2260616`/`2260618`: expected live log after reverse PTT is `D-TX GRANTED` followed by TCH/S voice, with no stale post-grant `D-TX CEASED` repeats to the newly granted MS.
4. Retest group `226333` alternating PTT between two terminals: expected no first-return `PTT denied`, no stale `D-TX CEASED` after grant, and no static-only talk spurt.

## 2026-06-05 00:19:24 EEST - Floor-control repeat guard deployed to RF test BS

Deployment:

- Committed patch: `51f8eb8 fix: single-shot floor-control BL-UDATA`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `1f669e251b134a0af947fe796eb1a6313e8f8ea40318bbf74ff0ffe7f2e13d55`.
- Remote build banner: `Build: v0.1.55-51f8eb8f`.
- Remote processes after restart: `nexus-bs-control-service` on `127.0.0.1:9002`, `nexus-bs` with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.

Post-start live state:

- `2260618` registered and affiliated to `226333`.
- `2260082` registered and affiliated to `226333`.
- `2260616` registered and affiliated to `226333`.
- A bounded 140 s live tail after deploy saw no new `U-SETUP`, `U-TX DEMAND`, `PTT denied`, `Service unavailable`, or `Unit Not Attached` event, so no RF private-simplex retest has been observed in the post-deploy log yet.

Next non-repeating execution:

1. Run private simplex RF retest `2260616 <-> 2260618`, both PTT directions.
2. Inspect the post-deploy log for the first reverse-PTT sequence: expected `U-TX DEMAND` then `D-TX GRANTED` then TCH/S voice, with no stale `D-TX CEASED` emitted after the new grant.
3. If P2P still fails, patch the next proven layer only: likely UMAC bearer/speaker gating or CMCE floor-holder state, not another LLC repetition change without fresh log evidence.
4. Then retest group `226333` alternating PTT for the same stale floor-control pattern.

## 2026-06-05 00:32:46 EEST - P2P simplex crossed-timeslot floor media cleanup

Problem targeted:

- User reported current P2P simplex is broken after the floor-control repeat guard deployment.
- The live post-deploy log still showed no `USetup`/`UTxDemand` P2P sequence in the current process log; a bounded 90 s tail saw only broadcasts, Brew deregisters, and one isolated TCH burst.
- Code inspection found a missing UMAC case for local P2P simplex when the two MSs are on separate assigned timeslots: floor release/grant cleanup only cleared the source UL timeslot, while downlink speech for that source is queued on the crossed peer timeslot.
- That can leave old-speaker raw TCH/S queued on the peer DL timeslot across `D-TX CEASED`/`D-TX GRANTED`, matching the static/no-voice symptom class.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b/e: private simplex request-to-transmit/floor release must switch the single authorized speaker cleanly.
- EN 300 392-2 clause 23.5: assigned traffic channels carry FACCH/STCH signalling during floor control.
- EN 300 392-2 clause 23.8.5: TCH/S media timing/half-slot handling must not be carried across an obsolete floor epoch.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `floor_media_timeslots(ts)` to include the peer timeslot for local P2P cross-route circuits.
  - `FloorReleased` now clears DL media, enters hangtime, clears UL inactivity state, and clears current STCH speaker state on both the source and crossed peer timeslots.
  - `FloorGranted` now clears stale media and exits hangtime on both affected timeslots before accepting the new floor holder.
  - `CallEnded` now clears both affected timeslots for crossed P2P circuits.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_private_simplex_cross_route_floor_release_purges_peer_dl_media`.
  - Added `test_private_simplex_cross_route_floor_grant_keeps_new_peer_audio`.
- `crates/tetra-saps/src/control/call_control.rs` and `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Updated internal comments: `peer_ts` is for local P2P cross-routing, including simplex calls on separate assigned timeslots, not only duplex.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 49 passed.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC crossed-timeslot cleanup.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex `2260616 <-> 2260618`: expected reverse PTT has `U-TX DEMAND`, `D-TX GRANTED`, then TCH/S routed to the peer TS with no old raw TCH/S from the previous floor.
4. If P2P setup still does not appear in the log, instrument/inspect the MAC/LLC decode path before changing CMCE floor semantics again.

## 2026-06-05 00:34:40 EEST - Crossed P2P media cleanup deployed to RF test BS

Deployment:

- Committed patch: `82297b5 fix: clear crossed P2P floor media`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `8317e81208d92ef5e4ec7839e2ee1037bcd3a0f5117b772884e7ea8614bebb67`.
- Remote build banner: `Build: v0.1.55-82297b54`.
- Remote processes after restart: `nexus-bs-control-service` on `127.0.0.1:9002`, `nexus-bs` with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.

Post-start live state:

- `2260618` registered and affiliated to `226333`.
- `2260616` registered and affiliated to `226333`.
- `2260082` registered and affiliated to `226333`.
- A bounded post-deploy tail for P2P/floor-control/audio-route events expired without observing a new private simplex attempt.

Next non-repeating execution:

1. RF retest private simplex both directions between `2260616` and `2260618`.
2. Expected on reverse PTT: `U-TX DEMAND`, `D-TX GRANTED`, `UMAC floor granted`, then `UMAC voice route` from granted UL TS to peer DL TS.
3. If terminal still shows PTT denied or no P2P setup appears, collect a fresh bounded log around the attempt and inspect MAC/LLC decode before another CMCE/LLC semantic patch.

## 2026-06-05 00:42:56 EEST - UMAC invalid TCH/S no longer refreshes floor voice timer

Problem targeted:

- Current post-deploy RF log still has no new private simplex setup/floor-control sequence after `82297b5`.
- Code inspection showed UMAC refreshed `last_ul_voice` immediately after any `TmdCircuitDataInd` on an active UL circuit, before validating that the media was a supported TCH/S payload and before scheduling it to downlink or forwarding it to Brew.
- Unsupported UL voice could therefore mask the BS-side inactivity timeout for a simplex private floor holder and keep floor state alive while no valid speech was delivered.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex private floor ownership must be released/handoff-driven when the current speaker stops transmitting valid speech.
- EN 300 392-2 clauses 23.8.3 and 23.8.5: bad/unsupported TCH/S media must not be treated as clean speech on the downlink path.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Validates UL media before treating it as voice activity.
  - For full-slot ACELP, accepts only payloads that `pack_ul_acelp_bits` can pack and forwards to Brew only after validation.
  - For raw TCH/S half-slot media, accepts only `Block2` with 216 bits.
  - Refreshes `last_ul_voice` only after valid media is actually delivered to Brew or scheduled to a downlink circuit.
  - Refreshes the peer timeslot timer for crossed P2P only after successful media delivery.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_unsupported_ul_voice_does_not_refresh_inactivity_timer`.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs unsupported_ul_voice --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 50 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC invalid-media timer guard.
2. Decide deploy after commit: deploy if we want the RF BS to include this guard before the next private/group PTT retest.
3. Continue next hardening candidate if no RF retest evidence arrives: CMCE Brew-routed private simplex initial `floor_holder`, or UMAC media admission requiring explicit floor epochs for local-loopback traffic.

## 2026-06-05 01:05:00 EEST - CMCE Brew-routed private simplex initial floor hardening

Live diagnostic:

- Remote test BS is running `Build: v0.1.55-2cef71d3`.
- The current log after restart shows `2260082`, `2260616`, and `2260618` re-registering/affiliating to `226333`, plus one `UFacility` from `2260618`; no fresh `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, `UMAC voice route`, or P2P media sequence was present in the bounded log search.
- Live config has `call_preemptive = false`; no private/group pre-emption was enabled.

Problem targeted:

- Code audit found a real CMCE gap on Brew-routed private simplex paths: the call became active and UMAC opened a SwMI-backed bearer, but CMCE did not seed `floor_holder` from the `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE` transmission grant.
- With `floor_holder=None`, a granted local MS could later send `U-TX CEASED` and have it ignored, preventing clean floor release/tail-drain behavior. This can affect P2P simplex when a local destination is temporarily not recognized as local and the setup falls through to Brew routing.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex private-call transmission permission is controlled by the SwMI, and `U-TX CEASED` must be handled for the MS that owns the floor.
- EN 300 392-2 tables 14.80 and 14.81: `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE` transmission grant and request permission must drive the initial transmit permission state.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
  - Added `network_circuit_grant()` to decode Brew/TetraPack grant values through the CMCE `TransmissionGrant` enum.
  - Added `apply_brew_simplex_initial_floor()` to seed `floor_holder` for Brew-routed private simplex calls from the actual connect grant.
  - Local-origin Brew private connect now preserves `call_info.grant` and `call_info.permission` in `D-CONNECT` and the Brew connect confirm instead of hardcoding granted/no-permission.
  - Network-origin Brew private connect confirm now seeds local floor state after opening the SwMI bearer.
  - `FloorGranted` is emitted to UMAC only when the local MS is the granted initial speaker.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_local_origin_brew_private_simplex_connect_sets_initial_floor`.
  - Added `test_network_origin_brew_private_simplex_connect_confirm_sets_initial_floor`.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs brew_private_simplex --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 127 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit the CMCE Brew private simplex floor patch.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Confirm remote build banner and SHA.
4. Retest private simplex `2260616 <-> 2260618`. If no `U-SETUP` appears, diagnose MAC/LLC decode before changing floor semantics again.

## 2026-06-05 01:09:30 EEST - MM recovery preserves subscriber state during failed accept reprobe

Live diagnostic:

- Remote test BS was running `Build: v0.1.55-dd16dae5`.
- The bounded log search after the build marker found no private simplex call-control sequence: no `U-SETUP`, `D-SETUP`, `U-CONNECT`, `D-CONNECT`, `U-TX`, `D-TX`, `FloorGranted`, or `UMAC voice route`.
- The same log showed `2260616` reappearing on RF with good RSSI and MAC access, then being deaffiliated/deregistered after a failed `D-LOCATION UPDATE ACCEPT` transfer report, then soft re-attaching shortly after.
- This means the observed "broken P2P simplex" path was blocked before CMCE private-call setup: MM/LLC recovery temporarily removed a live terminal from CMCE subscriber routing.

Problem targeted:

- `mark_registration_unconfirmed_and_reprobe()` sent a new `D-LOCATION-UPDATE-COMMAND` after failed delivery of an acknowledged `D-LOCATION UPDATE ACCEPT`, but also emitted `Deaffiliate` and `Deregister` and removed the shared subscriber.
- That made CMCE forget the ISSI and its GSSI during the recovery window even while the MS was still transmitting MAC access and later ACKs. A private or group PTT during that window could be rejected as not attached/not affiliated.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: the SwMI may initiate a location update with `D-LOCATION-UPDATE-COMMAND`; the command is the recovery procedure, not proof that the existing MS context must be torn down immediately.
- EN 300 392-2 clauses 16.9.2.8 and 16.9.3.4: `DemandLocationUpdating` is the MS response path for BS-initiated location update; subscriber routing should remain coherent until a detach, reject, timeout, or completed replacement update says otherwise.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - `mark_registration_unconfirmed_and_reprobe()` still fails open to `StayAlive`, sends `D-LOCATION-UPDATE-COMMAND`, abandons stale pending group transactions, and marks the command pending.
  - It no longer emits immediate `Deaffiliate`/`Deregister` or removes the shared subscriber during the reprobe window.
  - It logs that provisional subscriber state is preserved while the registration reprobe is pending.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated `test_restart_recovery_failed_location_update_accept_reprobes_registration` to assert that a failed accept transfer reprobes without dropping CMCE/Brew subscriber routing.
  - The test now verifies that ISSI registration and GSSI affiliation survive the reprobe, while energy saving is cleared to StayAlive.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_failed_location_update_accept_reprobes_registration --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 112 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy this MM recovery hardening directly to the test BS.
2. Confirm the remote build banner and watch for `2260616` recovery without CMCE deregister/deaffiliate churn.
3. Retest private simplex only after a fresh `U-SETUP -> D-CONNECT -> U-TX/D-TX -> UMAC voice route` sequence appears in the log.

## 2026-06-05 09:23:15 EEST - CMCE private simplex initial floor aligned with ETSI raw request bit

Live diagnostic:

- The recent private simplex log for `2260616 -> 2260618` showed `U-SETUP` with `hook_method_selection=true` and `request_to_transmit_send_data=true`, then `D-SETUP` with `transmission_grant=NotGranted`.
- The same call then opened the shared P2P traffic bearer and routed voice, but CMCE only set `floor_holder`; it did not emit an internal `CallControl::FloorGranted` to UMAC for the initial setup grant.
- This left CMCE and UMAC relying on `Open.active_addr` instead of the same floor event model used by group and Brew private paths.

Problem targeted:

- The local `U-SETUP` raw request-to-transmit/send-data bit was being interpreted from the Rust field name rather than ETSI table 14.74.
- EN 300 392-2 table 14.74 defines raw value `0` as "request to transmit/send data" and raw value `1` as "request that other MS may transmit/send data".
- For on/off-hook private simplex, clause 14.5.1.2.1 uses that field to decide the setup-phase transmit permission. A raw `1` in the lab trace means the called MS may transmit first, not the caller.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.1.1: `D-CONNECT ACKNOWLEDGE` tells the called MS which party is permitted to transmit and triggers lower-layer configuration at through-connection.
- EN 300 392-2 clause 14.5.1.1.2: `D-CONNECT` tells the calling MS which party is permitted to transmit and triggers lower-layer configuration at through-connection.
- EN 300 392-2 clause 14.5.1.2.1 and table 14.74: the SwMI controls private simplex transmit permission; on/off-hook setup interprets raw request-to-transmit/send-data values as setup permission direction.
- EN 300 392-2 clause 14.5.1.2.1 also says the SwMI shall not send unsolicited `D-TX GRANTED`; this patch does not send an over-air `D-TX GRANTED` at setup. It emits only internal CMCE-to-UMAC floor synchronization after `U-CONNECT`.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-pdus/src/cmce/pdus/u_setup.rs`
  - Corrected the `USetup::request_to_transmit_send_data` field comment to document the raw table 14.74 values.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Corrected `IndividualCall::request_to_transmit_send_data` documentation to avoid treating the bool as a semantic "caller requested" flag.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Added `private_simplex_called_ms_transmits_first()` to keep setup and connect semantics in one place.
  - Local `U-CONNECT` now uses that helper for initial floor-holder selection.
  - After setting initial simplex `floor_holder`, CMCE now emits internal `CallControl::FloorGranted` to UMAC so hangtime/media queues/current speaker are synchronized with the grant carried by `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Local P2P `D-SETUP` now carries a setup-phase grant only for on/off-hook simplex:
    - raw bit `0`: called MS sees `GrantedToOtherUser`;
    - raw bit `1`: called MS sees `Granted`;
    - direct setup and duplex stay `NotGranted` until `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added assertions that local P2P `U-CONNECT` emits the initial UMAC `FloorGranted`.
  - Corrected on/off-hook raw bit `0` and raw bit `1` tests against table 14.74.
  - Added `test_simplex_p2p_current_floor_holder_u_tx_demand_is_granted_not_denied`.

Verification:

- `cargo fmt --package tetra-entities --package tetra-pdus` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 64 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 128 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Remaining risk / next non-repeating execution:

1. Commit and deploy directly to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Retest private simplex `2260616 -> 2260618` with the same sequence that produced the bad first floor.
3. Watch live logs for `U-SETUP request_to_transmit_send_data=true`, `D-SETUP transmission_grant=Granted`, `D-CONNECT transmission_grant=GrantedToOtherUser`, `D-CONNECT-ACKNOWLEDGE transmission_grant=Granted`, and `UMAC floor granted source_issi=2260618`.
4. If static persists after the floor fix, do not change CMCE again first; inspect the remaining LMAC/UMAC raw `NormalTrainSeq2 Block2` path. That path preserves ETSI TCH/S half-slot timing but the current SAP cannot carry a bad-half-slot condition, so a future patch should add an explicit quality/condition field instead of treating unknown raw half-slots as clean speech.

## 2026-06-05 10:11:00 EEST - MM restart group report completion keeps GSSI after BS restart

Field symptom:

- After a BS restart, terminals showed attached to the network but displayed `No Group`.
- The post-restart log showed terminals sending `U-LOCATION UPDATE DEMAND` with GSSI `226333`, then a follow-up `U-ATTACH/DETACH GROUP IDENTITY` carrying the same GSSI plus `group_report_response = complete`.
- The BS accepted the first group list but cleared the local solicited group-report window too early, then rejected the follow-up complete PDU as a mixed MS-initiated attach/detach request.

Component explanation:

- MM is Mobile Management. It owns terminal registration, energy saving mode negotiation, and group affiliation state.
- Group report recovery is the restart path where the BS asks a still-camped terminal to restate its active groups so CMCE and the dashboard know which GSSI listeners exist.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: after `D-LOCATION UPDATE COMMAND` with group report request, the MS may report group identities in `U-LOCATION UPDATE DEMAND` or by `U-ATTACH/DETACH GROUP IDENTITY`.
- Clause 16.4.4 further says if all reported groups fit in one PDU, the PDU contains `group report complete`; otherwise a final follow-up PDU may carry completion.
- Clause 16.8.3 defines the SwMI-initiated group report response using `U-ATTACH/DETACH GROUP IDENTITY`.
- Clause 16.10.27a defines `group_report_response` value `0` as group report complete.
- This is clause-scoped hardening and test evidence, not a formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - `U-LOCATION UPDATE DEMAND` with group identities but without `group_report_response = complete` no longer clears the solicited group-report window.
  - `U-ATTACH/DETACH GROUP IDENTITY` with group identities plus `group_report_response = complete` is accepted only while that solicited group-report window is pending.
  - The existing reject path remains for the same mixed PDU outside the SwMI-requested report window.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added restart-recovery coverage for the real split sequence: `D-LOCATION UPDATE COMMAND`, then `U-LOCATION UPDATE DEMAND` with GSSI but no complete, then `U-ATTACH/DETACH GROUP IDENTITY` with GSSI and complete.
  - Updated the EG3 restart recovery test so group identities without complete keep the report window open.
  - Kept the negative non-solicited mixed-PDU rejection test.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs group_report --locked` -> 24 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 113 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy directly to `/home/chris/nexus-bs-v0.1.55-test` with EG7 config preserved.
2. Restart the test BS and verify post-restart logs no longer contain `Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY` for `2260082`, `2260616`, or `2260618`.
3. Confirm terminals show GSSI `226333` rather than `No Group` after restart and before any PTT test.

## 2026-06-05 10:11:04 EEST - Post-deploy restart log confirms group affiliation recovery

Field validation:

- Remote test BS is running `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- Remote build banner: `Build: v0.1.55-79272974`.
- Remote config keeps the hard EG test case enabled: `energy_saving_mode = "eg7"` and `call_preemptive = false`.
- Local inspected log copy: `/private/tmp/nexus-bs-current.log`.

Component explanation:

- MM restart recovery asks already-camped terminals to re-state their registration and group identities after BS restart.
- CMCE consumes MM `Register` and `Affiliate` updates so group call control and dashboard state know which ISSIs are valid listeners for a GSSI.

Observed result after the latest restart:

- `2260616` sent `U-LOCATION UPDATE DEMAND` with GSSI `226333`; BS sent `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept`; CMCE registered and affiliated `2260616` to `[226333]`.
- `2260082` answered the BS recovery command with `DemandLocationUpdating` and GSSI `226333`; BS accepted and affiliated it to `[226333]`.
- `2260618` re-registered with GSSI `226333`; BS accepted and affiliated it to `[226333]`.
- The prior blocker sequence was present for `2260082`: follow-up `U-ATTACH/DETACH GROUP IDENTITY` with `group_report_response len=1 data=0` and GSSI `226333`.
- The new code accepted that final group-report-complete PDU with `solicited=true`, sent `D-ATTACH/DETACH GROUP IDENTITY ACK` with `group_identity_accept_reject=0`, and left CMCE affiliated to `[226333]`.

Negative checks:

- No `Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY` appeared in the current post-restart log.
- No `PTT denied`, `RequestedServiceNotAvailable`, `Service unavailable`, `Unit Not Attached`, `No answer`, or `ERROR` appeared in the current post-restart log slice.

Remaining risk / next non-repeating execution:

1. User should confirm the terminal UI now shows group `226333`, not `No Group`, immediately after BS restart.
2. If any terminal still displays `No Group`, capture a fresh full log from the new restart before patching; check whether dashboard display state diverges from MM/CMCE affiliate state.
3. If the group display is fixed, continue field validation with EG7 active: group PTT turn-taking, private simplex/duplex, SDS/WAP smoke, and longer soak.

## 2026-06-05 10:45:39 EEST - Dashboard and MM restart cache hardened for `No Group` after restart

Field symptom:

- User reported that after BS restart the stations appeared attached but with `No Group`.
- Fresh remote log `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log`, copied to `/private/tmp/nexus-bs-current.log`, started at `10:07:57` with build `v0.1.55-79272974` and `energy_saving_mode = "eg7"`.
- The active remote restart cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` was still old format: only `2260082`, `2260616`, `2260618`, no cached GSSI.

Log finding:

- MM/CMCE did not lose the group in this restart slice.
- `2260616`, `2260082`, and `2260618` each sent location update with GSSI `226333`; MM sent `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate ... groups=[226333]`.
- Therefore the observed `No Group` was either dashboard/browser event-order display loss, or a restart-cache risk for cases where an MS answers the restart recovery command without a fresh group report.

Component explanation:

- MM is Mobile Management: it owns terminal registration, group affiliation state, and the local restart-recovery cache used after BS restart.
- CMCE is call control: it consumes MM register/affiliate updates so group/private calls know which terminals are valid participants.
- Dashboard telemetry is observability only: it must accurately show MM/CMCE state, but it is not the ETSI air-interface procedure.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: BS-commanded registration can request a group identity report after restart.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a: group identities reported/accepted during MM attach/group-report procedures remain the authority for affiliation state.
- Clauses 16.10.19 and 16.10.20: accepted group attachment information and reject reasons must be coherent; this patch preserves accepted `GroupAttachmentInfo` in the restart cache and does not fabricate an over-air GSSI accept when the MS did not report one.
- This is clause-scoped hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Restart recovery cache now supports `ISSI GSSI:lifetime:class_of_usage`.
  - Successful group affiliation persists the current GSSI/class/lifetime in the local cache.
  - Legacy `ISSI`-only cache entries remain valid.
  - If a restarted BS has a cached accepted GSSI and a solicited `DemandLocationUpdating` response arrives without a fresh group report, MM restores only local routing/CMCE affiliation from the cache; it does not add a fake `GroupIdentityLocationAccept` to that over-air response.
  - Explicit empty complete reports clear cached groups; explicit group reports replace cached groups.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Dashboard state now creates/preserves an MS entry when `MsGroupAttach` or `MsGroupsSnapshot` arrives before `MsRegistration`, instead of later showing `No Group`.
- `crates/tetra-entities/src/net_dashboard/html.rs`
  - Browser-side WS handling now uses `ensureMsEntry()` for `ms_groups` and non-empty `ms_groups_all`, so a live browser does not drop group events that race ahead of `ms_registered`.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added restart-cache GSSI persistence/restoration/empty-clear/replacement coverage.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added server-state and shipped-HTML regression tests for group-before-registration ordering.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 14 passed.
- `cargo test -p tetra-entities --lib dashboard_ --locked` -> 8 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Deploy this local patch directly to `/home/chris/nexus-bs-v0.1.55-test` without compiling on the Pi.
2. Restart the test BS with EG7 still active and confirm the cache rewrites to `2260616 226333:0:4`, `2260082 226333:0:4`, `2260618 226333:0:4` or equivalent class values after the terminals report.
3. Re-open the dashboard after restart and verify all three stations show `226333`, not `No Group`.
4. If a terminal itself, not the dashboard, still displays `No Group`, inspect whether it received/ACKed `D-LOCATION UPDATE ACCEPT` and whether it later sends an explicit empty group-report-complete.

## 2026-06-05 10:54:12 EEST - Restart candidate self-attach without groups no longer clears cached GSSI

Additional audit finding:

- A read-only MM audit found a remaining race: a terminal can self-attach before BS sends the startup `D-LOCATION UPDATE COMMAND`.
- If that early self-attach is `U-LOCATION UPDATE DEMAND` / `ITSI attach` with no group identities, old logic did not treat it as a solicited restart recovery response and could call `remember_restart_recovery_issi()` with an empty local group set.
- With EG7, that missed group-report command can persist longer because the terminal sleeps more aggressively after the initial attach.

Component explanation:

- MM restart recovery has two valid field orders:
  - BS first: BS sends `D-LOCATION UPDATE COMMAND(group identity report=1)`, then MS answers.
  - MS first: still-camped MS sends its own attach/update before the BS command is due.
- The second order must not erase the restart cache just because the MS did not include a fresh GSSI in that first PDU.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4 permits SwMI-commanded registration and group identity report.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a keep explicit group reports/complete reports authoritative.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7 and 23.7.6/T.210 constrain the EG7 interaction: group-report recovery must be scheduled before a BS-initiated EG request can make the MS harder to reach.
- This remains clause-scoped hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Captures `was_restart_recovery_candidate` before registration removes the candidate from the recovery map.
  - For a new, group-less restart candidate, restores cached GSSI locally even if no `D-LOCATION UPDATE COMMAND` was already pending and even if the LU type is `ITSI attach`.
  - Still does not fabricate `GroupIdentityLocationAccept`; the explicit MS group report or empty complete report remains authoritative.
  - Queues `D-LOCATION UPDATE COMMAND(group_identity_report=1)` for a restart candidate that self-attaches without groups.
  - Queues that group-report command before a configured BS-initiated `D-MM STATUS` energy-saving request, so EG7 does not hide the terminal before group recovery is requested.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added coverage for unsolicited group-less `ITSI attach` restoring cached GSSI and preserving cache.
  - Added EG7 coverage proving group-report command order precedes `D-MM STATUS`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 16 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this second MM hardening patch.
2. Redeploy to test BS and confirm build id changes from `7d72c06b`.
3. Verify cache stays `ISSI 226333:0:4` for `2260082`, `2260616`, and `2260618` after restart.

## 2026-06-05 10:58:30 EEST - Final deploy blocked by SSH timeout

Execution status:

- Code commit `f02371a` (`fix: recover restart candidate groups before eg`) was created after local verification.
- Local `scripts/nexus-bs-test-deploy.sh` cross-build completed successfully for `nexus-bs v0.1.55`.
- The remote deploy phase failed before copying the new binary because `ssh chris@192.168.1.179` timed out on port 22.
- Two short-timeout SSH retries also timed out.
- Last confirmed remote running build remains `v0.1.55-7d72c06b`; that build already includes the dashboard/cache GSSI persistence patch and had rewritten the cache to:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`

Next non-repeating execution:

1. When `chris@192.168.1.179` is reachable again, rerun `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Confirm remote build id changes from `v0.1.55-7d72c06b` to the current HEAD.
3. Re-read `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` and verify all three ISSIs still persist `226333:0:4`.

## 2026-06-05 11:04:26 EEST - Restart `No Group` status check after user report

User report:

- After BS restart, terminals attach but appear with `No Group`.
- This is the MM/GMM restart-recovery path: MM owns terminal registration and GSSI affiliation state; CMCE consumes MM register/affiliate events for group calls; dashboard only displays that state.

Local findings:

- Repo was clean before inspection.
- Current HEAD is `70ad46f` with the prior MM restart recovery fixes already committed.
- Local code already contains the follow-up fix from `f02371a`:
  - restart candidates are captured before registration removes them from the recovery map;
  - a new group-less restart candidate restores cached GSSI locally when available;
  - an unsolicited group-less `ITSI attach` still gets `D-LOCATION UPDATE COMMAND(group_identity_report=1)`;
  - that group-report command is queued before the configured BS-initiated EG7 `D-MM STATUS` request;
  - no fake over-air `GroupIdentityLocationAccept` is generated when the MS did not report a group.
- Dashboard code already preserves `ms_groups` / group snapshot events if they arrive before `ms_registered`, preventing UI-only `No Group`.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may command registration and request group identity report.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a: explicit group reports and complete empty reports remain authoritative.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7 and 23.7.6/T.210: EG7 scheduling must not hide the MS before group recovery is requested.
- This is engineering evidence for the touched clauses only, not formal TETRA certification.

Verification rerun:

- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 16 passed.
- `cargo test -p tetra-entities --lib dashboard_ --locked` -> 8 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/log status:

- `ssh -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=2 -o ServerAliveCountMax=1 chris@192.168.1.179 date` timed out twice.
- Could not read live logs or deploy current HEAD because port 22 is unreachable.
- Do not patch further from the field symptom alone while local clause-scoped tests already cover it; next execution is deploy current HEAD and inspect the fresh full log from the next restart.

Next non-repeating execution:

1. When SSH returns, deploy directly with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Confirm remote build id is current HEAD, not `v0.1.55-7d72c06b`.
3. Read `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers`; expected steady state after terminal reports is `2260082`, `2260616`, and `2260618` with `226333:0:4` or equivalent class values.
4. Read the fresh full log from the latest restart and compare MM `subscriber affiliate` state with dashboard display if any terminal still shows `No Group`.

## 2026-06-05 12:23:29 EEST - MM restart `No Group` ACK hardening and CMCE proof

User report:

- After BS restart, terminals attach but appear with `No Group`.
- Field concern is especially relevant with `2260616`, `2260082`, `2260618`, group `226333`, and EG7 energy saving.

Components touched:

- MM restart recovery: restores cached GSSI affiliation after restart and runs the SwMI-initiated `D-ATTACH/DETACH GROUP IDENTITY` refresh.
- CMCE group call control: consumes MM `Register`/`Affiliate` updates; if these are missing, group call/PTT behaves like the terminal has no group.
- Restart recovery cache: local persistent memory of `ISSI -> GSSI:lifetime:class_of_usage` used after BS restart.

Patch summary:

- Restart-refresh group ACKs now accept the same ISSI even if the local MLE primitive handle is non-zero and does not match the downlink handle.
- Normal non-restart SwMI group transactions remain strict on matching MLE handle.
- Segmented cached scan-list recovery now preserves groups that were not yet sent over air if an earlier batch fails or T353 expires.
- Added MM tests for non-zero ACK, EG7 restart refresh, segmented success, and T353 segmented failure preservation.
- Added MM+CMCE integration for `226333`: three lab ISSIs recover cached group, ACK with non-matching handles, survive T353, start a group call, and queue return PTT without release/deny.

ETSI clause scope:

- EN 300 392-2 clause 16.8.1: SwMI-initiated attach/detach group identity procedure and ACK request.
- Clauses 16.10.14/16.10.17/16.10.19: ACK type and group identity attachment information.
- Clause 16.11.1.3: T353 expiry handling.
- Clause 14.5.2.2.1: group call floor request/queued transmission behavior after CMCE receives restored affiliation.
- The MLE handle is local stack plumbing, not an over-air ETSI ACK discriminator; the clause-scoped key is the same ISSI and active group identity procedure.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_group_refresh_accepts_unrouted_nonzero_ack_without_t353_purge --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_group_less_demand_segments_cached_scan_list_refresh --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_segmented_group_refresh_t353_preserves_unsent_cached_groups --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 22 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_cmce_bs 226333 --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_ --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this patch.
2. Deploy directly to testing with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Confirm remote build id changes from the old live `v0.1.55-7d72c06b` to the new commit.
4. After restart, read the fresh full log and confirm:
   - `MM: sending SwMI group attach refresh` for cached `226333` when a terminal answers without group IE;
   - `CMCE: subscriber affiliate issi=... groups=[226333]`;
   - no `T353 expired` rollback for accepted same-ISSI ACKs;
   - no `No Group` steady state for `2260082`, `2260616`, `2260618`.

## 2026-06-05 12:26:33 EEST - Deployed restart ACK hardening to test BS

Deployment:

- Commit deployed: `40398d91` (`fix: accept restart group ack handles`).
- Command used: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Build happened locally and binary was copied directly to testing; no Rust compile on Pi and no binary backup step.
- Live header now shows `Build: v0.1.55-40398d91`.

Remote state after restart:

- Running processes:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
  - `nexus-bs-control-service --listen 127.0.0.1:9002`
- Recovery cache:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`

Fresh log review from last restart:

- Full log copied to `/private/tmp/nexus-bs-after-40398d91.log` and read in chunks.
- `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}` appears immediately after startup.
- `2260082` registers with `226333` and CMCE affiliates `groups=[226333]`.
- `2260618` registers with `226333`; one solicited attach/detach completion briefly deaffiliates then immediately reaffiliates `226333`, ending affiliated.
- `2260616` initially misses a few recovery command deliveries, then sends `DemandLocationUpdating` with EG7 and `226333`; CMCE registers and affiliates `groups=[226333]`.
- No `T353 expired`, `No Group`, `PTT denied`, `Unit Not Attached`, `RequestedServiceNotAvailable`, or `service unavailable` found in the fresh restart log.
- Remaining warnings are RF/startup or expected live-radio noise:
  - startup TX late / lost samples;
  - `SX1255 temperature read failed` while streams are active;
  - initial LLC retransmission exhaustion before the radios answer;
  - occasional malformed/short MAC access bursts.

Status:

- The deployed build fixes the MM restart ACK-handle purge path in code and proves, from this restart, that the three lab terminals are affiliated to `226333` at CMCE level.
- This is clause-scoped engineering validation for the touched ETSI procedures, not formal certification.

Next non-repeating execution:

1. User should visually confirm terminals no longer show `No Group` after this restart.
2. If any terminal still displays `No Group`, capture the fresh log interval after that visual state and compare dashboard state vs MM/CMCE affiliation lines.
3. Run a live group PTT test on `226333`; if failure occurs, inspect from the current build log around `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, and UMAC floor events.

## 2026-06-05 12:44:11 EEST - Hardened restart group retention against transient No Group

Component scope:

- MM: Mobility Management owns terminal registration and group affiliation after attach/restart.
- CMCE: call control consumes MM subscriber updates; a short false deaffiliate can make group PTT look unavailable.
- Dashboard telemetry: observability only; it must show the final MM group list, not a local intermediate state.

Patch:

- `client_detach_all_groups_silent()` now lets MM apply ETSI mode=1 as one logical replace operation without first emitting an intermediate empty group telemetry event.
- MM mode=1 handling now keeps retained GSSIs affiliated in shared subscriber/CMCE state when the replacement list contains the same accepted group.
- MM still deaffiliates groups that are actually absent from the replacement list, so explicit empty reports/detach-all remain authoritative.
- Cached restart group restoration now emits a full current group snapshot after replaying cached affiliations, so dashboard clients do not depend only on incremental attach ordering.
- Added tests for standalone and location-update mode=1 refreshes retaining `226333` without CMCE `Deaffiliate -> Affiliate` churn.
- Updated the restart-recovery follow-up test to assert no transient CMCE `No Group` window when the final complete report retains the same GSSI.

ETSI clause scope:

- EN 300 392-2 clause 16.10.17/table 16.49: group identity attach/detach mode=1 means detach all current groups and attach the listed identities as one requested operation.
- Clauses 16.8.0/16.8.4: accepted group identities remain valid attached identities and are represented in the downlink acknowledgement/accept.
- This is a clause-scoped engineering hardening for restart/group-affiliation behavior, not formal certification.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs mode_one_retains_same_group --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 22 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_mm_bs group_identity --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 127 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --lib dashboard --locked` -> 49 passed.
- `cargo test -p tetra-entities --test test_cmce_bs 226333 --locked` -> 2 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM restart/no-transient-group patch.
2. Deploy directly to testing with local build only.
3. After restart, read the full fresh log from process start and confirm:
   - `2260082`, `2260616`, `2260618` end with `groups=[226333]`;
   - no `CMCE: subscriber deaffiliate issi=... groups=[226333]` followed immediately by re-affiliate for retained `226333`;
   - no steady dashboard/WebSocket snapshot with `groups:[]` for the three lab ISSIs;
   - no `PTT denied`, `No Group`, `Unit Not Attached`, or `T353 expired` in the restart interval.

## 2026-06-05 12:54:07 EEST - Split soft re-attach private-call cleanup from group affiliation

Live trigger:

- Deployed `4317457e` and read the full fresh log from Pi start.
- The three lab terminals ended correctly affiliated to `226333` in the WebSocket snapshot.
- Remaining risk found in the fresh log: a soft `RoamingLocationUpdating` from `2260082` emitted an internal CMCE `Deregister -> Register -> Affiliate` sequence in the same millisecond. That can create a tiny dashboard/CMCE `No Group` window even though the final group state is correct.

Component scope:

- MM: registration and group affiliation owner. On soft re-attach, it may need to clear stale private-call state but must not withdraw accepted groups.
- CMCE: call/PTT controller. It now has a separate internal action to release stale individual calls without touching `subscriber_groups` or GSSI listener counts.
- Brew/backhaul: ignores this internal cleanup action; it is not a subscriber deregistration or group-affiliation procedure.

Patch:

- Added internal `ReleaseIndividualCalls` subscriber action.
- MM soft re-attach now emits `ReleaseIndividualCalls` to CMCE instead of simulating `Deregister -> Register -> Affiliate`.
- CMCE handles `ReleaseIndividualCalls` by releasing active individual calls involving the ISSI, while preserving group memberships and listener counts.
- Brew ignores the internal action defensively.
- Updated the MM soft re-attach test so the expected behavior is one private-call cleanup action and no group churn.
- Added a CMCE test proving a group-affiliated MS still receives a queued return PTT grant after private-call cleanup.

ETSI clause scope:

- EN 300 392-2 clauses 16.9.3.4 and 16.10.35a: the soft location update is accepted as the same location-registration update type.
- Clauses 16.8.0/16.8.4: accepted group identities remain valid attached group identities until an explicit detach/replacement procedure changes them.
- The private-call cleanup is a local robustness guard for stale CMCE individual-call state; it is not an over-air ETSI group detach operation and must not be represented as one.
- This is clause-scoped engineering validation only, not formal certification.

Verification:

- `cargo fmt -p tetra-saps -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs soft_roaming_reattach --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs private_call_cleanup_preserves_group_floor_membership --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 127 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 131 passed.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this soft re-attach/no-group-churn patch.
2. Deploy directly to testing with local build only.
3. After restart, read the fresh full Pi log and confirm:
   - build id matches the new commit;
   - `2260082`, `2260616`, `2260618` end with `groups=[226333]`;
   - soft re-attach logs `ReleaseIndividualCalls`/private cleanup and no CMCE `Deregister -> Register -> Affiliate` group churn for retained `226333`;
   - no `No Group`, `Unit Not Attached`, `PTT denied`, `RequestedServiceNotAvailable`, or `T353 expired` during restart recovery.

## 2026-06-05 12:56:41 EEST - Deployed soft re-attach group preservation to test BS

Deployment:

- Commit deployed: `113f2a91` (`fix: preserve groups during soft reattach cleanup`).
- Command used: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Build happened locally and binary was copied directly to testing; no Rust compile on Pi and no binary backup step.
- Live header now shows `Build: v0.1.55-113f2a91`.

Remote restart evidence:

- Fresh full log copied to `/private/tmp/nexus-bs-after-113f2a91.log` and read from process start.
- Recovery cache remains coherent:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Dashboard WebSocket snapshot after restart:
  - `2260082` has `groups:[226333]`;
  - `2260616` has `groups:[226333]`;
  - `2260618` has `groups:[226333]`.
- `2260082` soft re-attach now logs:
  - `MM: requested CMCE individual-call cleanup for ISSI 2260082 while preserving group affiliation (soft re-attach)`;
  - `CMCE: individual-call cleanup issi=2260082 preserved groups=[226333]`.
- The old local `CMCE Deregister -> Register -> Affiliate` churn for `2260082` no longer appears in this restart interval.

Negative log review:

- No `PTT denied`.
- No `No Group`.
- No `Unit Not Attached`.
- No `RequestedServiceNotAvailable`.
- No `Service unavailable`.
- No `T353 expired`.
- No `CMCE: subscriber deaffiliate` or `CMCE: subscriber deregister` for `2260082`, `2260616`, or `2260618` with retained `226333`.

Observed non-blocking warnings:

- Startup TX late / lost samples.
- `SX1255 temperature read failed` while streams are active.
- LLC retransmission exhaustion while radios are not yet answering during restart probing.
- One unexpected ACK from `2260616` after successful registration.
- Short malformed MAC access bursts from live RF.

Status:

- The restart `No Group` transient caused by soft re-attach private-call cleanup has been removed in this live deployment.
- Current validation is engineering evidence scoped to the touched EN 300 392-2 procedures and local CMCE state handling; it is not formal TETRA certification.

Next non-repeating execution:

1. User should test group PTT on `226333` immediately after this restart and report any `PTT denied` or radio-side `No Group`.
2. If a new failure appears, inspect the live interval around `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, `ReleaseIndividualCalls`, and UMAC floor events.
3. Continue broader hardening on remaining basic stack surfaces: long-run registration/affiliation retention, group floor handoff under EG7, private simplex/duplex, SDS, and WAP.

## 2026-06-05 13:54:03 EEST - Live restart No Group report audit on build 7cf2e4a2

User report:

- After BS restart, radios appeared attached but with `No Group`.

Live evidence read from the current test Pi process:

- Running build: `Nexus-BS v0.1.55`, build `v0.1.55-7cf2e4a2`.
- Runtime restart cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` still contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the current restart shows `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
- `2260082` sent `U-LOCATION UPDATE DEMAND` with `GroupIdentityLocationDemand` for `226333`; BS replied with `D-LOCATION UPDATE ACCEPT` carrying `GroupIdentityLocationAccept` for `226333`, then CMCE registered and affiliated `groups=[226333]`.
- `2260618` followed the same location update and group accept path for `226333`.
- `2260616`, configured/requested for EG7, missed early BS-initiated restart probes but later sent `DemandLocationUpdating` with `energy_saving_mode=Eg7` and `GroupIdentityLocationDemand` for `226333`; BS replied with `D-LOCATION UPDATE ACCEPT` carrying EG7 information and `GroupIdentityLocationAccept` for `226333`.
- Dashboard WebSocket snapshot after the restart reports:
  - `2260082 groups=[226333] energy_saving_mode=0`
  - `2260618 groups=[226333] energy_saving_mode=0`
  - `2260616 groups=[226333] energy_saving_mode=7 frame=12 multiframe=19`

Negative log review for the current restart interval:

- No `No Group`.
- No `Unit Not Attached`.
- No `PTT denied`.
- No `RequestedServiceNotAvailable`.
- No `Service unavailable`.
- No `T353 expired`.

Observed but non-blocking:

- EG7 station `2260616` may not hear early restart recovery `D-LOCATION-UPDATE-COMMAND` transmissions until its listen window or next uplink activity; it later completed the standardized location update with the group included and acknowledged.
- `2260082` and `2260618` timed out BS-initiated EG7 assignment and stayed in `StayAlive`; this is expected when those radios do not accept the optional BS-initiated energy saving change.

Technical conclusion:

- The current BS state, CMCE listener state, restart cache, and dashboard snapshot all have `226333` restored. The specific stale dashboard `No Group` path is covered by commit `7cf2e4a2`.
- If a radio screen still shows `No Group` while the BS snapshot has `groups=[226333]`, the next investigation is terminal-side retained display/scan-list state or a short visible interval before the radio receives/ACKs the group-bearing `D-LOCATION UPDATE ACCEPT`, not a lost BS restart cache.

ETSI clause scope:

- EN 300 392-2 clauses 16.9.2.8, 16.9.3.4, and 16.10.35a: location update accept type and accepted location update response path.
- EN 300 392-2 clauses 16.8.0, 16.8.4, and 16.10.17: accepted group identities and attach/detach group identity semantics.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 16.10.35a, 23.5.2.2.7, and 23.7.6: EG7 negotiation/assignment and scheduling awareness.
- Dashboard telemetry remains observability evidence only; it is not formal ETSI conformance evidence.

Next non-repeating execution:

1. Have the user check the actual radio display after the completed location update window, especially `2260616` in EG7.
2. If a radio still shows `No Group`, capture the exact ISSI and wall-clock time, then inspect the log around that station's `U-LOCATION UPDATE DEMAND`, `D-LOCATION UPDATE ACCEPT`, LLC ACK, and any subsequent group report/attach-detach PDU.
3. Continue live group PTT validation on `226333`; if PTT fails, inspect the interval around `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, floor ownership, and UMAC voice grant timing.

## 2026-06-05 14:30:21 EEST - Private simplex called-ISSI End Call release

User symptom:

- Private simplex end-of-call still had two bad peer-side outcomes: the called ISSI could show `Not Answered`, or Motorola MXP600 `2260618` could soft-reset after the remote caller pressed red.
- The high-risk live shape is `2260616 -> 2260618`: caller clears with `U-DISCONNECT`, while `2260618` is the called ISSI and may have recently held or released the simplex floor.

Component, simple technical meaning:

- CMCE/CC-BS is the private-call control state machine inside the BS. It decides which call-control PDU is sent when a terminal opens, speaks in, or ends a private call.
- `U-DISCONNECT` is the uplink terminal request to end the call.
- `D-RELEASE` is the downlink release indication that does not require a terminal response; this is the clean peer-side "end call" path.
- `D-DISCONNECT` is a downlink disconnect request that requires the peer to answer `U-RELEASE`; keeping this path for sensitive caller-hangup-to-called-peer cases was the likely source of the bad UI/reset behavior.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: either calling or called user may initiate individual-call disconnection; the MS that sent `U-DISCONNECT` waits for `D-RELEASE`; the SwMI should inform the other MS of call clearance either by `D-DISCONNECT` or by `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: an MS receiving `D-DISCONNECT` shall respond with `U-RELEASE`; an MS receiving `D-RELEASE` sends no response.
- This patch uses the `D-RELEASE` alternative explicitly allowed by clause 14.5.1.3.1 for the called ISSI in local private simplex caller-hangup cases. It is clause-scoped hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex caller-hangup now detects `sender == calling_addr` and `peer == called_addr`.
  - In that case the called peer is cleared after the existing tail drain with `D-RELEASE`, not `D-DISCONNECT`.
  - The disconnecting caller still gets prompt `D-RELEASE(UserRequestedDisconnection)`.
  - The called peer gets `D-RELEASE(SwmiRequestedDisconnection)`, so the peer sees SwMI call release / end-call semantics instead of a user-request/no-answer-style handshake.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Pending private disconnect tail-drain state now stores a separate peer cause.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Tail-drain completion now uses that separate peer cause when sending peer `D-RELEASE`.
  - A private disconnect now also consumes any pending simplex `U-TX CEASED` tail-drain for the same call. This preserves the bearer-tail wait but suppresses stale `D-TX CEASED` / floor-release signalling after call release has started.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated direct simple private call workflow and unsolicited `U-RELEASE` regression to assert no peer `D-DISCONNECT` for caller-hangup-to-called-ISSI.
  - Added/kept D-DISCONNECT coverage by making the called party disconnect after it has held the floor, so the caller peer path still exercises `D-DISCONNECT -> U-RELEASE` where applicable.
  - Updated MXP600 regressions to require peer `D-RELEASE(SwmiRequestedDisconnection)`.
  - Added overlap regression for `2260616 -> 2260618`: MXP600 peer sends `U-TX CEASED`, caller presses red before tail-drain expiry, and BS sends only peer `D-RELEASE` with no delayed `D-TX CEASED`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_cancels_pending_peer_tx_ceased_tail --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with the normal local-build deploy script.
2. Retest exact field case: `2260616 -> 2260618`, let `2260618` speak last if desired, then press red on `2260616`.
3. Expected live log: prompt `D-RELEASE(UserRequestedDisconnection)` to `2260616`, no peer `D-DISCONNECT` to `2260618`, tail-drained `D-RELEASE(SwmiRequestedDisconnection)` to `2260618`, no peer `U-RELEASE` required, no MXP600 soft reboot, no `Not Answered`.

## 2026-06-05 15:12:44 EEST - Group first-speaker floor retake without immediate back-up D-SETUP

User symptom:

- In GSSI group call, the station that opens/retakes PTT could produce static/no voice, while later interventions by other stations could carry voice.
- The live log slice for `call_id=11`, GSSI `226333`, showed a floor grant to ISSI `2260082`, then a burst of FACCH/STCH signalling with `speech_present=false`, followed by UL inactivity timeout. Earlier and later periods showed normal `UMAC voice route`, so the failure path was tied to the first frames after group floor retake.

Component, simple technical meaning:

- CMCE/CC-BS group floor control decides who is allowed to speak in a group call and sends `D-TX GRANTED` / `D-TX CEASED`.
- UMAC media routing carries actual TCH/S speech bits on the assigned traffic slot after CMCE has granted floor.
- Back-up `D-SETUP` is the group-call late-entry mechanism: radios that missed the original setup can still join an ongoing group call. It is not the immediate floor-grant mechanism.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI controls group transmit permission; the speaking MS gets individually addressed `D-TX GRANTED`, while the group gets `D-TX GRANTED` with "granted to another user".
- EN 300 392-2 clause 14.5.2.1 and Annex D: `D-SETUP` establishes/backs up group-call entry; Annex D describes optional back-up `D-SETUP` for called group members.
- EN 300 392-2 clause 23.8.2.3.1 requires both CC transmit authorization and an uplink-applicable traffic usage marker before an MS transmits traffic.
- This is clause-scoped hardening of the group-call floor path, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Group floor grant paths now update the cached late-entry `D-SETUP` speaker but do not immediately enqueue a fresh group `D-SETUP` in the same burst as `D-TX GRANTED`.
  - Immediate floor retake therefore sends the individual grant to the new speaker and the group grant to listeners without a same-burst setup refresh that can disturb the first speech frames.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - UL-inactivity handoff to a queued group requester follows the same rule: grant floor immediately, refresh cached late-entry setup, defer the actual back-up `D-SETUP` to the normal late-entry scheduler.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Group `D-INFO` T310 reset remains FACCH/STCH but now carries DL-only channel allocation. It is timer/listener signalling, not transmit authorization for all GSSI members.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated group handoff, hangtime retake, repeated same-GSSI setup, and UL-inactivity handoff regressions to assert no immediate back-up `D-SETUP` during the first floor-grant burst.
  - Added/kept checks that the deferred late-entry `D-SETUP` still advertises the new speaker when the retry window runs.
  - Updated group `D-INFO` reset assertions to require DL-only allocation.

P2P/private-call safety:

- No P2P/private-call code path was changed.
- P2P regression suite still passes, including simplex floor handoff, caller/called release, pending disconnect, and MXP600-safe called-peer release tests.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_hangtime_tx_demand_defers_late_entry_d_setup_refresh --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_ul_inactivity_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 52 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this patch, then deploy direct to the test Pi with local build only.
2. Retest GSSI `226333`: initial group setup speech, hangtime retake by the original speaker, and handoff/return PTT by the other station.
3. Expected live log around retake: `D-TX GRANTED` individual + group, `D-INFO` reset T310 DL-only, no immediate `DSetup` in the same floor-grant burst, then early `UMAC voice route` instead of `UL inactivity timeout`.

## 2026-06-05 15:25:08 EEST - P2P/private-call scope guard and group initial-speaker seed

User request:

- Patch must take P2P calls into account while continuing group-call hardening.

Component, simple technical meaning:

- `Circuit.active_addr` is the primary address for an assigned traffic bearer.
- For a private/P2P call this primary address is an ISSI, so only the two private participants may drive floor/audio state.
- For a group call this primary address is a GSSI. The current speaker ISSI can be stored as secondary metadata so EG/listening state is correct, but that must not turn the group bearer into a private-call participant list.
- UMAC scheduler is the component that enforces this distinction before accepting a `FloorGranted` update.
- UMAC also owns STCH `MAC-U-SIGNAL` attribution. STCH signalling has no SSI field, so UMAC must already know the current ISSI speaker when it forwards early traffic-channel signalling.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1: individual/private calls are participant-scoped ISSI-to-ISSI services.
- EN 300 392-2 clause 14.5.2: group calls are P2MP/GSSI-scoped services where different affiliated ISSIs may take transmit permission over the same group bearer.
- EN 300 392-2 clause 23.7.6 remains relevant because assigned-channel activity must keep the correct EG terminals awake.
- This is clause-scoped engineering hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-saps/src/control/call_control.rs`
  - Documented that `active_addr` is the primary bearer scope.
  - Documented that secondary ISSIs are EG/listening metadata and do not by themselves make a group bearer private/P2P-scoped.
  - Documented `Circuit::is_primary_issi_scoped()` as the private/P2P discriminator.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Documented `ul_circuit_is_private_participant_scoped()` as primary-address based, not "any secondary ISSI" based.
  - Added a scheduler regression proving GSSI-primary + speaker-ISSI-secondary remains group-scoped.
  - Added a scheduler regression proving ISSI-primary + peer-ISSI-secondary remains strict private/P2P-scoped and excludes a third ISSI.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - On circuit open, UMAC now seeds the current UL speaker from ISSI-primary circuits for P2P exactly as before.
  - For GSSI-primary group circuits, UMAC seeds the current UL speaker from the first secondary ISSI if present. This covers early STCH before a later `FloorGranted`, without turning the bearer into a private/P2P participant list.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added a regression that a GSSI-primary group `Open` with speaker ISSI secondary forwards immediate STCH `MAC-U-SIGNAL` as that ISSI.
  - Updated the group handoff audio-path test to use the real current group circuit shape: GSSI primary plus first-speaker ISSI secondary.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Tightened group setup helper assertions to require exactly one secondary speaker ISSI.
  - Tightened hook private-simplex setup assertions to require exactly one secondary peer ISSI.

P2P/private-call safety:

- P2P audio and release runtime paths were not changed; the new speaker seed keeps the existing ISSI-primary P2P behavior and adds only the GSSI-primary group secondary fallback.
- The new tests protect the current behavior so future group-call fixes cannot weaken private-call participant filtering.

Verification:

- `cargo fmt --package tetra-saps --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_ul_private_scope --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_uses_secondary_speaker_from_group_open_circuit --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_handoff_reopens_ul_traffic_for_lmac_tch_s_decode --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_uses_current_ul_speaker_from_private_open_circuit --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_grant_accepts_new_speaker_when_initial_speaker_is_secondary --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_ignores_floor_granted_for_non_participant_private_speaker --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_simplex_p2p_current_floor_holder_u_tx_demand_is_granted_not_denied --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_tail_drains_when_mxp600_peer_holds_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_setup_sends_proceeding_connect_and_group_setup_with_allocations --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_hook_setup_other_ms_request_sets_called_ms_initial_floor --locked` -> pass.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Operational note:

- Local `target` filled the disk during parallel Cargo tests. Ran `cargo clean` locally and reran verification sequentially with `CARGO_INCREMENTAL=0`.

Next non-repeating execution:

1. Commit this guard/seed patch.
2. Continue group-call live validation on GSSI `226333`; if static/no-voice persists, inspect the next live log for actual `UMAC voice route` vs FACCH/STCH-only frames rather than changing P2P release paths.
3. Keep P2P regressions in the verification set for every group-call patch.

## 2026-06-05 15:32:30 EEST - P2P/group speaker scope build deployed

Commit deployed:

- `8a53b919` (`fix: preserve p2p scope with group speaker metadata`)

Build/deploy:

- Command:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Local build only; no compile on `chris@192.168.1.179`.
- Remote binary copied directly to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup created.
- Remote deployed binary SHA-256:
  - `b88bc154e727f1fe8c3f21b00e93c1e956773b213cd506ee11e9f231da3ca774`

Live evidence after deploy:

- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Startup build line:
  - `Build: v0.1.55-8a53b919`
- Startup registration/affiliation:
  - `2260618` registered and affiliated to `226333`.
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.
- The post-start filtered log sample showed no `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached`.

Next non-repeating execution:

1. RF retest GSSI `226333`: first PTT after group setup, then alternating PTT between stations.
2. RF retest private simplex P2P `2260616` <-> `2260618`, including reverse PTT and red-key close.
3. If group static/no-voice persists, inspect post-deploy log for `UMAC voice route`, `rx_blk_traffic`, early STCH/FACCH stealing, and `UL inactivity timeout` around the exact PTT window.

## 2026-06-05 15:58:03 EEST - GSSI floor notification hardening for 2260082/MTP3550

Observed live issue:

- User RF test on GSSI `226333` showed repeated PTT static/no-voice only from ISSI `2260082` (Motorola MTP3550).
- The terminal did not display `PTT denied`; UI looked normal for TX/RX.
- Log analysis confirmed this was not a CMCE denial:
  - `U-TX DEMAND` from `2260082` was accepted.
  - CMCE emitted individual `D-TX GRANTED` to `2260082`.
  - UMAC emitted `FloorGranted` for `source_issi=2260082`.
  - After grant, no `NormalTrainSeq*` / `UMAC voice route` appeared before `UL inactivity timeout`.

ETSI clause-scoped reasoning, not formal certification:

- EN 300 392-2 clause 14.5.2.2.1 b): group floor response must send individual `D-TX GRANTED` to the granted MS and group-addressed `D-TX GRANTED` to listeners indicating "granted to another user".
- The same clause notes the group-addressed grant should identify the transmitting party when needed to prevent the newly granted MS from switching back to U-plane receive.
- Clause 21.4.3.1: `random_access_flag` acknowledges successful random access. This must remain ISSI-scoped; do not acknowledge one ISSI's access on a GSSI resource for a large group.
- Clause 23.8 / assigned-channel FACCH/STCH: listener floor-control signalling should stay on the assigned traffic channel where group members are listening.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Group-addressed `D-TX GRANTED/GrantedToOtherUser` now carries `transmitting_party_type_identifier=SSI` and `transmitting_party_address_ssi=<current speaker ISSI>`.
  - This keeps one scalable GSSI notification for all listeners while preventing the just-granted speaker from interpreting the GSSI PDU as "someone else got the floor".
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - If a speaker-qualified GSSI listener grant would exceed 124-bit STCH when serializing a redundant MAC channel allocation, UMAC keeps it on STCH and omits only that MAC channel-allocation element.
  - The primitive still carries channel allocation internally for timeslot routing; the on-air GSSI listener PDU keeps the usage marker and remains FACCH/STCH.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Preserved random-access ACKs remain exact `TetraAddress` matches.
  - ACK-only STCH may mirror `random_access_flag` for the same ISSI but does not consume it before the following channel-allocation STCH.
  - Another ISSI cannot mirror or consume that ACK, which is required for groups with many members.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_group_listener_floor_grant_with_speaker_id_stays_on_stch`.
  - Added group requester RA ACK regression for `2260082`-like floor grant.
  - Updated private RA ACK regression to prove P2P remains protected.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated D-TX GRANTED helper: ISSI/P2P stays compact; GSSI listener grants must carry speaker SSI.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_pending_random_access_ack_for_stch_waits_for_channel_allocation --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_listener_floor_grant_with_speaker_id_stays_on_stch --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_private_floor_grant_stch_carries_preserved_random_access_ack --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_grant_stch_repeats_preserved_random_access_ack_for_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_ul_inactivity_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_repeated_group_u_setup_same_gssi_during_hangtime_grants_existing_call_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_repeated_group_u_setup_from_current_speaker_reasserts_existing_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_simplex_p2p_u_tx_ceased_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this patch.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with local build only.
3. RF retest GSSI `226333`, especially repeated PTT from `2260082`.
4. Expected live evidence after fix: for `2260082` PTT, log should show individual `D-TX GRANTED`, GSSI `GrantedToOtherUser` with transmitting party `2260082`, then `NormalTrainSeq*` and `UMAC voice route` before any inactivity timeout.

## 2026-06-05 16:00:44 EEST - Deployed group floor notification hardening

Commit deployed:

- `3531b3c6` (`fix: harden group floor notification`)

Build/deploy:

- Command:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Local build only; no compile on `chris@192.168.1.179`.
- Remote binary copied directly to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup created.
- Remote deployed binary SHA-256:
  - `52918b7ca64258dac9c89c7b7f6e77f6a5a2025f24e110949ed2503a635cc036`

Live startup evidence:

- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Startup build line:
  - `Build: v0.1.55-3531b3c6`
- Startup registration/affiliation:
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.
  - `2260618` registered and affiliated to `226333`.
- Startup filtered log sample showed no `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached`.

Next non-repeating execution:

1. RF retest GSSI `226333` with repeated PTT from `2260082` (MTP3550).
2. Confirm live log contains:
   - individual `D-TX GRANTED` to `2260082`;
   - GSSI `D-TX GRANTED/GrantedToOtherUser` carrying transmitting party `2260082`;
   - `NormalTrainSeq*` and `UMAC voice route` before inactivity timeout.
3. If static persists with those logs present, next investigation should be PHY/RSSI/vocoder path for 2260082, not CMCE floor denial.

## 2026-06-05 16:42:53 EEST - UMAC large-GSSI scheduler hardening

User goal:

- Make group operation robust for thousands of affiliated terminals, not only two or three radios.
- Continue clause-scoped ETSI EN 300 392-2 hardening without claiming formal certification.

Component in simple technical terms:

- UMAC scheduler is the layer that decides which downlink MAC PDU is sent on each radio slot.
- CMCE decides who may talk; UMAC makes the floor-control/status/SDS/grant messages actually fit onto MCCH/SCH-F or assigned-channel FACCH/STCH.
- For a large GSSI, the scheduler must keep one group-addressed message and repeat it only by real Energy Economy receive batches, not create one message per affiliated ISSI.

ETSI clause scope:

- EN 300 392-2 clauses 21.4.3.1 and 23.5.2.2.2: random-access ACK and slot-grant scheduling remain addressed to the exact requesting MS.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6/T.210: downlink signalling to EG terminals is sent when the target MS or group batch is listening; T.210 is marked only for the batch that actually received the downlink.
- EN 300 392-2 clause 14.5.2.2.1: group floor listener notification remains one GSSI-addressed FACCH/STCH transfer for the group, not per-member signalling.
- Invalid local timeslot guard is internal robustness only; it does not alter valid over-air PDU semantics.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added slot-scoped `GroupReadinessCache` so repeated readiness checks for the same GSSI in one scheduling opportunity reuse the current target list and awake/asleep result.
  - Avoided allocating a temporary uncovered-listener `Vec` just to answer "is any member listening?" for existing GSSI delivery/stealing state.
  - Reused the same FACCH/STCH readiness cache when building the selected group stealing state.
  - Added defensive downlink timeslot validation on enqueue/drop/ACK helper APIs so invalid `ts=0` or `ts=5` logs and returns instead of panicking.
  - Invalid reported STCH enqueue now marks its `TxReporter` discarded rather than leaving a permanent pending request.

Tests added:

- `test_large_mixed_eg7_gssi_facch_stealing_repeats_by_receive_batch_not_member`
- `test_large_gssi_readiness_cache_is_slot_scoped_across_queued_resources`
- `test_invalid_downlink_timeslot_enqueue_and_drop_apis_do_not_panic_or_mutate`

Verification:

- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 134 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC large-GSSI scheduler hardening patch.
2. Continue with QA findings that remain open:
   - MM restart-recovery cache I/O amplification for thousands of ISSIs.
   - UMAC/TMA pending report bounds when downlink completion stalls.
   - DL media queue backpressure under sustained group-call overfeed.
3. Keep private simplex/duplex and group floor tests in the regression set before any deploy.

## 2026-06-05 16:49:28 EEST - MM restart recovery cache scaling

User goal:

- After BS restart, thousands of terminals must remain recoverable without "Unit Not Attached" drift or slow full-file cache churn.
- Keep group affiliation and scan-list recovery robust for lab GSSI `226333` and larger deployments.

Component in simple technical terms:

- MM is Mobility Management: it handles registration/attach, group affiliation state, energy economy negotiation, and BS-initiated `D-LOCATION UPDATE COMMAND` recovery.
- The restart recovery cache is a local Nexus-BS persistence file. It remembers local ISSIs and cached GSSI affiliation hints so the BS can reprobe camped terminals after process restart.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate registration with `D-LOCATION UPDATE COMMAND`.
- EN 300 392-2 clause 16.8.1/table 16.49: group identity attach/detach state is refreshed through MM group identity procedures.
- File caching is local implementation robustness only; it is not an over-air ETSI PDU change.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - MM now keeps the restart recovery cache in memory for the current configured path.
  - Startup reads the cache once and arms restart recovery from the in-memory view.
  - `remember_restart_recovery_issi*` and `forget_restart_recovery_issi` no longer read the whole file per ISSI update.
  - Multiple same-window updates are coalesced and flushed from memory instead of forcing full-file write churn for every ISSI.
  - Path changes flush the old dirty cache and load the new path cleanly.
  - Added debug-only test helpers for cache dirty state, cache size, and forced flush.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `test_restart_recovery_cache_coalesces_multiple_updates_until_flush`.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_cache_coalesces_multiple_updates_until_flush --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 134 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this MM restart recovery cache scaling patch.
2. Continue with remaining QA findings:
   - UMAC/TMA pending report bounds when downlink completion stalls.
   - DL media queue backpressure under sustained group-call overfeed.
3. Before deploy, rerun UMAC scheduler, MM, CMCE group/private, and diff checks.

## 2026-06-05 16:54:34 EEST - UMAC TMA pending report bounds

User goal:

- Keep Nexus-BS robust for long-running 24/7 operation with large groups and sustained signalling load.
- Avoid internal queues that can grow forever when RF/downlink completion stalls.

Component in simple technical terms:

- TMA is the MAC service primitive used by LLC/upper layers to submit a downlink TM-SDU and later receive a `TMA-REPORT`.
- UMAC tracks each submitted request with a `TxReporter`; when the scheduler transmits or discards the request, UMAC reports success or fragmentation failure back to LLC.

ETSI clause scope:

- EN 300 392-2 clause 20.4.1.1.1: `TMA-CANCEL` cancels a submitted `TMA-UNITDATA` request.
- EN 300 392-2 clause 20.4.1.1.3: `TMA-REPORT` reports MAC transfer completion state to LLC.
- EN 300 392-2 clause 22.3.2.3 uses MAC/TMA failure reporting for LLC retry/failure handling.
- The new cap/timeout is local resource-control hardening; it preserves the existing `FragmentationFailure` report for incomplete local MAC transfer and does not claim formal conformance.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `MAX_PENDING_TMA_REPORTS` cap for retained TMA report state.
  - Added local pending-report timeout guard for reporters that never reach transmitted/discarded.
  - Overflowed reported requests are immediately marked discarded and reported as `TmaReport::FragmentationFailure` instead of growing `pending_tma_reports`.
  - Added debug-only helpers for pending TMA report cap/count.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_tma_report_tracking_is_bounded_under_stalled_downlink_completion`.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_tma_report_tracking_is_bounded_under_stalled_downlink_completion --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 57 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this UMAC TMA pending report bounds patch.
2. Continue with DL media queue backpressure under sustained group-call overfeed.
3. Before deploy, rerun UMAC scheduler, MM, CMCE group/private, and diff checks.

## 2026-06-05 16:57:19 EEST - DL media queue backpressure

User goal:

- Keep group/private call audio paths stable under sustained overfeed and long-running operation.
- Prevent stale queued speech frames from accumulating when producer rate exceeds the radio drain rate.

Component in simple technical terms:

- `CircuitMgr` owns active UL/DL traffic circuits and the per-timeslot downlink media queues.
- These queues hold ACELP or raw TCH/S blocks that are waiting to be transmitted on an assigned traffic channel.
- If the queue grows without bound, old speech can add latency, stale audio, or memory pressure; for live PTT voice, keeping the latest bounded window is safer.

ETSI clause scope:

- EN 300 392-2 clause 23.5 traffic-channel scheduling: this is local queue/backpressure behavior before selecting the next TCH/S block for a valid assigned channel.
- Floor release/grant still purges stale media at UMAC/CMCE boundaries; this patch only bounds ordinary per-timeslot media buildup.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Added `MAX_TX_DATA_BLOCKS_PER_TIMESLOT`.
  - `put_block` and `put_raw_tch_s_half_slot` now push through a bounded helper.
  - When full, the oldest queued DL media block is dropped and the newest block is retained.
  - Added unit tests for bounded ACELP and raw TCH/S overfeed.

Verification:

- `cargo test -p tetra-entities --lib umac::subcomp::circuit_mgr --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 57 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this DL media queue backpressure patch.
2. Run a final combined verification set across scheduler, MM, UMAC, CMCE, and diff checks.
3. If deploy is requested, build locally and deploy direct to testing only; do not compile on the Pi and do not create binary backups.

## 2026-06-05 17:19:27 EEST - UMAC downlink signalling queue caps

User goal:

- Make group operation robust for thousands of terminals, not only two or three radios.
- Keep long-running Nexus-BS operation bounded when many terminals reattach, request floor, or receive group signalling at once.

Component in simple technical terms:

- UMAC is the MAC scheduler for the BS. It decides which downlink signalling PDU goes into each radio timeslot.
- The downlink signalling queues hold MAC-RESOURCE, random-access ACKs, slot grants, channel allocations, and FACCH/STCH signalling before over-air transmission.
- For large groups, these queues must have bounded memory growth while still preserving the control messages that keep PTT, attach, and call setup correct.

ETSI clause scope:

- EN 300 392-2 clause 21.4.3.1: random-access acknowledgement/grant timing is critical and must not be discarded as ordinary backlog.
- EN 300 392-2 clause 23.5.2.2.2: slot grants must keep correct timing semantics.
- EN 300 392-2 clause 23.5.2.2.7 and clause 23.7.6: downlink scheduling must account for energy-economy receive windows.
- EN 300 392-2 clause 14.5.2.1 and 14.5.2.2.1: group-call/floor signalling remains protected through the existing FACCH/STCH and grant paths.
- The queue caps are local implementation robustness; they are clause-scoped engineering hardening, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added bounded push helpers for per-timeslot downlink queues and the next-slot merge queue.
  - Preserved protected control signalling under backpressure: direct grants, pending grants, random-access ACKs, FACCH/STCH stealing, channel allocations, and MAC-RESOURCEs carrying integrated grant/RA ACK.
  - Ordinary queued MAC-RESOURCE/FragBuf backlog is discardable only through the existing reporter path, so upper layers can observe a local MAC transfer failure instead of waiting forever.
  - The next-slot merge path also enforces the cap after deferred signalling is merged back into the active timeslot queue.

Tests added:

- `test_downlink_scheduler_discards_reported_ordinary_resource_when_queue_cap_is_reached`
- `test_downlink_scheduler_backpressure_preserves_grants_over_ordinary_resources`

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 60 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC downlink signalling queue cap patch.
2. Continue hardening the remaining large-group risks:
   - global/message ingress queue backpressure for very large bursts,
   - restart recovery and group affiliation tests at multi-thousand scale,
   - active EG suspension memory shape for many simultaneous GSSI circuits.
3. Keep the regression set anchored on private simplex/duplex, group-call turn taking, SDS/status, MM attach/group affiliation, and UMAC EG7 scheduling before deploy.

## 2026-06-05 17:24:32 EEST - UMAC StayAlive large-GSSI state fast path

User goal:

- Make group operation robust for thousands of terminals, not just a two-radio lab case.
- Avoid per-member repeat state when a group has no Energy Economy subscribers.

Component in simple technical terms:

- `GroupDeliveryState` and `GroupStealingState` are UMAC bookkeeping objects.
- They remember which GSSI members have already had an EG receive opportunity for a group-addressed MAC-RESOURCE or FACCH/STCH block.
- If no group member has an Energy Economy assignment, all members are treated as continuously listening, so this repeat tracker is unnecessary.

ETSI clause scope:

- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require the BS to account for Energy Economy receive windows.
- This patch keeps the EG repeat path unchanged when any affiliated target has an Energy Economy assignment.
- For pure `StayAlive` groups, one GSSI-addressed downlink remains sufficient; skipping the local repeat tracker is an implementation memory/CPU optimization, not an over-air PDU change.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - `group_state_for_resource` now receives the current Energy Economy assignment map.
  - Large GSSI MAC-RESOURCE delivery skips `GroupDeliveryState` allocation when no target ISSI has an Energy Economy assignment.
  - GSSI FACCH/STCH stealing similarly skips `GroupStealingState` allocation for pure `StayAlive` groups.
  - Existing EG mixed/EG7 tests continue to exercise the full repeat-by-receive-batch path.

Tests added:

- `test_large_stayalive_gssi_resource_skips_group_delivery_state_snapshot`
- `test_large_stayalive_gssi_facch_transmits_once_without_group_stealing_state`

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 62 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC StayAlive large-GSSI fast path.
2. Continue with large-group restart/affiliation persistence tests at multi-thousand scale.
3. Review global `MessageQueue`/LLC queue behavior separately; do not add arbitrary drops without clause-scoped handling/reporting.

## 2026-06-05 17:26:21 EEST - UMAC deferred downlink queue cap test

User goal:

- Keep Nexus-BS robust under large group/restart signalling bursts, including traffic deferred into the next TDMA frame.

Component in simple technical terms:

- `dltx_next_slot_queue` is the scheduler holding area for downlink signalling that cannot fit in the current frame and must be tried again on the next frame.
- It is separate from the live per-timeslot queue, so it needs its own regression evidence.

ETSI clause scope:

- EN 300 392-2 clause 20.4.1.1.3: MAC reports local transfer completion/failure through TMA reporting.
- EN 300 392-2 clauses 21.4.3.1 and 23.5.2.2.2 remain protected by the production cap logic; this test covers ordinary deferred signalling only.
- This is local robustness evidence, not formal certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added a unit test proving `dltx_next_slot_queue` is capped and reports the discarded ordinary deferred request through `TxReporter`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 63 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this test-only deferred queue cap evidence patch.
2. Continue with CMCE large initial group setup fanout or MM restart recovery large-GSSI persistence tests.

## 2026-06-05 17:29:37 EEST - CMCE large group setup fanout evidence

User goal:

- Group call setup must scale to thousands of terminals without sending one call setup per ISSI.

Component in simple technical terms:

- CMCE is TETRA call control: it owns group/private call setup, release, and floor-control decisions.
- For a group call, CMCE should address setup signalling to the GSSI and open one UMAC traffic circuit for that group, not create per-terminal setup fanout.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1 defines normal group call setup using group identity scoped signalling.
- The test verifies the existing implementation remains group-scoped for a 2048-member GSSI; it is engineering regression evidence only, not formal certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_large_group_setup_uses_one_gssi_d_setup_and_one_umac_open`.
  - The test registers and affiliates 2048 ISSIs to one GSSI, starts a group call, and asserts:
    - exactly one `D-SETUP`,
    - the `D-SETUP` main address is the GSSI,
    - exactly one UMAC `Open`,
    - the traffic circuit primary address is the GSSI,
    - the initial speaker is only a secondary ISSI,
    - no release is emitted during setup.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_setup_uses_one_gssi_d_setup_and_one_umac_open --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 136 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE large group setup evidence patch.
2. Continue with MM restart recovery large-GSSI persistence and EG7 restart-derived group tests.

## 2026-06-05 17:32:38 EEST - CMCE/MM large restart-recovered GSSI evidence

User goal:

- After BS restart, thousands of terminals must remain attached to their groups and group PTT must not degrade to `PTT denied`, `No Group`, or unsolicited release.

Component in simple technical terms:

- MM restart recovery restores cached ISSI/GSSI affiliation when a terminal reappears after BS restart.
- CMCE consumes MM subscriber updates so call control knows which ISSIs are valid group listeners.
- This test proves the restored state is usable for real group floor control, not only visible in dashboard state.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate or recover registration using location update procedures.
- EN 300 392-2 clause 16.8.1: group identity attach/detach is confirmed through group identity procedures.
- EN 300 392-2 clause 14.5.2.1 and 14.5.2.2.1: group call setup and floor request handling remain GSSI scoped after recovery.
- The restart cache and large-scale test harness are local engineering evidence only, not formal certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_restart_recovery_large_cached_gssi_restores_cmce_listeners_and_turn_taking`.
  - Seeds the restart recovery cache with 2048 ISSIs affiliated to one GSSI.
  - Drives Demand Location Updating without group list for every ISSI.
  - Sends SwMI group refresh ACKs with non-matching handles to cover the field-observed unrouted ACK path.
  - Asserts all 2048 affiliates remain in the shared subscriber registry.
  - Starts a group call and verifies a restored listener receives `RequestQueued` for return PTT, with no release or UMAC close.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_large_cached_gssi_restores_cmce_listeners_and_turn_taking --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 137 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this large restart-recovered GSSI evidence patch.
2. Continue with EG7 restart-derived large-group suspension/resume tests.
3. Keep the deploy gate as local build only; do not compile on the Pi and do not create binary backups.

## 2026-06-05 17:35:43 EEST - MM large restart-recovered EG7 activation evidence

User goal:

- EG7 must work for large restored groups after BS restart, not only for one terminal.
- Terminals restored from cache must not remain in a half-attached/no-group state while Energy Economy negotiation is pending.

Component in simple technical terms:

- MM owns registration, group restore, and Energy Economy negotiation.
- In EG7, the BS requests a long sleep-cycle mode, but the assignment only becomes active after the MS sends `U-MM STATUS` response confirming it.
- This test proves that 2048 restart-restored group members can all confirm EG7 and remain affiliated to the restored GSSI.

ETSI clause scope:

- EN 300 392-2 clauses 16.4.4 and 16.8.1 cover registration/group identity recovery procedures.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6 cover Energy Economy negotiation and scheduling constraints.
- The restart cache is local implementation state; this remains engineering evidence only, not formal certification.

Patch summary:

- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members`.
  - Seeds 2048 cached ISSI -> GSSI affiliations.
  - Drives ITSI attach without explicit group identities for every ISSI.
  - Confirms SwMI group refresh handling and then sends matching EG7 responses for every restored member.
  - Asserts all restored members remain group-affiliated and each has an active EG7 assignment with no assigned-channel suspension leakage.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM EG7 restart-recovery evidence patch.
2. Run a final focused combined regression across UMAC scheduler, UMAC integration, CMCE, MM, and diff checks.
3. Next implementation target after that: inspect global `MessageQueue` and LLC queue backpressure policies without dropping protocol-critical messages blindly.

## 2026-06-05 17:46:15 EEST - LLC outbound backlog caps for large groups

User goal:

- Group and SDS/status operation must remain robust with thousands of affiliated terminals, not only two or three radios.
- Local queues must not grow without bound under group traffic, EG scheduling delay, or MAC congestion.

Component in simple technical terms:

- LLC is the link layer between TETRA service users and MAC. It stores BL-DATA while waiting for ACK/retransmission and stores BL-UDATA for `N.253 + 1` repeated transmissions.
- For large groups, LLC must keep control/data queues finite, report local admission failure explicitly, and preserve already-submitted MAC work.

ETSI clause scope:

- EN 300 392-2 clause 20.4.1.1.3: MAC/LLC completion or failure is reported upward through TMA/TLA report semantics.
- EN 300 392-2 clause 22.3.2.3: acknowledged BL-DATA owns N(S), ACK, T.251, and retransmission once admitted.
- EN 300 392-2 clause 22.3.2.4.1: BL-UDATA is stored for `N.253 + 1` complete transmissions.
- The queue caps are local Nexus-BS resource-control hardening. They do not claim formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - Added finite LLC outbound caps sized for thousands of terminals:
    - `LLC_MAX_OUTBOUND_ACKED_MESSAGES = 8192`
    - `LLC_MAX_OUTBOUND_UDATA_MESSAGES = 8192`
  - BL-DATA now rejects new requests before N(S) allocation when the acknowledged backlog is full, preserving the basic-link sequence state and returning `TLA_REPORT_FAILED_TRANSFER`.
  - BL-UDATA now enforces capacity before creating new MAC work. If an incoming higher-priority UDATA arrives at capacity, LLC may evict only a lower-priority unsubmitted UDATA entry and reports that evicted service as failed.
  - Submitted MAC work, equal-priority FIFO entries, and existing priority-7 work are not evicted by the local cap.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib llc::llc_bs_ms::tests::udata_backlog_limit --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 80 passed.
- `cargo test -p tetra-entities --lib llc::llc_bs_ms::tests::outbound_backlog_limits_are_sized_for_thousands_of_terminals --locked` -> 1 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this LLC backpressure patch.
2. Integrate any returned agent audit findings into the next implementation target.
3. Continue with global `MessageRouter`/UMAC queue pressure and CMCE group floor robustness for thousands of listeners.

Agent audit integration:

- LLC/timers agent confirmed the global outbound caps are the correct first backpressure step. Remaining LLC work is per-link admission caps, per-tick standalone BL-ACK budget, O(n) scan reduction for TMA report routing, and T.251 timer scheduling that does not scan thousands of non-due entries every tick.
- QA agent prioritized large-group round-robin PTT, restart-recovered usable CMCE, SDS/status/WAP one-GSSI delivery, mixed EG3/EG7 resource batching, and LLC pressure tests at 4096+ scale.
- MM/EG agent flagged restart-recovery pacing fairness, mass T353 rollback/reprobe, mass T352 non-response fallback, and the combined case of restart-restored EG7 members entering an assigned-channel group call.
- UMAC/MAC EG agent flagged stale GSSI repeat snapshots after floor changes, late-affiliating EG listeners during active group calls, mixed StayAlive+EG groups retaining too much per-member repeat state, and 5000-member grant/RA storms exceeding existing 4096 scheduler cap assumptions.
- SDS/status/WAP agent flagged unbounded ingress/control queues before LLC caps, live SDS queue/repeat pressure, dashboard "sent" logging before confirmed acceptance, and missing queue/failure observability.
- CMCE group/private call auditor did not return before this commit gate; keep CMCE floor/private release robustness as an active next audit/patch target rather than treating it as completed.

## 2026-06-05 17:53:08 EEST - UMAC GSSI repeat state tracks only real EG listeners

User goal:

- Group call and group signalling must scale to thousands of terminals, including mixed StayAlive and EG members.
- A single EG terminal in a large group must not force Nexus-BS to retain per-member repeat state for every always-awake terminal.

Component in simple technical terms:

- UMAC/MAC scheduling decides when a GSSI-addressed MAC-RESOURCE or FACCH/STCH block is actually transmitted.
- Energy Economy members may sleep, so GSSI signalling is repeated until sleeping EG batches have had a listening window. StayAlive members are already listening and should not remain in the repeat snapshot.

ETSI clause scope:

- EN 300 392-2 clause 23.5.2.2.7 requires downlink scheduling to account for MS reception opportunities.
- EN 300 392-2 clause 23.7.6 defines Energy Economy sleep-cycle behaviour and T.210 activity handling.
- EN 300 392-2 clause 20.4.1.1.3 remains the reporter/completion context for retained MAC requests.
- This is local resource-control and scheduling hardening, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - GSSI `GroupDeliveryState` and `GroupStealingState` now retain only targets with a valid `EnergySavingAssignment::is_energy_economy()`.
  - StayAlive or fail-open members still make the first GSSI transmission ready, but they no longer inflate the retained repeat snapshot.
  - Invalid/fail-open EG entries, including unsupported frame-18 receive recurrence, no longer trigger GSSI repeat state.
  - Pruning of retained GSSI repeat state now rechecks current valid EG targets, so stale assignment changes can complete or shrink pending repeats.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_mixed_stayalive_eg_gssi_resource_tracks_only_energy_economy_targets --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_fail_open_energy_assignment_does_not_create_gssi_repeat_snapshot --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_large_mixed_eg7_gssi --locked` -> 2 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this UMAC mixed EG/StayAlive repeat-state patch.
2. Continue with UMAC stale GSSI repeat invalidation on floor changes and late-affiliating EG listeners during active group calls.
3. Then address ingress/global `MessageQueue` and live SDS/WAP admission/observability caps.

## 2026-06-05 17:57:49 EEST - UMAC drops stale GSSI repeat snapshots on floor grant

User goal:

- Group floor changes must remain correct for large EG groups; late receive batches must not hear stale old-speaker signalling after a new PTT/floor grant.

Component in simple technical terms:

- CMCE decides who owns the group floor and sends the new `D-TX GRANTED`.
- UMAC may still have old GSSI repeat-state queued for EG listeners that were sleeping during an earlier batch.
- On a new group `FloorGranted`, UMAC now drops only already-created GSSI repeat snapshots for that group. Fresh unsent signalling for the new floor remains queued.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI floor control uses `D-TX GRANTED` to move transmission permission.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6: EG-aware downlink repeats must match the relevant receive opportunities.
- This patch is local stale-state invalidation around those clauses, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `dl_drop_queued_gssi_repeats`.
  - It removes only queued `group_state: Some` GSSI Resource/FragBuf/Stealing repeat items matching the group address.
  - It does not remove fresh `group_state: None` signalling for the same GSSI and does not remove repeat state for other groups.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - `CallControl::FloorGranted` now calls the stale-repeat dropper only for group-scoped bearers.
  - Private/P2P `FloorGranted` remains strict ISSI-participant scoped and does not invoke GSSI cleanup.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_floor_change_drops_only_requeued_gssi_repeat_state --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 66 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this stale GSSI repeat invalidation patch.
2. Continue with late-affiliating EG listeners during active assigned-channel group calls.
3. Then address global ingress/control `MessageQueue` and live SDS/WAP admission/observability caps.

## 2026-06-05 18:02:08 EEST - UMAC late EG activation joins active group suspension

User goal:

- Terminals that join/activate Energy Economy while a group call is already active must not fall asleep and miss assigned-channel group traffic.

Component in simple technical terms:

- UMAC tracks active assigned-channel suspensions so EG radios stay awake during calls.
- Previously the suspension target list was a snapshot taken when the circuit opened. A later EG activation for an ISSI newly affiliated to the same GSSI could miss that active suspension.

ETSI clause scope:

- EN 300 392-2 clause 23.7.6: Energy Economy sleep cycle is suspended while the MS has an assigned channel/call active.
- EN 300 392-2 clauses 20.3.5.4.1c and 20.4.3: TLMC configuration carries energy-economy parameters from upper layers to MAC.
- This is local suspension-state robustness, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `sync_active_suspensions_for_issi`.
  - When TLMC configures EG for an ISSI, UMAC now checks all active suspension keys and adds the ISSI if the current subscriber/group state is covered by an active GSSI/broadcast/ISSI suspension.
  - The new assignment starts with the correct `suspension_count`, and later close/resume decrements it normally.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_late_group_eg_activation_joins_active_assigned_channel_suspension`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_late_group_eg_activation_joins_active_assigned_channel_suspension --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 59 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this late EG active-suspension patch.
2. Continue with global ingress/control `MessageQueue` and live SDS/WAP admission/observability caps.
3. Continue CMCE large round-robin group PTT tests once the call-control auditor returns or after direct local inspection.

## 2026-06-05 18:14:35 EEST - CMCE call-id wrap skips live group/private calls

User goal:

- Group and private calls must remain robust at thousands of terminals and across long 24x7 runtime, not only with two or three radios.
- A fresh setup after call-id wrap must not overwrite an active or pending group/private call, because that can route PTT, release, or late-entry signalling to the wrong call.

Component in simple technical terms:

- `CircuitMgr` is the local traffic-channel manager. It chooses timeslot, usage number, and the SwMI call identifier for a new call.
- `CcBsSubentity` is the CMCE BS call-control state machine. It stores active group calls, private calls, cached `D-SETUP` PDUs, and pending `D-RELEASE`/`D-DISCONNECT` cleanup.
- The patch makes `CircuitMgr` ask CMCE which call identifiers are still occupied before allocating a new first-leg call identifier. Duplex private calls still intentionally reuse the same call-id for their second bearer leg because that is the same call.

ETSI clause scope:

- EN 300 392-2 clause 14.2.3: the CMCE call identifier is the call-handling reference allocated by the SwMI and then used by subsequent CMCE messages for that call.
- EN 300 392-2 table 14.36: call identifier is a 14-bit information element; value 0 is dummy and values 1..16383 identify calls.
- EN 300 392-2 clauses 14.5.1.1.2 and 14.5.1.2.1: individual/private call setup and initial floor state rely on the allocated call identifier.
- EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1: group setup and group floor control use the group call identifier as the maintained call reference.
- EN 300 392-2 clause 14.5.2.3: group release keeps the call identifier relevant until release cleanup completes.
- This is clause-scoped hardening and test evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/components/circuit_mgr.rs`
  - Added `get_next_call_id_avoiding` over the full 14-bit non-zero call-id range.
  - Added `CircuitErr::CallIdentifierExhausted` for the pathological case where every real call-id is occupied.
  - Added allocator variant `allocate_circuit_with_allocator_duplex_avoiding`.
  - Releases a just-reserved timeslot if call-id selection fails or circuit opening rejects, so a failed setup does not leak local timeslot state.
  - Added `active_call_ids` as a defensive backstop for circuit state that has not yet been reflected into higher CMCE maps.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `occupied_call_ids`, covering cached setups, active group calls, pending group releases, live individual calls, all pending private release/tail-drain maps, circuit-manager active ids, and echo session id.
  - Added hidden debug accessors for integration tests.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Local group setup, local private/P2P setup, Brew-originated local private setup, and echo setup now allocate first-leg call identifiers while avoiding occupied CMCE ids.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
  - Network-origin private and group setup now use the same occupied-id avoiding allocator.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added a group collision regression: keep group call A active, force allocator wrap to A's call-id, start group call B, then verify A still queues return PTT with the original call-id and no release/close side effects.
  - Added a private collision regression: keep a private setup call-id live, force allocator wrap to that id, start a group call, and verify the group call receives a different id.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib call_identifier --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_call_id_wrap_skips_live_group_call_and_preserves_ptt --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_setup_call_id_wrap_skips_live_private_call --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 139 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE call-id continuity patch.
2. Extend large-GSSI stress beyond current 2048-member tests toward 4096/5000 members for round-robin PTT, repeated `U-SETUP`, queued handoff, restart recovery, and EG7 listeners.
3. Continue SDS/WAP admission and truthful observability so live SDS/WAP cannot silently claim delivery when queues are saturated.
4. Continue MM affiliation persistence and restart recovery scale tests so terminals do not return as `No Group`/`Unit Not Attached` after BS restart.

## 2026-06-05 18:17:27 EEST - CMCE large GSSI tests raised to 4096 members

User goal:

- Group call handling must be robust for thousands of terminals, not just two or three lab radios.

Component in simple technical terms:

- CMCE group-call setup and floor control should remain group-scoped: one GSSI `D-SETUP`, one GSSI listener grant on handoff, and one bounded queued floor owner.
- MM restart-recovery must restore enough affiliation state that CMCE can still accept and queue return PTT after a BS restart.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: normal group call setup is addressed to the group identity.
- EN 300 392-2 clause 14.5.2.2.1: SwMI floor control grants, queues, or denies transmission permission without creating per-member group setup fanout.
- EN 300 392-2 clause 16.8.1: group attach/detach acknowledgement is the confirmation point used by the restart-recovery tests.
- This is scale regression evidence for the existing clause-scoped behavior, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `LARGE_GSSI_MEMBER_COUNT = 4096`.
  - Raised these CMCE regressions from 2048 to 4096 affiliated ISSIs:
    - large group setup emits one GSSI `D-SETUP` and one UMAC open.
    - large group PTT handoff emits one requester grant plus one GSSI listener grant.
    - large group floor queue remains bounded and later contenders do not replace the first queued requester.
    - restart-recovered cached GSSI restores CMCE listeners and return PTT works after attach/ACK refresh.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs large --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 139 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this 4096-member CMCE test-scale patch.
2. Continue SDS/WAP admission and truthful delivery/queue observability.
3. Continue MM affiliation persistence tests at EG7 and restart scale.
4. Add UMAC/MAC large EG batch pressure tests so 4096-member GSSI scheduling remains bounded below CMCE.

## 2026-06-05 18:24:48 EEST - Live SDS admission bounded for dashboard/WAP robustness

User goal:

- SDS/WAP features must stay robust during long-running BS operation and must not silently grow unbounded control state.
- The WAP MVP delivery path must keep working while live text-style SDS broadcast remains fail-closed for WAP PIDs that require their own raw payload encoder.

Component in simple technical terms:

- Live SDS is the dashboard/control queue for operator-injected broadcast messages, transmitted later by the Home Mode Display/SDS-TL sender.
- WAP MVP uses raw SDS Type4/WAP payload helpers, not the text-style live SDS queue.
- This patch caps only the live SDS broadcast queue. It does not limit normal SDS, raw SDS WAP delivery, or status delivery.

ETSI clause scope:

- EN 300 392-2 clause 13.2: SDS includes individual and group short data/status services.
- EN 300 392-2 clause 29.3.3.8.2: SDS-TL system broadcast may use the all-ones broadcast address.
- EN 300 392-2 clause 29.4.1 and table 29.21: SDS-TL transport PIDs are distinct from WAP/WCMP application PIDs; WAP raw Type4 remains on the raw SDS path.
- This is bounded local admission/observability hardening around SDS/WAP, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-config/src/bluestation/state.rs`
  - Added `LIVE_SDS_QUEUE_MAX_LEN = 256` for runtime live SDS broadcast entries.
- `crates/tetra-entities/src/cmce/cmce_bs.rs`
  - `AddLiveSds` now rejects new live SDS entries when the queue is full before allocating an ID or mutating state.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Dashboard live SDS POST now checks the shared queue and returns HTTP 429 when full.
  - Dashboard live SDS POST now returns HTTP 503 if the CMCE control channel is unavailable, rather than reporting OK after a failed send.
- `crates/tetra-entities/tests/test_sds_bs.rs`
  - Added `test_live_sds_control_queue_is_bounded`, proving overflow is rejected without evicting accepted broadcasts, consuming an ID, or emitting an RF message.

Verification:

- `cargo fmt --package tetra-config --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_sds_bs live_sds --locked` -> 5 passed.
- `cargo test -p tetra-entities --test test_sds_bs wap --locked` -> 12 passed.
- `cargo test -p tetra-entities net_dashboard::server::tests::live_sds --locked` -> 6 relevant dashboard tests passed.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 117 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this bounded live SDS admission patch.
2. Continue MM restart/affiliation persistence with EG7-scale tests.
3. Continue UMAC/MAC EG batch pressure below CMCE for 4096-member GSSI scheduling.
4. Add dashboard/API observability for accepted-vs-transmitted WAP/SDS where a synchronous response path exists.

## 2026-06-05 18:28:08 EEST - MM restart recovery EG7 scale raised to 4096 members

User goal:

- After BS restart, terminals must not come back as `Unit Not Attached` or `No Group`; cached affiliations and EG mode must converge robustly for thousands of terminals.

Component in simple technical terms:

- MM owns registration, group affiliation, restart recovery, and energy-economy negotiation.
- The restart recovery cache seeds known ISSIs/GSSIs after process restart; MM then refreshes the group on air and waits for explicit terminal ACK/EG response.
- EG7 is the longest configured energy economy mode in this test, so it is the harshest case for restart recovery plus sleeping terminals.

ETSI clause scope:

- EN 300 392-2 clauses 16.4 and 16.8: registration and group attach/detach procedures restore the MS and its group identities.
- EN 300 392-2 clauses 16.7.1, 16.10.9 and 16.10.10: energy-economy mode is negotiated and activated after the matching MS response.
- EN 300 392-2 clause 23.7.6 and table 23.9: EG7 has the longest sleep-cycle behavior, so it must not be activated speculatively before explicit response.
- This is scale regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `LARGE_RESTART_RECOVERY_MEMBER_COUNT = 4096`.
  - Raised `test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members` from 2048 to 4096 members.
  - The test now confirms 4096 cached members can ITSI attach, receive cached group refresh, ACK it, explicitly respond to EG7, remain affiliated to the GSSI, and receive an EG7 assignment.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM 4096-member restart recovery/EG7 test-scale patch.
2. Continue UMAC/MAC EG batch pressure below CMCE for 4096-member GSSI scheduling.
3. Continue dashboard/API observability for accepted-vs-transmitted WAP/SDS where a synchronous response path exists.

## 2026-06-05 18:39:33 EEST - UMAC STCH floor grants prioritized under 4096-entry group pressure

User goal:

- Group calls must remain robust for thousands of terminals, not just two or three radios.
- A queued requester that becomes the next speaker must receive the positive floor grant promptly even if the group has generated thousands of lower-value busy/queued responses.

Component in simple technical terms:

- UMAC/MAC scheduler is the layer that chooses which downlink control block is placed into the next radio timeslot.
- STCH/FACCH stealing is the assigned-channel control path used during voice traffic for urgent call-control messages such as `D-TX GRANTED`, `D-TX CEASED`, and `D-TX INTERRUPT`.
- This patch does not change the CMCE floor-control decision; it changes only which already-built STCH control block is transmitted first when the assigned-channel queue is under heavy group pressure.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI group-call floor control uses `D-TX GRANTED`, `D-TX CEASED`, and related responses to grant, queue, reject, or withdraw transmission permission.
- EN 300 392-2 clause 23.5: STCH/FACCH is the assigned-channel signalling path during traffic.
- EN 300 392-2 clause 23.5.2.2.7 remains relevant because assigned uplink opportunities must be reserved/advertised coherently with downlink control.
- This is clause-scoped scheduler hardening and regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added STCH scheduling priority derived from the actual MAC-RESOURCE/CMCE bitstream already queued for transmission.
  - Keeps `D-TX INTERRUPT` and `D-TX CEASED` ahead of lower-value floor responses so withdrawal/preemption ordering is not inverted.
  - Prioritizes positive `D-TX GRANTED` with an uplink channel allocation (`UL`/`Both`) ahead of DL-only `RequestQueued`/`NotGranted` backlog.
  - Preserves FIFO ordering within the same priority class.
  - Added `test_large_group_positive_floor_grant_stch_preempts_busy_response_backlog`, which queues 4096 DL-only `RequestQueued` STCH responses before a positive requester grant and proves the positive UL+DL grant is transmitted first.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_large_group_positive_floor_grant_stch_preempts_busy_response_backlog --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 67 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 59 passed.
- `cargo test -p tetra-entities --test test_cmce_bs large --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 139 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC STCH large-group floor-grant priority patch.
2. Add a scheduler/integration regression that proves `D-TX INTERRUPT` remains before preemptive `D-TX GRANTED` on the air path, not just in CMCE message order.
3. Continue pending group/private release call-id wrap and pending-release flood tests.
4. Continue SDS/WAP accepted-vs-transmitted observability and long-run bounded queues.

## 2026-06-05 18:45:46 EEST - UMAC STCH backpressure bounded for low-value group floor responses

User goal:

- The BS must stay robust with thousands of terminals in one group and must not grow unbounded STCH control queues during PTT storms.
- Preemptive floor-control ordering must stay correct: withdraw/interruption before the new grant.

Component in simple technical terms:

- Backpressure is the scheduler's safety valve when too many downlink control messages are waiting for one traffic timeslot.
- `D-TX GRANTED/RequestQueued/NotGranted` DL-only responses are useful feedback, but under a storm they are lower value than the one positive grant that lets the next speaker enter U-plane.
- `D-TX INTERRUPT` and `D-TX CEASED` are floor-withdrawal messages; those remain protected because they stop or move the current speaker.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: floor-control responses grant, queue, reject, or withdraw permission to transmit.
- EN 300 392-2 clause 14.5.2.2.1 f): transmission interruption withdraws the current permission before the new speaker is advertised.
- EN 300 392-2 clause 23.5: these messages are carried on assigned-channel STCH/FACCH during traffic.
- This is queue robustness and ordering regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Backpressure no longer protects every `DlSchedElem::Stealing` item blindly.
  - Protects floor-withdrawal STCH (`D-TX INTERRUPT`, `D-TX CEASED`) and positive `D-TX GRANTED` with `UL`/`Both` channel allocation.
  - Allows lower-value DL-only `D-TX GRANTED` outcomes (`RequestQueued`, `NotGranted`, listener-only `GrantedToOtherUser`) to be shed when the STCH queue is full.
  - Added `test_preemptive_floor_interrupt_stch_stays_ahead_of_positive_grant`, proving the air-path scheduler sends interrupt before a positive grant even when the grant was queued first.
  - Tightened the 4096-entry large-group test to assert the queue stays bounded at `MAX_DLSCHED_ELEMS_PER_TIMESLOT` while preserving/transmitting the positive grant.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_large_group_positive_floor_grant_stch_preempts_busy_response_backlog --locked` -> 1 passed after the bounded-backpressure assertion.
- `cargo test -p tetra-entities --lib test_preemptive_floor_interrupt_stch_stays_ahead_of_positive_grant --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 68 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 59 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this bounded STCH backpressure/preemption regression patch.
2. Continue pending group/private release call-id wrap tests.
3. Continue large pending-release PTT flood tests.
4. Continue restart-recovered EG7 affiliation through UMAC scheduling.
