# Nexus-BS Project Timeline

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

Next non-repeating execution:

1. Commit and deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Retest the exact field case: `2260616 -> 2260618`, let `2260618` speak last and release PTT, then hang up from `2260616`.
3. Expected live evidence: prompt `D-RELEASE` to `2260616`, no `D-DISCONNECT` to `2260618`, tail-drained `D-RELEASE` to `2260618`, no `U-RELEASE` required from `2260618`, no MXP600 reboot.

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
