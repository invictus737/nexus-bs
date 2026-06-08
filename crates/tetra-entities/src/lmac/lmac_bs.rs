use tetra_config::bluestation::{SharedConfig, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, BurstType, PhyBlockNum, PhysicalChannel, Sap, SsiType, TdmaTime, TrainingSequence};
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
use tetra_saps::tmv::{TmvUnitdataInd, TmvUnitdataReq};
use tetra_saps::tp::{TpUnitdataInd, TpUnitdataReqSlot};
use tetra_saps::{SapMsg, SapMsgInner};

use crate::lmac::components::{errorcontrol, scrambler};
use crate::{MessagePrio, MessageQueue, TetraEntityTrait};

#[derive(Debug, Clone, Copy)]
pub struct LmacTrafficChan {
    pub is_active: bool,
    pub logical_channel: LogicalChannel,
    // TODO FIXME: extend with all required fields
}

impl Default for LmacTrafficChan {
    fn default() -> Self {
        Self {
            is_active: false,
            logical_channel: LogicalChannel::TchS,
        }
    }
}

// #[derive(Default)]
// pub struct CurBurst {
//     pub is_traffic: bool,
//     pub usage: Option<u8>,
//     pub blk1_stolen: bool,
//     pub blk2_stolen: bool,
// }

const POST_GRANT_RX_DIAG_TIMESLOTS: i32 = 18 * 4;
const POST_GRANT_RX_DIAG_MAX_EVENTS: u8 = 24;

#[derive(Debug, Clone, Copy)]
struct PostGrantRxDiag {
    grant_time: TdmaTime,
    remaining_events: u8,
}

pub struct LmacBs {
    /// Timeslot time, provided by upper layer and then maintained in sync here
    dltime: TdmaTime,
    config: SharedConfig,

    /// Cached from global config
    stack_mode: StackMode,
    scrambling_code: u32,

    /// Traffic channels and associated state
    // ul_circuits: [Option<LmacTrafficChan>; 4],
    // dl_circuits: [Option<LmacTrafficChan>; 4],

    /// Per-timeslot UL physical channel indicator from UMAC.
    /// UL bursts arrive 2 timeslots after the corresponding DL slot, so we must
    /// keep this keyed by timeslot rather than a single "latest" value.
    uplink_phy_chan: [PhysicalChannel; 4],

    /// Signalled by UMAC per UL burst. A MAC-DATA/MAC-U-SIGNAL first half may
    /// mark the second half as stolen; keep the exact UL time so a stale
    /// indication cannot be applied to a later TCH/S speech half-slot.
    blk2_stolen_at: [Option<TdmaTime>; 4],

    /// Short RF diagnostic window after an uplink-capable STCH grant.
    post_grant_rx_diag: [Option<PostGrantRxDiag>; 4],
    // Details about current burst, parsed from BBK broadcast block
    // cur_burst: CurBurst,
}

impl LmacBs {
    pub fn new(config: SharedConfig) -> Self {
        // Retrieve initial basic network params from config
        let (stack_mode, sc) = {
            let c = config.config();
            tracing::info!(
                "LmacBs: initialized with stack mode {:?}, mcc {} mnc {} cc {}",
                c.stack_mode,
                c.net.mcc,
                c.net.mnc,
                c.cell.colour_code
            );
            (
                c.stack_mode,
                scrambler::tetra_scramb_get_init(c.net.mcc, c.net.mnc, c.cell.colour_code),
            )
        };

        Self {
            config,
            stack_mode,
            scrambling_code: sc,

            dltime: TdmaTime::default(),
            uplink_phy_chan: [PhysicalChannel::Unallocated; 4],
            blk2_stolen_at: [None; 4],
            post_grant_rx_diag: [None; 4],
        }
    }

    // fn determine_phy_chan_ul(&self) -> PhysicalChannel {
    //     let ultime = self.dltime.add_timeslots(-2);
    //     // Frame 18 is always CP (I think)
    //     if ultime.f == 18 {
    //         return PhysicalChannel::Control;
    //     }
    //     if self.ul_circuits[ultime.t as usize - 1].is_some() {
    //         return PhysicalChannel::Traffic;
    //     }
    //     PhysicalChannel::Unallocated
    // }

    // fn determine_phy_chan_dl(&self) -> PhysicalChannel {

    //     // Frame 18 is always CP (I think)
    //     if self.dltime.f == 18 {
    //         return PhysicalChannel::Control;
    //     }
    //     // Slot 1 is primary control channel
    //     if self.dltime.t == 1 {
    //         return PhysicalChannel::Control;
    //     }
    //     // Slots 2-4 may contain traffic or are unallocated
    //     if self.dl_circuits[self.dltime.t as usize - 1].is_some() {
    //         return PhysicalChannel::Traffic;
    //     } else {
    //         PhysicalChannel::Unallocated
    //     }
    // }

    /// Yields logical channel for given block. Based on Clause 9.5.1
    fn determine_logical_channel_ul(blk: &TpUnitdataInd, burst_is_traffic: bool, block2_stolen: bool) -> LogicalChannel {
        match blk.burst_type {
            BurstType::CUB => {
                // CUB is always SCH/HU
                if blk.train_type != TrainingSequence::ExtendedTrainSeq {
                    tracing::warn!("LMAC: CUB without ExtendedTrainSeq (got {:?}), treating as SchHu", blk.train_type);
                }
                LogicalChannel::SchHu
            }
            BurstType::NUB => {
                match blk.train_type {
                    TrainingSequence::NormalTrainSeq1 => {
                        // TCH or SCH/F
                        if blk.block_num != PhyBlockNum::Both {
                            tracing::warn!("LMAC: NUB/NormalTrainSeq1 unexpected block_num {:?} (expected Both)", blk.block_num);
                        }
                        if burst_is_traffic {
                            // Only support TCH/S speech channel for now
                            LogicalChannel::TchS
                        } else {
                            // Full slot signalling
                            LogicalChannel::SchF
                        }
                    }
                    TrainingSequence::NormalTrainSeq2 => {
                        // Clause 9.4.4.3.2:
                        // STCH+TCH
                        // STCH+STCH (if blk1 has resource stating 2nd block stolen)
                        if !burst_is_traffic {
                            tracing::debug!("NUB with NormalTrainSeq2 but non-traffic burst");
                            // tracing::warn!("NUB with NormalTrainSeq2 but non-traffic burst, unexpected");
                        }

                        if blk.block_num == PhyBlockNum::Block1 {
                            LogicalChannel::Stch
                        } else if blk.block_num == PhyBlockNum::Block2 {
                            if block2_stolen {
                                tracing::debug!("NUB blk2 in STCH?");
                                LogicalChannel::Stch
                            } else {
                                // EN 300 392-2 clause 23.8.4.1.4: for NTS2
                                // on an assigned traffic uplink, Block2 is
                                // TCH unless the first-half MAC header says
                                // the second half is stolen. A stale local CP
                                // marker after a floor grant must not consume
                                // valid speech as signalling.
                                LogicalChannel::TchS
                            }
                        } else {
                            tracing::warn!(
                                "LMAC: NUB/NormalTrainSeq2 unexpected block_num {:?}, treating as Stch",
                                blk.block_num
                            );
                            LogicalChannel::Stch
                        }
                    }
                    other => {
                        // Demodulator can classify a NUB with an unexpected training
                        // sequence (Seq3/Sync/NotFound) on a noisy or colliding signal.
                        // Treat as SchHu and let higher-layer CRC reject it, rather than
                        // unreachable!()-panicking on wire-derived data.
                        tracing::warn!("LMAC: NUB with unexpected train_type {:?}, treating as SchHu", other);
                        LogicalChannel::SchHu
                    }
                }
            }
            other => {
                // Any burst type other than CUB/NUB reaching UL classification is
                // unexpected (SDB is downlink). Drop-safe: treat as SchHu so CRC rejects.
                tracing::warn!("LMAC: unexpected UL burst_type {:?}, treating as SchHu", other);
                LogicalChannel::SchHu
            }
        }
    }

    fn ul_ts_index(ts: u8, context: &str) -> Option<usize> {
        if (1..=4).contains(&ts) {
            Some(ts as usize - 1)
        } else {
            tracing::warn!("LMAC: {context}: invalid UL timeslot {ts}");
            None
        }
    }

    fn diag_age(diag: PostGrantRxDiag, ul_time: TdmaTime) -> i32 {
        diag.grant_time.age(ul_time)
    }

    fn maybe_arm_post_grant_rx_diag_from_dl_stch(&mut self, tx_time: TdmaTime, blk1: &TmvUnitdataReq) {
        if blk1.logical_channel != LogicalChannel::Stch {
            return;
        }

        let mut mac_probe = BitBuffer::from_bitbuffer(&blk1.mac_block);
        let Ok(resource) = MacResource::from_bitbuf(&mut mac_probe) else {
            return;
        };
        let Some(addr) = resource.addr else {
            return;
        };
        if addr.ssi_type != SsiType::Issi {
            return;
        }
        let uplink_allocated = resource
            .chan_alloc_element
            .as_ref()
            .is_some_and(|chan_alloc| matches!(chan_alloc.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both));
        if !uplink_allocated {
            return;
        }

        let Some(ts_idx) = Self::ul_ts_index(tx_time.t, "maybe_arm_post_grant_rx_diag_from_dl_stch") else {
            return;
        };
        self.post_grant_rx_diag[ts_idx] = Some(PostGrantRxDiag {
            grant_time: tx_time,
            remaining_events: POST_GRANT_RX_DIAG_MAX_EVENTS,
        });
        tracing::info!(
            "LMAC RF diag: armed post-grant UL window tx_time={} addr={} ra_ack={} usage={:?} chan_alloc={:?}",
            tx_time,
            addr,
            resource.random_access_flag,
            resource.usage_marker,
            resource
                .chan_alloc_element
                .as_ref()
                .map(|ca| (ca.ts_assigned, ca.ul_dl_assigned, ca.mon_pattern, ca.frame18_mon_pattern))
        );
    }

    fn take_post_grant_rx_diag_event(&mut self, ul_time: TdmaTime) -> Option<PostGrantRxDiag> {
        let ts_idx = Self::ul_ts_index(ul_time.t, "take_post_grant_rx_diag_event")?;
        let mut diag = self.post_grant_rx_diag[ts_idx]?;
        let age = Self::diag_age(diag, ul_time);
        if !(0..=POST_GRANT_RX_DIAG_TIMESLOTS).contains(&age) {
            if age > POST_GRANT_RX_DIAG_TIMESLOTS {
                self.post_grant_rx_diag[ts_idx] = None;
            }
            return None;
        }
        if diag.remaining_events == 0 {
            self.post_grant_rx_diag[ts_idx] = None;
            return None;
        }

        diag.remaining_events = diag.remaining_events.saturating_sub(1);
        if diag.remaining_events == 0 {
            self.post_grant_rx_diag[ts_idx] = None;
        } else {
            self.post_grant_rx_diag[ts_idx] = Some(diag);
        }
        Some(diag)
    }

    fn log_post_grant_result(diag: Option<PostGrantRxDiag>, ul_time: TdmaTime, result: &str) {
        if let Some(diag) = diag {
            tracing::info!(
                "LMAC RF diag: post-grant UL result grant_time={} age={} ul_time={} result={}",
                diag.grant_time,
                Self::diag_age(diag, ul_time),
                ul_time,
                result
            );
        }
    }

    fn rx_blk_traffic(
        &mut self,
        queue: &mut MessageQueue,
        blk: TpUnitdataInd,
        lchan: LogicalChannel,
        ul_time: TdmaTime,
        diag: Option<PostGrantRxDiag>,
    ) {
        if lchan != LogicalChannel::TchS {
            tracing::trace!(
                "rx_blk_traffic: ignoring unsupported traffic lchan={:?} blk_num={:?}",
                lchan,
                blk.block_num
            );
            Self::log_post_grant_result(diag, ul_time, "drop_unsupported_traffic_lchan");
            return;
        }
        if blk.block_num == PhyBlockNum::Block2 {
            let data = blk.block.into_bitvec();
            if data.len() != 216 {
                tracing::warn!("rx_blk_traffic: dropping raw TCH/S Block2 with {} bits; expected 216", data.len());
                Self::log_post_grant_result(diag, ul_time, "drop_len");
                return;
            }
            // EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5 require the BS to
            // interpret a non-stolen second half-slot as TCH and preserve its
            // timing/position. This is not a complete ACELP frame, so keep it
            // tagged as raw type-5 TCH/S instead of decoding it as clean speech.
            tracing::debug!(
                "rx_blk_traffic: forwarding raw TCH/S Block2 on UL ts={} bits={}",
                ul_time.t,
                data.len()
            );
            let msg = SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Lmac,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                    ts: ul_time.t,
                    data,
                    raw_tch_s_block: Some(PhyBlockNum::Block2),
                }),
            };
            queue.push_back(msg);
            Self::log_post_grant_result(diag, ul_time, "forward_raw_block2");
            return;
        }
        if blk.block_num != PhyBlockNum::Both {
            // EN 300 392-2 clauses 23.8.3 and 23.8.3.2 permit bad or
            // partially unavailable speech only when the bad-frame/half-slot
            // condition is preserved. The current TMD SAP cannot carry that
            // condition, so do not turn a 216-bit stolen/partial block into
            // clean ACELP speech.
            tracing::debug!(
                "rx_blk_traffic: dropping partial TCH/S lchan={:?} blk_num={:?}; TMD SAP has no BFI/half-slot condition",
                lchan,
                blk.block_num
            );
            Self::log_post_grant_result(diag, ul_time, "drop_partial");
            return;
        }

        let (decoded, crc_ok) = errorcontrol::decode_tp(lchan, blk.block, self.scrambling_code);
        let Some(acelp_bits) = decoded else {
            tracing::warn!("rx_blk_traffic: decode_tp returned None");
            Self::log_post_grant_result(diag, ul_time, "decode_none");
            return;
        };

        if !crc_ok {
            // EN 300 392-2 clauses 23.8.3 and 23.8.3.2 allow undecodable
            // TCH to be delivered only with the half-slot condition marked
            // bad. The current TMD SAP has no bad-frame/half-slot-condition
            // field, so fail closed instead of forwarding corrupt bits as
            // clean speech.
            tracing::debug!("rx_blk_traffic: CRC fail (BFI), dropping TCH/S frame on UL ts={}", ul_time.t);
            Self::log_post_grant_result(diag, ul_time, "drop_crc");
            return;
        }

        // Convert ACELP BitBuffer to Vec<u8> (one bit per byte, 274 bytes)
        let mut data = vec![0u8; acelp_bits.get_len()];
        let mut bb = acelp_bits;
        bb.seek(0);
        bb.to_bitarr(&mut data);
        tracing::debug!(
            "rx_blk_traffic: decoded valid TCH/S frame on UL ts={} bits={}",
            ul_time.t,
            data.len()
        );

        let msg = SapMsg {
            sap: Sap::TmdSap,
            src: TetraEntity::Lmac,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                ts: ul_time.t,
                data,
                raw_tch_s_block: None,
            }),
        };
        queue.push_back(msg);
        Self::log_post_grant_result(diag, ul_time, "forward_acelp");
    }

    fn rx_blk_control(
        &mut self,
        queue: &mut MessageQueue,
        blk: TpUnitdataInd,
        lchan: LogicalChannel,
        ul_time: TdmaTime,
        diag: Option<PostGrantRxDiag>,
    ) -> bool {
        // AACH is a control channel but uses a completely different decode path
        // (decode_aach); decode_cp() below explicitly rejects it. Guard here so a future
        // routing change that sends AACH this way logs and drops instead of panicking.
        if !lchan.is_control_channel() || lchan == LogicalChannel::Aach {
            tracing::warn!("LMAC: rx_blk_control called with unsupported channel {:?}, ignoring", lchan);
            Self::log_post_grant_result(diag, ul_time, "drop_unsupported_control_lchan");
            return false;
        }

        let block_num = blk.block_num;
        let rssi_dbfs = blk.rssi_dbfs;
        let (type1bits, crc_pass) = errorcontrol::decode_cp(lchan, blk, Some(self.scrambling_code));
        // decode_cp only returns None when no scrambling code is available; we always pass
        // Some() here, so this is guaranteed. Use let-else instead of unwrap to stay
        // panic-free if that contract ever changes.
        let Some(type1bits) = type1bits else {
            tracing::warn!(
                "LMAC: decode_cp returned None for {:?} despite scrambling code set, dropping",
                lchan
            );
            Self::log_post_grant_result(diag, ul_time, "decode_none_control");
            return false;
        };

        // tracing::debug!("rx_blk_cp {:?} CRC: {} type1 {:?}",
        //     lchan,
        //     if crc_pass { "ok" } else { "WRONG" },
        //     type1bits
        // );
        tracing::debug!("rx_blk_cp {:?} CRC: {}", lchan, if crc_pass { "ok" } else { "WRONG" });

        // TODO FIXME, for now, we're not passing broken CRC msgs up to Lmac
        // If we see purpose, we may pass it up in the future
        if !crc_pass {
            Self::log_post_grant_result(diag, ul_time, "control_crc_fail");
            return false;
        }

        // Pass block to the upper mac
        let m = SapMsg {
            sap: Sap::TmvSap,
            src: TetraEntity::Lmac,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
                pdu: type1bits,
                logical_channel: lchan,
                block_num,
                crc_pass,
                scrambling_code: self.scrambling_code,
                rssi_dbfs,
            }),
        };

        // Suppose we've just parsed blk1 in a stolen traffic burst.
        // We then don't know whether blk2 is also stolen, as that will be shown by the Umac
        // We thus push this with prio, and the umac will signal with prio if blk2 is stolen too
        queue.push_prio(m, MessagePrio::Immediate);
        Self::log_post_grant_result(diag, ul_time, "forward_control");
        true
    }

    fn should_fallback_non_traffic_nub_to_tch_s(blk: &TpUnitdataInd, pchan: PhysicalChannel) -> bool {
        matches!(pchan, PhysicalChannel::Unallocated | PhysicalChannel::Cp)
            && blk.burst_type == BurstType::NUB
            && matches!(
                (blk.train_type, blk.block_num),
                (TrainingSequence::NormalTrainSeq1, PhyBlockNum::Both) | (TrainingSequence::NormalTrainSeq2, PhyBlockNum::Block2)
            )
    }

    fn rx_tp_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_tp_prim: msg {:?}", message);

        let SapMsgInner::TpUnitdataInd(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        let Some(ts_idx) = Self::ul_ts_index(msg_dltime.t, "rx_tp_prim") else {
            return;
        };
        let pchan = self.uplink_phy_chan[ts_idx];
        let block_num = prim.block_num;
        let mut block2_stolen = self.blk2_stolen_at[ts_idx] == Some(msg_dltime);
        let diag = self.take_post_grant_rx_diag_event(msg_dltime);
        if self.blk2_stolen_at[ts_idx].is_some() && !block2_stolen {
            tracing::warn!(
                "lmac_bs: dropping stale blk2_stolen marker for UL ts {} at {:?}; current uplink time is {:?}",
                msg_dltime.t,
                self.blk2_stolen_at[ts_idx],
                msg_dltime
            );
            self.blk2_stolen_at[ts_idx] = None;
        }

        // Sanity checks
        if block_num == PhyBlockNum::Block1 && block2_stolen {
            tracing::warn!("lmac_bs: blk2_stolen set when receiving block1, resetting");
            self.blk2_stolen_at[ts_idx] = None;
            block2_stolen = false;
        }
        if pchan != PhysicalChannel::Tp && block2_stolen {
            tracing::warn!(
                "lmac_bs: blk2_stolen set on non-traffic burst (pchan={:?}), resetting — likely late STCH after circuit close",
                pchan
            );
            self.blk2_stolen_at[ts_idx] = None;
            return;
        }

        let lchan = Self::determine_logical_channel_ul(&prim, pchan == PhysicalChannel::Tp, block2_stolen);
        if let Some(diag) = diag {
            tracing::info!(
                "LMAC RF diag: post-grant UL candidate grant_time={} age={} ul_time={} pchan={:?} burst={:?} train={:?} block={:?} block2_stolen={} lchan={:?} rssi_dbfs={:.1}",
                diag.grant_time,
                Self::diag_age(diag, msg_dltime),
                msg_dltime,
                pchan,
                prim.burst_type,
                prim.train_type,
                prim.block_num,
                block2_stolen,
                lchan,
                prim.rssi_dbfs
            );
        }

        match lchan {
            LogicalChannel::Clch => {}
            LogicalChannel::TchS | LogicalChannel::Tch24 | LogicalChannel::Tch48 | LogicalChannel::Tch72 => {
                self.rx_blk_traffic(queue, prim, lchan, msg_dltime, diag)
            }
            LogicalChannel::SchF | LogicalChannel::SchHu | LogicalChannel::Stch => {
                let fallback_candidate = prim.clone();
                let control_forwarded = self.rx_blk_control(queue, prim, lchan, msg_dltime, diag);
                if !control_forwarded && Self::should_fallback_non_traffic_nub_to_tch_s(&fallback_candidate, pchan) {
                    // EN 300 392-2 clauses 23.5.2.2.1 and 23.8.5 require the
                    // BS to accept TCH/S on an assigned traffic channel. During
                    // private-call setup and repeated simplex floor re-entry,
                    // the UL channel marker can lag the downlink allocation or
                    // hangtime exit by two timeslots; try TCH/S only after the
                    // candidate failed as control. UMAC still drops the result
                    // unless a matching non-hangtime circuit/floor is active.
                    tracing::debug!(
                        "LMAC: retrying undecoded NUB as candidate TCH/S on non-traffic UL marker {:?} ts={} train={:?} block={:?}",
                        pchan,
                        msg_dltime.t,
                        fallback_candidate.train_type,
                        fallback_candidate.block_num
                    );
                    self.rx_blk_traffic(queue, fallback_candidate, LogicalChannel::TchS, msg_dltime, diag);
                }
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }

        if block_num == PhyBlockNum::Block2 && block2_stolen {
            self.blk2_stolen_at[ts_idx] = None;
        }
    }

    fn rx_tmv_configure_req(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        let SapMsgInner::TmvConfigureReq(prim) = &message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        if let Some(stolen) = prim.blk2_stolen {
            let ul_time = prim.time.unwrap_or_else(|| self.dltime.add_timeslots(-2));
            let Some(ts_idx) = Self::ul_ts_index(ul_time.t, "rx_tmv_configure_req") else {
                return;
            };
            self.blk2_stolen_at[ts_idx] = stolen.then_some(ul_time);
        }
    }

    /// Request from Umac to transmit a message
    fn rx_tmv_unitdata_req_slot(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::debug!("rx_tmv_unitdata_req_slot");
        let SapMsgInner::TmvUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Update per-timeslot UL physical channel indicator
        let ts_idx = prim.ts.t as usize - 1;
        self.uplink_phy_chan[ts_idx] = prim.ul_phy_chan;

        let Some(bbk) = prim.bbk.take() else {
            tracing::error!("LMAC: rx_tmv_unitdata_req_slot: bbk missing, dropping slot");
            return;
        };
        let Some(blk1) = prim.blk1.take() else {
            tracing::error!("LMAC: rx_tmv_unitdata_req_slot: blk1 missing, dropping slot");
            return;
        };
        let blk2 = prim.blk2.take();
        self.maybe_arm_post_grant_rx_diag_from_dl_stch(prim.ts, &blk1);

        // Determine train and burst type
        let (burst_type, train_type) = match blk1.logical_channel {
            LogicalChannel::Bsch => {
                // Synchronization Downlink Burst
                if blk2.is_none() {
                    tracing::warn!("LMAC: Bsch slot missing blk2, dropping");
                    return;
                }
                (BurstType::SDB, TrainingSequence::SyncTrainSeq)
            }

            LogicalChannel::SchF => {
                // Single full block
                if blk2.is_some() {
                    tracing::warn!("LMAC: SchF slot has unexpected blk2, ignoring blk2");
                }
                (BurstType::NDB, TrainingSequence::NormalTrainSeq1)
            }
            LogicalChannel::TchS => {
                // Traffic burst
                if blk2.is_some() {
                    tracing::warn!("LMAC: TCH slot has unexpected blk2, ignoring blk2");
                }
                (BurstType::NDB, TrainingSequence::NormalTrainSeq1)
            }
            LogicalChannel::Tch24 | LogicalChannel::Tch48 | LogicalChannel::Tch72 => {
                // EN 300 392-2 clauses 8.3.1.3.2 to 8.3.1.3.4 define circuit-mode
                // data TCHs with their own type-1 sizes, coding and interleaving. This
                // stack currently implements only the TCH/S speech encoder path, so do
                // not silently encode data traffic as speech.
                tracing::warn!(
                    "LMAC: circuit-mode data channel {:?} is not implemented, dropping slot",
                    blk1.logical_channel
                );
                return;
            }
            LogicalChannel::SchHd | LogicalChannel::Stch | LogicalChannel::Bnch => {
                // Two half-blocks
                if blk2.is_none() {
                    tracing::warn!("LMAC: {:?} slot missing blk2, dropping", blk1.logical_channel);
                    return;
                }
                (BurstType::NDB, TrainingSequence::NormalTrainSeq2)
            }
            _ => {
                tracing::warn!(
                    "LMAC: unsupported logical channel {:?} in rx_tmv_unitdata_req_slot, dropping",
                    blk1.logical_channel
                );
                return;
            }
        };

        let mut prim_phy = TpUnitdataReqSlot {
            train_type,
            burst_type,
            bbk: None,
            blk1: None,
            blk2: None,
        };

        // Encode blk1 and optionally blk2
        prim_phy.bbk = Some(errorcontrol::encode_aach(bbk.mac_block, bbk.scrambling_code));
        prim_phy.blk1 = if blk1.logical_channel.is_traffic() {
            let logical_channel = blk1.logical_channel;
            let Some(encoded) = errorcontrol::encode_tp(blk1, 1) else {
                tracing::warn!("LMAC: failed encoding {:?} blk1, dropping slot", logical_channel);
                return;
            };
            Some(encoded)
        } else {
            let logical_channel = blk1.logical_channel;
            let Some(encoded) = errorcontrol::encode_cp(blk1) else {
                tracing::warn!("LMAC: failed encoding {:?} blk1, dropping slot", logical_channel);
                return;
            };
            Some(encoded)
        };
        if let Some(blk2) = blk2 {
            if blk2.logical_channel.is_traffic() {
                if blk2.logical_channel == LogicalChannel::TchS && blk2.mac_block.get_len() == 216 {
                    // The upper MAC can ask us to preserve a received raw TCH/S
                    // second half-slot. It is already type-5 encoded, so passing
                    // it directly preserves the half-slot pairing required by
                    // EN 300 392-2 clause 23.8.5.
                    prim_phy.blk2 = Some(blk2.mac_block);
                } else {
                    let logical_channel = blk2.logical_channel;
                    let Some(encoded) = errorcontrol::encode_tp(blk2, 2) else {
                        tracing::warn!("LMAC: failed encoding {:?} blk2, dropping slot", logical_channel);
                        return;
                    };
                    prim_phy.blk2 = Some(encoded);
                }
            } else {
                let logical_channel = blk2.logical_channel;
                let Some(encoded) = errorcontrol::encode_cp(blk2) else {
                    tracing::warn!("LMAC: failed encoding {:?} blk2, dropping slot", logical_channel);
                    return;
                };
                prim_phy.blk2 = Some(encoded);
            }
        }

        // Pass timeslot worth of blocks to Phy
        let m = SapMsg {
            sap: Sap::TpSap,
            src: TetraEntity::Lmac,
            dest: TetraEntity::Phy,
            msg: SapMsgInner::TpUnitdataReq(prim_phy),
        };
        queue.push_back(m);
    }

    fn rx_tmv_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tmv_prim");

        match message.msg {
            SapMsgInner::TmvConfigureReq(_) => {
                self.rx_tmv_configure_req(queue, message);
            }
            SapMsgInner::TmvUnitdataReq(_) => {
                self.rx_tmv_unitdata_req_slot(queue, message);
            }
            // SapMsgInner::CmceCallControl(_) => {
            //     self.rx_control(queue, message);
            // }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    // fn rx_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {

    //     tracing::trace!("rx_control");
    //     let SapMsgInner::CmceCallControl(prim) = message.msg else {panic!()};

    //     match prim {
    //         CallControl::Open(_) => {
    //             self.rx_control_circuit_open(queue, prim);
    //         },
    //         CallControl::Close(_, _) => {
    //             self.rx_control_circuit_close(queue, prim);

    //         },
    //     }
    // }
}

impl TetraEntityTrait for LmacBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Lmac
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TpSap => {
                self.rx_tp_prim(queue, message);
            }
            Sap::TmvSap => {
                self.rx_tmv_prim(queue, message);
            }
            other => {
                tracing::error!("LMAC: unexpected SAP {:?} -- routing error, dropping", other);
            }
        }
    }

    fn tick_start(&mut self, _queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        let stale_before = ts.add_timeslots(-2);
        for marker in &mut self.blk2_stolen_at {
            if marker.is_some_and(|marked_time| marked_time != stale_before && marked_time.diff(stale_before) < 0) {
                *marker = None;
            }
        }
    }
}
