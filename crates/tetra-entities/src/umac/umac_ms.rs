use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, PhyBlockNum, PhysicalChannel, Sap, SsiType, TdmaTime, Todo, unimplemented_log};
use tetra_saps::tlmb::{TlmbSyncInd, TlmbSysinfoInd};
use tetra_saps::tma::{TmaReport, TmaReportInd, TmaUnitdataInd};
use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
use tetra_saps::tmv::{TmvConfigureReq, TmvUnitdataReq, TmvUnitdataReqSlot};
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::umac::enums::broadcast_type::BroadcastType;
use tetra_pdus::umac::enums::mac_pdu_type::MacPduType;
use tetra_pdus::umac::pdus::access_assign::AccessAssign;
use tetra_pdus::umac::pdus::access_assign_fr18::AccessAssignFr18;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_end_dl::MacEndDl;
use tetra_pdus::umac::pdus::mac_frag_dl::MacFragDl;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_pdus::umac::pdus::mac_sync::MacSync;
use tetra_pdus::umac::pdus::mac_sysinfo::MacSysinfo;

use crate::umac::subcomp::fillbits;
use crate::umac::subcomp::ms_defrag::MsDefrag;
use crate::{MessagePrio, MessageQueue, TetraEntityTrait};

const SCH_HU_TYPE1_CAP_BITS: usize = 92;

pub struct UmacMs {
    // config: Option<SharedConfig>,
    dltime: TdmaTime,
    self_component: TetraEntity,
    config: SharedConfig,
    defrag: MsDefrag,

    /// Provided by MLE over TlmbSap, to compute scrambling code, which is passed to lmac
    mcc: Option<u16>,
    /// Provided by MLE over TlmbSap, to compute scrambling code, which is passed to lmac
    mnc: Option<u16>,
    /// Provided by MLE over TlmbSap, to compute scrambling code, which is passed to lmac
    cc: Option<u8>,
    /// Derived from mcc/mnc, and passed to lmac
    scrambling_code: Option<u32>,
}

impl UmacMs {
    pub fn new(config: SharedConfig) -> Self {
        Self {
            dltime: TdmaTime::default(),
            self_component: TetraEntity::Umac,
            config,
            defrag: MsDefrag::new(),

            mcc: None,
            mnc: None,
            cc: None,
            scrambling_code: None,
        }
    }

    fn rx_tmv_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tmv_prim");
        match message.msg {
            SapMsgInner::TmvUnitdataInd(_) => {
                self.rx_tmv_unitdata_ind(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    pub fn rx_tmv_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        tracing::trace!("rx_tmv_unitdata_ind: {:?}", prim.logical_channel);

        match prim.logical_channel {
            LogicalChannel::Aach => {
                self.rx_tmv_aach(queue, message);
            }

            LogicalChannel::Bsch => {
                self.rx_tmv_bsch(queue, message);
            }

            LogicalChannel::SchF => {
                // Full slot signalling
                assert!(
                    prim.block_num == PhyBlockNum::Both,
                    "{:?} can't have block_num {:?}",
                    prim.logical_channel,
                    prim.block_num
                );
                self.rx_tmv_sch(queue, message);
            }

            LogicalChannel::Bnch | LogicalChannel::Stch | LogicalChannel::SchHd => {
                // Half slot signalling
                assert!(
                    matches!(prim.block_num, PhyBlockNum::Block1 | PhyBlockNum::Block2),
                    "{:?} can't have block_num {:?}",
                    prim.logical_channel,
                    prim.block_num
                );
                self.rx_tmv_sch(queue, message);
            }
            _ => unreachable!("invalid channel: {:?}", prim.logical_channel),
        }
    }

    /// Receive signalling (SCH, or STCH / BNCH)
    pub fn rx_tmv_sch(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tmv_sch");

        // Iterate until no more messages left in mac block
        loop {
            // Extract info from inner block
            let SapMsgInner::TmvUnitdataInd(prim) = &message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            };
            let Some(bits) = prim.pdu.peek_bits(3) else {
                tracing::warn!("insufficient bits: {}", prim.pdu.dump_bin());
                return;
            };
            let Ok(pdu_type) = MacPduType::try_from(bits >> 1) else {
                tracing::warn!("invalid pdu type: {}", bits >> 1);
                return;
            };
            let orig_start = prim.pdu.get_raw_start();
            let lchan = prim.logical_channel;

            match pdu_type {
                MacPduType::MacResourceMacData => {
                    self.rx_mac_resource(queue, &mut message);
                }
                MacPduType::MacFragMacEnd => {
                    // Also need third bit; designates mac-frag versus mac-end
                    if bits & 1 == 0 {
                        self.rx_mac_frag(queue, &mut message);
                    } else {
                        self.rx_mac_end(queue, &mut message);
                    }
                }
                MacPduType::Broadcast => {
                    self.rx_broadcast(queue, &mut message);
                }
                MacPduType::SuppMacUSignal => {
                    if lchan == LogicalChannel::Stch {
                        // U-SIGNAL since we're on the stealing channel
                        self.rx_usignal(queue, &mut message);
                    } else {
                        self.rx_supp(queue, &mut message);
                    }
                }
            }

            // Check if end of message reached by re-borrowing inner
            // If start was not updated, we also consider it end of message
            // If 16 or more bits remain (len of null pdu), we continue parsing
            if let SapMsgInner::TmvUnitdataInd(prim) = &message.msg {
                if prim.pdu.get_raw_start() != orig_start && prim.pdu.get_len() >= 16 {
                    tracing::trace!(
                        "rx_tmv_unitdata_ind_sch: Remaining {} bits: {:?}",
                        prim.pdu.get_len_remaining(),
                        prim.pdu.dump_bin_full(true)
                    );
                } else {
                    tracing::trace!("rx_tmv_unitdata_ind_sch: End of message reached");
                    break;
                }
            }
        }
    }

    // message pos: start of broadcast frame
    // Will NOT advance pos but pass to underlying function
    fn rx_broadcast(&self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_broadcast");

        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.peek_bits(2).unwrap() == MacPduType::Broadcast.into_raw()); // MAC PDU type

        let bits = prim.pdu.peek_bits_posoffset(2, 2).unwrap();
        let bcast_type = BroadcastType::try_from(bits).expect("invalid broadcast type");

        match bcast_type {
            BroadcastType::Sysinfo => {
                self.rx_broadcast_sysinfo(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    // Parses the sysinfo pdu
    fn rx_broadcast_sysinfo(&self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_broadcast_sysinfo");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Parse SYSINFO header and optional data
        let pdu = match MacSysinfo::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacSysinfo: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // TODO FIXME adopt sysinfo info into global state
        if pdu.hyperframe_number.is_some() && pdu.hyperframe_number.unwrap() != self.dltime.h {
            // Send message to Phy about new hyperframe number
            let mut new_time = self.dltime;
            new_time.h = pdu.hyperframe_number.unwrap();
            let t = TdmaTime {
                t: self.dltime.t,
                f: self.dltime.f,
                m: self.dltime.m,
                h: pdu.hyperframe_number.unwrap(),
            };
            let m = SapMsg {
                sap: Sap::TmvSap,
                src: self.self_component,
                dest: TetraEntity::Lmac,
                msg: SapMsgInner::TmvConfigureReq(TmvConfigureReq {
                    time: Some(t),
                    ..Default::default()
                }),
            };
            tracing::info!("rx_broadcast_sysinfo: Updated TdmaTime: {:?} -> {:?}", self.dltime, new_time);
            queue.push_back(m);
        }

        let tlsdu = BitBuffer::from_bitbuffer_pos(&prim.pdu);
        let m = SapMsg {
            sap: Sap::TlmbSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::TlmbSysinfoInd(TlmbSysinfoInd {
                endpoint_id: 0,
                tl_sdu: tlsdu,
                mac_broadcast_info: None,
            }),
        };

        queue.push_back(m);
    }

    fn rx_mac_resource(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_resource");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        // Parse header and optional ChanAlloc
        let pdu = match MacResource::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacResource: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        if pdu.encryption_mode > 0 {
            unimplemented_log!("rx_mac_resource: Encryption mode > 0, not implemented");
        }

        // Compute len
        let mut pdu_len_bits = {
            match pdu.length_ind {
                0b000001..0b111010 => {
                    // tracing::trace!("rx_mac_resource: length_ind {}", pdu.length_ind);
                    pdu.length_ind as usize * 8
                }
                0b111110 => {
                    // Second half slot stolen in STCH
                    unimplemented_log!("rx_mac_resource: SECOND HALF SLOT STOLEN IN STCH but signal not implemented");
                    prim.pdu.get_len()
                }
                0b111111 => {
                    // Start of fragmentation
                    // tracing::trace!("rx_mac_resource: frag start length_ind {}", pdu.length_ind);
                    prim.pdu.get_len()
                }
                _ => {
                    tracing::warn!("UMAC: rx_mac_resource: unexpected length_ind {:#08b}, dropping PDU", pdu.length_ind);
                    return;
                }
            }
        };

        if pdu_len_bits > prim.pdu.get_len() {
            // EN 300 392-2 clause 21.4.3.1 defines MAC-RESOURCE length_ind
            // as the MAC PDU length. If the announced SDU window is not
            // present on the air input, do not synthesize a truncated TM-SDU
            // for LLC/TMA delivery (clauses 20.4.1.1.3 and 20.4.1.1.4).
            tracing::warn!(
                "rx_mac_resource: dropping oversized MAC-RESOURCE length_ind={} ({} bits) for {} bit block",
                pdu.length_ind,
                pdu_len_bits,
                prim.pdu.get_len()
            );
            return;
        }

        // Strip fill bits. Maintain original end to allow for later parsing of a second mac block
        tracing::trace!("rx_mac_resource: {}", prim.pdu.dump_bin_full(true));
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, pdu.is_null_pdu())
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::trace!(
            "rx_mac_resource: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        if pdu.addr.is_none() {
            // TODO not sure if there is scenarios in which we want to pass a null pdu to the LLC
            // tracing::warn!("rx_mac_resource: Null PDU not passed to LLC");
            return;
        }

        // Decrypt if needed
        if pdu.encryption_mode > 0 {
            unimplemented_log!("rx_mac_resource: Encryption mode > 0");
            return;
            // TODO:
            // Check if key available
            // generate keystream
            // apply keystream to data
            // re-decode chanalloc
            // continue
        }

        tracing::debug!("rx_mac_resource: {}", prim.pdu.dump_bin_full(true));
        if pdu.length_ind == 0b111111 {
            // Fragmentation start, add to defragmenter
            self.defrag.insert_first(&mut prim.pdu, self.dltime, pdu.addr.unwrap(), None);
        } else if pdu.length_ind == 0b111110 {
            tracing::warn!("rx_mac_resource: SECOND HALF SLOT STOLEN IN STCH but not implemented");
        } else {
            // Pass directly to LLC
            let sdu = {
                if pdu.length_ind == 0 {
                    None // Null PDU
                } else if prim.pdu.get_len_remaining() == 0 {
                    None // No more data in this block
                } else {
                    // TODO FIXME should not copy here but take ownership
                    // Copy inner part, without MAC header or fill bits
                    Some(BitBuffer::from_bitbuffer_pos(&prim.pdu))
                }
            };
            // tracing::debug!("rx_mac_resource: sdu: {:?}", sdu.as_ref().unwrap().dump_bin_full(true));

            if sdu.is_some() {
                // We have an SDU for the LLC, deliver it.
                let m = SapMsg {
                    sap: Sap::TmaSap,
                    src: TetraEntity::Umac,
                    dest: TetraEntity::Llc,
                    msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                        pdu: sdu,
                        main_address: pdu.addr.unwrap(),
                        scrambling_code: prim.scrambling_code,
                        endpoint_id: 0,        // TODO FIXME
                        new_endpoint_id: None, // TODO FIXME
                        css_endpoint_id: None, // TODO FIXME
                        air_interface_encryption: pdu.encryption_mode as Todo,
                        chan_change_response_req: false,
                        chan_change_handle: None,
                        chan_info: None,
                    }),
                };
                queue.push_back(m);
            } else {
                // Either this is a null pdu or we are at the end of the block
                // For now, we don't deliver this. However, important data may need to be signalled upwards
                tracing::info!("rx_mac_resource: empty PDU not passed to LLC");
            }
        }

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_mac_frag(&mut self, _queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_frag");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        // Parse header and optional ChanAlloc
        let pdu = match MacFragDl::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacFragDl: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Strip fill bits. This message is known to fill the slot.
        let mut pdu_len_bits = prim.pdu.get_len();
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::debug!("rx_mac_frag: pdu_len_bits: {} fill_bits: {}", pdu_len_bits, num_fill_bits);

        // Decrypt if needed
        if let Some(_aie_info) = self.defrag.buffers[(self.dltime.t - 1) as usize].aie_info {
            // TODO FIXME implement
            unimplemented_log!("rx_mac_frag: Encryption not supported");
            return;
        }

        // Insert into defragmenter
        self.defrag.insert_next(&mut prim.pdu, self.dltime);
    }

    fn rx_mac_end(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_end");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        // Parse header and optional ChanAlloc
        let pdu = match MacEndDl::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacEndDl: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Compute len. EN 300 392-2 table 21.59 reserves length_ind=0; the
        // parser rejects it, so any remaining oversized declaration is corrupt
        // input and must be dropped before fill-bit removal reads past the slot.
        let mut pdu_len_bits = pdu.length_ind as usize * 8;
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!(
                "rx_mac_end: dropping oversized MAC-END length_ind={} ({} bits) for {} bit block",
                pdu.length_ind,
                pdu_len_bits,
                prim.pdu.get_len()
            );
            return;
        }

        // Strip fill bits. Maintain original end to allow for later parsing of a second mac block
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::debug!("rx_mac_end: pdu_len_bits: {} fill_bits: {}", pdu_len_bits, num_fill_bits);

        // Decrypt if needed
        if let Some(_aie_info) = self.defrag.buffers[(self.dltime.t - 1) as usize].aie_info {
            // EN 300 392-2 air-interface encryption is not implemented in
            // this stack. Drop encrypted continuations instead of panicking or
            // forwarding undeciphered bits as a clear C-plane SDU.
            unimplemented_log!("rx_mac_end: Encryption not supported");
            return;
        }

        // Insert into defragmenter
        self.defrag.insert_last(&mut prim.pdu, self.dltime);

        // Fetch finalized block
        let defragbuf = self.defrag.take_defragged_buf(self.dltime);
        let Some(defragbuf) = defragbuf else {
            tracing::warn!("rx_mac_end: could not obtain defragged buf");
            return;
        };

        // Pass block directly to LLC
        tracing::debug!("rx_mac_end: sdu: {:?}", defragbuf.buffer.dump_bin());

        let m = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                pdu: Some(defragbuf.buffer),
                main_address: defragbuf.addr,
                scrambling_code: prim.scrambling_code,
                endpoint_id: 0,              // TODO FIXME
                new_endpoint_id: None,       // TODO FIXME
                css_endpoint_id: None,       // TODO FIXME
                air_interface_encryption: 0, // TODO FIXME implement
                chan_change_response_req: false,
                chan_change_handle: None,
                chan_info: None,
            }),
        };
        queue.push_back(m);

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_usignal(&self, _queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_usignal");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        // EN 300 392-2 clause 21.4.5 defines MAC-U-SIGNAL on STCH for
        // U-plane signalling. This MS shim does not expose a U-plane
        // application yet, so ignore the TM-SDU instead of panicking.
        unimplemented_log!(
            "rx_usignal: MAC-U-SIGNAL/STCH reception not implemented; dropping {} bits",
            prim.pdu.get_len_remaining()
        );
    }

    fn rx_supp(&self, _queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_supp");

        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        // Check we're indeed on the right channel (Clause 21.4.1 Table 21.48)
        if prim.logical_channel == LogicalChannel::Stch || prim.logical_channel == LogicalChannel::SchHd {
            tracing::warn!(
                "rx_supp: supplementary MAC PDU on invalid logical channel {:?}; dropping",
                prim.logical_channel
            );
            return;
        }
        // EN 300 392-2 clause 21.4.2.5 defines MAC-U-BLCK as optional
        // event-label C-plane signalling. This MS shim has no event-label
        // mapping for supplementary downlink delivery, so fail closed.
        unimplemented_log!(
            "rx_supp: supplementary MAC PDU reception not implemented on {:?}; dropping {} bits",
            prim.logical_channel,
            prim.pdu.get_len_remaining()
        );
    }

    pub fn rx_tmv_aach(&self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tmv_aach");

        // TODO FIXME, more extensively store and process AACH state in both LMAC and UMAC
        // Then we send a msg down only if a change is needed, like we do for the scrambling code

        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let is_traffic = if self.dltime.f != 18 {
            let pdu = match AccessAssign::from_bitbuf(&mut prim.pdu) {
                Ok(pdu) => {
                    tracing::debug!("<- {:?}", pdu);
                    pdu
                }
                Err(e) => {
                    tracing::warn!("Failed parsing AccessAssign: {:?} {}", e, prim.pdu.dump_bin());
                    return;
                }
            };

            pdu.dl_usage.is_traffic()
        } else {
            let _pdu = match AccessAssignFr18::from_bitbuf(&mut prim.pdu) {
                Ok(pdu) => {
                    tracing::debug!("<- {:?}", pdu);
                    pdu
                }
                Err(e) => {
                    tracing::warn!("Failed parsing AccessAssignFr18: {:?} {}", e, prim.pdu.dump_bin());
                    return;
                }
            };

            false
        };

        let m = SapMsg {
            sap: Sap::TmvSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TmvConfigureReq(TmvConfigureReq {
                is_traffic: Some(is_traffic),
                ..Default::default()
            }),
        };
        // This message needs to be processed NOW since it affects the other blocks in this timeslot
        queue.push_prio(m, MessagePrio::Immediate);
    }

    pub fn rx_tmv_bsch(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tmv_bsch");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Unpack and validate with expected state
        let pdu = match MacSync::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacSync: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        self.dltime = pdu.time;
        self.cc = Some(pdu.colour_code);

        queue.push_back(SapMsg {
            sap: Sap::TmvSap,
            src: self.self_component,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TmvConfigureReq(TmvConfigureReq {
                time: Some(pdu.time),
                ..Default::default()
            }),
        });
        self.update_scrambing_and_submit_to_lmac(queue);

        let tlsdu = BitBuffer::from_bitbuffer_pos(&prim.pdu);
        queue.push_back(SapMsg {
            sap: Sap::TlmbSap,
            src: self.self_component,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::TlmbSyncInd(TlmbSyncInd {
                endpoint_id: 0,
                tl_sdu: tlsdu,
            }),
        });

        // let netinfo_changed = {
        //     let config_r = self.config.read();
        //         mac_sync.system_code != config_r.la_info.system_code
        //             || mac_sync.sharing_mode != config_r.la_info.sharing_mode
        //             || mac_sync.ts_reserved_frames != config_r.la_info.ts_reserved_frames
        //             || mac_sync.u_plane_dtx != config_r.la_info.u_plane_dtx
        //             || mac_sync.frame_18_ext != config_r.la_info.frame_18_ext
        // };
        // // tracing::trace!("rx_tmv_bsch: netinfo_changed: {}, cc_changed: {}, tdma_time_changed: {}", netinfo_changed, cc_changed, tdma_time_changed);

        // // Update global state if needed
        // if netinfo_changed  {
        //     let mut config_w = self.config.write();
        //     config_w.la_info.system_code = mac_sync.system_code;
        //     // config_w.netinfo.colour_code = mac_sync.colour_code;
        //     config_w.la_info.sharing_mode = mac_sync.sharing_mode;
        //     config_w.la_info.ts_reserved_frames = mac_sync.ts_reserved_frames;
        //     config_w.la_info.u_plane_dtx = mac_sync.u_plane_dtx;
        //     config_w.la_info.frame_18_ext = mac_sync.frame_18_ext;
        //     tracing::info!("rx_tmv_bsch: Updated TetraGlobalState: {:?}", mac_sync);
        // }

        // if mac_sync.time.t != message.t_submit.t || mac_sync.time.f != message.t_submit.f || mac_sync.time.m != message.t_submit.m {
        //     // TODO warn/bail when really not in line with expected time
        //     let t = TdmaTime{
        //         t: mac_sync.time.t,
        //         f: mac_sync.time.f,
        //         m: mac_sync.time.m,
        //         h: message.t_submit.h,
        //     };
        //     let m = SapMsg {
        //         sap: Sap::TmvSap,
        //         src: self.self_component,
        //         dest: TetraComponent::Lmac,
        //         t_submit: message.t_submit,
        //         msg: SapMsgInner::TmvConfigureReq(
        //             TmvConfigureReq{
        //                 time: Some(t),
        //                 .. Default::default()
        //             }
        //         )
        //     };
        //     tracing::info!("rx_tmv_bsch: Updated TdmaTime: {:?} -> {:?}", message.t_submit, t);
        //     queue.push_back(m);
        // }

        // if Some(mac_sync.colour_code) != self.cc {
        //     // Update scrambling code
        //     tracing::info!("rx_tmv_bsch: Updated colour code: {:?} -> {:?}", self.cc, mac_sync.colour_code);
        //     self.cc = Some(mac_sync.colour_code);
        //     self.update_scrambing_and_submit_to_lmac(queue, &message);

        // } else {
        //     tracing::trace!("rx_tmv_bsch: Colour code unchanged: {:?}", self.cc);
        // }

        // // Take ownership of prim and sdu
        // let prim = if let SapMsgInner::TmvUnitdataInd(inner) = message.msg {
        //     inner
        // } else {
        //     panic!();
        // };
        // let tlsdu = prim.pdu;

        // let m = SapMsg {
        //     sap: Sap::TlmbSap,
        //     src: TetraComponent::Umac,
        //     dest: TetraComponent::Mle,
        //     t_submit: message.t_submit,

        //     msg: SapMsgInner::TlmbSyncInd(
        //         TlmbSyncInd {
        //             endpoint_id: 0,
        //             tl_sdu: tlsdu
        //         }
        //     )
        // };
        // tracing::info!("rx_tmv_bsch: {:?}", m.msg);
        // queue.push_back(m);
    }

    fn rx_tma_unitdata_req(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tma_unitdata_req");
        let SapMsgInner::TmaUnitdataReq(mut prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if prim.air_interface_encryption.unwrap_or(0) != 0 {
            tracing::warn!(
                "UMAC-MS: encrypted TMA-UNITDATA req_handle={} not implemented, dropping",
                prim.req_handle
            );
            if let Some(tx_reporter) = prim.tx_reporter {
                tx_reporter.mark_discarded();
            }
            return;
        }

        if !matches!(prim.main_address.ssi_type, SsiType::Issi | SsiType::Ssi) {
            tracing::warn!(
                "UMAC-MS: MAC-ACCESS random access only supports individual SSI addresses, got {}",
                prim.main_address
            );
            if let Some(tx_reporter) = prim.tx_reporter {
                tx_reporter.mark_discarded();
            }
            return;
        }

        let sdu_len = prim.pdu.get_len();
        let mut mac_access = MacAccess {
            fill_bits: false,
            encrypted: false,
            addr: Some(prim.main_address),
            event_label: None,
            length_ind: None,
            frag_flag: None,
            reservation_req: None,
        };
        let mut header_probe = BitBuffer::new_autoexpand(32);
        mac_access.to_bitbuf(&mut header_probe);
        let header_len = header_probe.get_len_written();
        if header_len + sdu_len > SCH_HU_TYPE1_CAP_BITS {
            tracing::warn!(
                "UMAC-MS: TMA-UNITDATA req_handle={} is {} bits and does not fit one SCH/HU MAC-ACCESS payload ({} bits max), fragmentation not implemented",
                prim.req_handle,
                sdu_len,
                SCH_HU_TYPE1_CAP_BITS.saturating_sub(header_len)
            );
            if let Some(tx_reporter) = prim.tx_reporter {
                tx_reporter.mark_discarded();
            }
            return;
        }

        let fill_bits = SCH_HU_TYPE1_CAP_BITS - header_len - sdu_len;
        mac_access.fill_bits = fill_bits > 0;

        let mut mac_block = BitBuffer::new(SCH_HU_TYPE1_CAP_BITS);
        mac_access.to_bitbuf(&mut mac_block);
        prim.pdu.seek(0);
        mac_block.copy_bits(&mut prim.pdu, sdu_len);
        fillbits::addition::write(&mut mac_block, Some(fill_bits));
        mac_block.seek(0);

        if prim.stealing_permission {
            tracing::debug!(
                "UMAC-MS: TMA-UNITDATA req_handle={} permitted stealing, but no assigned uplink stealing state is active; using SCH/HU random access",
                prim.req_handle
            );
        }

        // EN 300 392-2 clauses 20.4.1.1.4, 21.4.2.1 and 23.5.2.4:
        // when the MS has C-plane signalling to send and no reserved capacity
        // is available, it may transmit a non-fragmented TM-SDU in a SCH/HU
        // MAC-ACCESS PDU. This implementation's UMAC/Lmac boundary has no
        // lower MAC completion primitive, so handoff to LMAC is the same local
        // completion boundary used by BS-side TxReporter handling.
        queue.push_back(SapMsg {
            sap: Sap::TmvSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TmvUnitdataReq(TmvUnitdataReqSlot {
                ts: self.dltime,
                ul_phy_chan: PhysicalChannel::Cp,
                blk1: Some(TmvUnitdataReq {
                    mac_block,
                    logical_channel: LogicalChannel::SchHu,
                    scrambling_code: self.scrambling_code.unwrap_or(0),
                }),
                blk2: None,
                bbk: None,
            }),
        });

        if let Some(tx_reporter) = prim.tx_reporter {
            tx_reporter.mark_transmitted();
        }
        // EN 300 392-2 clause 20.4.1.1.3 reports request progress/failure
        // from MAC to LLC. Clause 20.4.1.1.4 submitted this TM-SDU through
        // TMA-UNITDATA, and this path uses MAC-ACCESS random access rather
        // than reserved or stealing capacity, so report complete random-access
        // transmission with the retained request handle.
        queue.push_back(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaReportInd(TmaReportInd {
                req_handle: prim.req_handle,
                report: TmaReport::SuccessRandomAccess,
            }),
        });
    }

    fn rx_tma_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tma_prim");
        match message.msg {
            SapMsgInner::TmaUnitdataReq(_) => {
                self.rx_tma_unitdata_req(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn rx_tlmb_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tlmb_prim");
        // EN 300 392-2 clauses 20.3.5.3.1, 20.3.5.3.2 and 20.4.4:
        // TLMB/TMB indications on an MS carry decoded broadcast information
        // from MAC upwards to MLE. A TLMB primitive delivered back to UMAC is
        // therefore a local routing error; drop it instead of terminating the
        // whole MS stack.
        tracing::warn!("UMAC-MS received unexpected TLMB primitive {}, ignoring", message.msg);
    }

    fn update_scrambing_and_submit_to_lmac(&mut self, queue: &mut MessageQueue) {
        if let (Some(mcc), Some(mnc), Some(cc)) = (self.mcc, self.mnc, self.cc) {
            self.scrambling_code = Some((((cc as u32) | ((mnc as u32) << 6) | ((mcc as u32) << 20)) << 2) | 3);

            tracing::trace!(
                "compute_scrambling_and_submit_to_lmac cc {} mcc {} mnc {} scrambling_code: {}",
                cc,
                mcc,
                mnc,
                self.scrambling_code.unwrap()
            );

            let m = SapMsg {
                sap: Sap::TmvSap,
                src: self.self_component,
                dest: TetraEntity::Lmac,
                msg: SapMsgInner::TmvConfigureReq(TmvConfigureReq {
                    scrambling_code: self.scrambling_code,
                    ..Default::default()
                }),
            };
            queue.push_back(m);
        }
    }

    fn rx_tlmc_configure_req(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tlmc_configure_req");
        let SapMsgInner::TlmcConfigureReq(prim) = &message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if let Some(valid_addresses) = &prim.valid_addresses {
            tracing::debug!("rx_tlmc_configure_req: valid_addresses: {:?}", valid_addresses);

            self.mcc = Some(valid_addresses.mcc);
            self.mnc = Some(valid_addresses.mnc);

            // Attempt to update scrambling code (if cc is also known)
            self.update_scrambing_and_submit_to_lmac(queue);
        } else {
            tracing::warn!("rx_tlmc_configure_req: No valid addresses provided");
        }
    }

    fn rx_tlmc_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tlmc_prim");
        match message.msg {
            SapMsgInner::TlmcConfigureReq(_) => {
                self.rx_tlmc_configure_req(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }
}

impl TetraEntityTrait for UmacMs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Umac
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TmvSap => {
                self.rx_tmv_prim(queue, message);
            }

            Sap::TmaSap => {
                self.rx_tma_prim(queue, message);
            }

            Sap::TlmbSap => {
                self.rx_tlmb_prim(queue, message);
            }

            Sap::TlmcSap => {
                self.rx_tlmc_prim(queue, message);
            }

            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }
}
