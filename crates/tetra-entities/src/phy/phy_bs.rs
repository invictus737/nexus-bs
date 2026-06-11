use crossbeam_channel::Sender;

use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, BurstType, PhyBlockNum, PhyBlockType, Sap, TdmaTime, TrainingSequence, unimplemented_log};
use tetra_pdus::phy::traits::rxtx_dev::{RxBurstBits, RxSlotBits, RxTxDev, TxSlotBits};
use tetra_saps::tp::TpUnitdataInd;
use tetra_saps::{SapMsg, SapMsgInner};

use crate::net_control::{ControlCommand, ControlEndpoint};
use crate::phy::components::phy_io_file::{FileWriteMsg, PhyIoFileMode};
use crate::phy::components::{burst_consts::*, slotter, train_consts::*};
use crate::umac::subcomp::bs_sched::MACSCHED_TX_AHEAD;
use crate::{MessageQueue, TetraEntityTrait};

use super::components::phy_io_file::PhyIoFile;

pub struct PhyBs<D: RxTxDev> {
    config: SharedConfig,
    dltime: TdmaTime,

    /// Channel for asynchronous downlink TX data logging
    dl_tx_sender: Option<Sender<FileWriteMsg>>,
    /// Channel for asynchronous uplink RX data logging
    ul_rx_sender: Option<Sender<FileWriteMsg>>,

    /// Testing mode: Transmit input data from file instead of from stack
    dl_input_file: Option<PhyIoFile>,
    /// Testing mode: Parse input data from file instead of from SDR
    ul_input_file: Option<PhyIoFile>,

    /// RX/TX device
    rxtxdev: D,
    carrier_inhibited_applied: bool,
    control: Option<ControlEndpoint>,

    tick: u64,
}

impl<D: RxTxDev> PhyBs<D> {
    pub fn new(config: SharedConfig, rxtxdev: D, control: Option<ControlEndpoint>) -> Self {
        let c = &config.config().phy_io;
        let initial_carrier_inhibited = config.state_read().carrier_inhibited;
        let mut rxtxdev = rxtxdev;
        if initial_carrier_inhibited {
            if let Err(err) = rxtxdev.set_tx_inhibited(true) {
                tracing::error!(?err, "PHY: failed to apply initial RF carrier inhibit");
            }
        }

        // Create async writers for file logging of generated DL and received UL signals
        let dl_tx_logger = c
            .dl_tx_file
            .as_ref()
            .and_then(|f| PhyIoFile::create_async_writer(f, "dl_tx_logger".to_string()).ok());
        let ul_rx_logger = c
            .ul_rx_file
            .as_ref()
            .and_then(|f| PhyIoFile::create_async_writer(f, "ul_rx_logger".to_string()).ok());

        // Open input files overriding either generated DL or received UL data
        let dl_input_file = if let Some(ref f) = c.dl_input_file {
            Some(PhyIoFile::new(f, PhyIoFileMode::ReadRepeat).expect("Failed to open dl_input_file"))
        } else {
            None
        };
        let ul_input_file = if let Some(ref f) = c.ul_input_file {
            Some(PhyIoFile::new(f, PhyIoFileMode::Read).expect("Failed to open ul_input_file"))
        } else {
            None
        };

        Self {
            config,
            dltime: TdmaTime::default(), // updated in tick_start
            dl_tx_sender: dl_tx_logger,
            ul_rx_sender: ul_rx_logger,
            dl_input_file,
            ul_input_file,
            rxtxdev,
            carrier_inhibited_applied: initial_carrier_inhibited,
            control,
            tick: 0,
        }
    }

    fn sync_carrier_inhibit_state(&mut self) -> bool {
        let inhibited = self.config.state_read().carrier_inhibited;
        if inhibited != self.carrier_inhibited_applied {
            match self.rxtxdev.set_tx_inhibited(inhibited) {
                Ok(()) => {
                    self.carrier_inhibited_applied = inhibited;
                    tracing::warn!("PHY: RF carrier {}", if inhibited { "hard-inhibited" } else { "enabled" });
                }
                Err(err) => {
                    tracing::error!(
                        ?err,
                        "PHY: failed to {} RF carrier",
                        if inhibited { "hard-inhibit" } else { "enable" }
                    );
                }
            }
        }
        inhibited
    }

    fn send_rxblock_to_lmac(
        queue: &mut MessageQueue,
        train_type: TrainingSequence,
        burst_type: BurstType,
        block_type: PhyBlockType,
        block_num: PhyBlockNum,
        bits: BitBuffer,
        rssi_dbfs: f32,
    ) {
        // Uplink timeslot is two after downlink. Thus was transmitted at dltime - 2
        let sapmsg = SapMsg {
            sap: Sap::TpSap,
            src: TetraEntity::Phy,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TpUnitdataInd(TpUnitdataInd {
                train_type,
                burst_type,
                block_type,
                block_num,
                block: bits,
                rssi_dbfs,
            }),
        };
        queue.push_back(sapmsg);
    }

    fn split_rxslot_and_send_to_lmac(queue: &mut MessageQueue, burst: &RxBurstBits<'_>) {
        let train_seq = burst.train_type;
        match train_seq {
            TrainingSequence::NormalTrainSeq1 => {
                // burst.bits is a variable-length slice from the demodulator. A length
                // mismatch (DSP glitch, misconfiguration) would otherwise panic on the
                // slice index below — drop and log instead so the cell survives.
                if burst.bits.len() != NUB_BITS {
                    tracing::warn!("PHY: NUB burst wrong length ({} != {}), dropping", burst.bits.len(), NUB_BITS);
                    return;
                }

                let mut blk = BitBuffer::new(NUB_BLK_BITS * 2);
                blk.copy_bits_from_bitarr(&burst.bits[NUB_BLK1_OFFSET..NUB_BLK1_OFFSET + NUB_BLK_BITS]);
                blk.copy_bits_from_bitarr(&burst.bits[NUB_BLK2_OFFSET..NUB_BLK2_OFFSET + NUB_BLK_BITS]);
                blk.seek(0);

                Self::send_rxblock_to_lmac(
                    queue,
                    train_seq,
                    BurstType::NUB,
                    PhyBlockType::NUB,
                    PhyBlockNum::Both,
                    blk,
                    burst.rssi_dbfs,
                );
            }

            TrainingSequence::NormalTrainSeq2 => {
                if burst.bits.len() != NUB_BITS {
                    tracing::warn!("PHY: NUB burst wrong length ({} != {}), dropping", burst.bits.len(), NUB_BITS);
                    return;
                }

                let blk1 = BitBuffer::from_bitarr(&burst.bits[NUB_BLK1_OFFSET..NUB_BLK1_OFFSET + NUB_BLK_BITS]);
                let blk2 = BitBuffer::from_bitarr(&burst.bits[NUB_BLK2_OFFSET..NUB_BLK2_OFFSET + NUB_BLK_BITS]);

                Self::send_rxblock_to_lmac(
                    queue,
                    train_seq,
                    BurstType::NUB,
                    PhyBlockType::NUB,
                    PhyBlockNum::Block1,
                    blk1,
                    burst.rssi_dbfs,
                );
                Self::send_rxblock_to_lmac(
                    queue,
                    train_seq,
                    BurstType::NUB,
                    PhyBlockType::NUB,
                    PhyBlockNum::Block2,
                    blk2,
                    burst.rssi_dbfs,
                );
            }
            TrainingSequence::ExtendedTrainSeq => {
                if burst.bits.len() != CUB_BITS {
                    tracing::warn!("PHY: CUB burst wrong length ({} != {}), dropping", burst.bits.len(), CUB_BITS);
                    return;
                }

                let mut blk = BitBuffer::new(CUB_BLK_BITS * 2);
                blk.copy_bits_from_bitarr(&burst.bits[CUB_BLK1_OFFSET..CUB_BLK1_OFFSET + CUB_BLK_BITS]);
                blk.copy_bits_from_bitarr(&burst.bits[CUB_BLK2_OFFSET..CUB_BLK2_OFFSET + CUB_BLK_BITS]);
                blk.seek(0);

                Self::send_rxblock_to_lmac(
                    queue,
                    train_seq,
                    BurstType::CUB,
                    PhyBlockType::SSN1,
                    PhyBlockNum::Block1,
                    blk,
                    burst.rssi_dbfs,
                );
            }

            // SyncTrainSeq, NormalTrainSeq3 and NotFound are not handled here (sync bursts
            // are processed elsewhere; NotFound is filtered by the caller). A real demod
            // can legitimately classify a burst as SyncTrainSeq, so this must NOT be an
            // unreachable!()/panic — drop and log instead.
            other => {
                tracing::debug!("PHY: training sequence {:?} not handled in split_rxslot, dropping", other);
            }
        }
    }

    fn handle_rx_slots(
        queue: &mut MessageQueue,
        dltime: TdmaTime,
        tick: u64,
        ul_rx_sender: Option<&Sender<FileWriteMsg>>,
        rx: Vec<Option<RxSlotBits<'_>>>,
    ) {
        // Process received slot (either full, subslot1 or subslot2).
        // In exceptional cases, we might receive multiple slots from false
        // positives or split subslots; LMAC error control filters those.
        for rx_slot in rx {
            if let Some(rx_slot) = rx_slot {
                let mut slot_sent = false;
                if rx_slot.slot.train_type != TrainingSequence::NotFound {
                    tracing::debug!(ts=%dltime, "rx_tpsap_prim got {:?} in fullslot", rx_slot.slot.train_type);

                    if let Some(ul_rx_sender) = ul_rx_sender {
                        let _ = ul_rx_sender.try_send(FileWriteMsg::WriteHeaderAndBlock(3, tick, rx_slot.slot.bits.to_vec()));
                    }

                    Self::split_rxslot_and_send_to_lmac(queue, &rx_slot.slot);
                    slot_sent = true;
                }
                if rx_slot.subslot1.train_type != TrainingSequence::NotFound {
                    tracing::debug!(ts=%dltime, "rx_tpsap_prim got {:?} in subslot1", rx_slot.subslot1.train_type);
                    if slot_sent {
                        tracing::warn!("Sending same burst twice to LMAC");
                    }
                    if let Some(ul_rx_sender) = ul_rx_sender {
                        let _ = ul_rx_sender.try_send(FileWriteMsg::WriteHeaderAndBlock(1, tick, rx_slot.subslot1.bits.to_vec()));
                    }

                    Self::split_rxslot_and_send_to_lmac(queue, &rx_slot.subslot1);
                    slot_sent = true;
                }
                if rx_slot.subslot2.train_type != TrainingSequence::NotFound {
                    tracing::debug!(ts=%dltime, "rx_tpsap_prim got {:?} in subslot2", rx_slot.subslot2.train_type);
                    if slot_sent {
                        tracing::warn!("Sending same burst twice to LMAC");
                    }
                    if let Some(ul_rx_sender) = ul_rx_sender {
                        let _ = ul_rx_sender.try_send(FileWriteMsg::WriteHeaderAndBlock(2, tick, rx_slot.subslot2.bits.to_vec()));
                    }

                    Self::split_rxslot_and_send_to_lmac(queue, &rx_slot.subslot2);
                }
            }
        }
    }

    fn rx_tpsap_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        // Handle TpUnitdataReq with a TX slot
        // Prepare TxSlotBits for transmission
        // TODO FIXME: optimize

        self.tick += 1;

        let SapMsgInner::TpUnitdataReq(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if self.sync_carrier_inhibit_state() {
            let ul_rx_sender = self.ul_rx_sender.clone();
            let rx = self.rxtxdev.rxtx_timeslot(&[]).expect("Got error from rxtx_timeslot");
            Self::handle_rx_slots(queue, self.dltime, self.tick, ul_rx_sender.as_ref(), rx);
            return;
        }

        // Generate block (from file or from LMAC data)
        let mut dl_burst = [0u8; TIMESLOT_TYPE4_BITS];
        if let Some(dl_input_file) = &mut self.dl_input_file {
            // Code for testing mode, when replaying from DL input file
            dl_input_file.read_block(&mut dl_burst).expect("Failed to read dl_input_file data");
        } else {
            // We received data from LMAC, convert BBK block to bitarr
            assert!(prim.bbk.is_some());
            let mut bbk = [0u8; 30];
            prim.bbk.unwrap().to_bitarr(&mut bbk);

            // Build NDB or SDB burst
            dl_burst = match prim.burst_type {
                BurstType::SDB => {
                    // SDB burst
                    assert!(prim.train_type == TrainingSequence::SyncTrainSeq);
                    assert!(prim.blk1.is_some() && prim.blk2.is_some());

                    let mut blk1 = [0u8; 120];
                    let mut blk2 = [0u8; 216];
                    prim.blk1.unwrap().to_bitarr(&mut blk1); // Guaranteed for SDB
                    prim.blk2.unwrap().to_bitarr(&mut blk2); // Guaranteed for SDB

                    slotter::build_sdb(&blk1, &bbk, &blk2)
                }
                BurstType::NDB => {
                    let mut blk1 = [0u8; 216];
                    let mut blk2 = [0u8; 216];

                    match prim.train_type {
                        TrainingSequence::NormalTrainSeq1 => {
                            // Single large block
                            assert!(prim.blk1.is_some() && prim.blk2.is_none());
                            let mut blk1_src = prim.blk1.unwrap(); // Guaranteed for NDB
                            blk1_src.to_bitarr(&mut blk1);
                            blk1_src.to_bitarr(&mut blk2);
                        }
                        TrainingSequence::NormalTrainSeq2 => {
                            // Two half slots
                            assert!(prim.blk1.is_some() && prim.blk2.is_some());
                            prim.blk1.unwrap().to_bitarr(&mut blk1); // Guaranteed for NDB
                            prim.blk2.unwrap().to_bitarr(&mut blk2); // Guaranteed for NDB trainseq 2
                        }
                        _ => {
                            tracing::warn!("PHY: unsupported training sequence {:?} for NDB burst, dropping", prim.train_type);
                            return;
                        }
                    }

                    slotter::build_ndb(prim.train_type, &blk1, &bbk, &blk2)
                }
                _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
            };
        }

        // Prepare the TX slot for the tx device
        let tx_slot: [TxSlotBits; 1] = [TxSlotBits {
            time: self.dltime.add_timeslots(MACSCHED_TX_AHEAD as i32),
            slot: Some(&dl_burst),
            ..Default::default()
        }];

        // Code for testing mode, when capturing all DL output to file
        if let Some(dl_tx_sender) = &self.dl_tx_sender {
            let _ = dl_tx_sender.try_send(FileWriteMsg::WriteBlock(dl_burst.to_vec()));
        }

        // Transmit slot and receive rx data (if any trainseq was found)
        // This function is blocking and the source of timing sync in the whole stack
        // let tick_done = std::time::Instant::now();
        let ul_rx_sender = self.ul_rx_sender.clone();
        let rx = self.rxtxdev.rxtx_timeslot(&tx_slot).expect("Got error from rxtx_timeslot");
        // let new_tick_start = std::time::Instant::now();
        // let elapsed = new_tick_start.duration_since(tick_done);
        // tracing::debug!("rxtx_timeslot: tick_done {:?}, new_tick_start {:?}, elapsed {:?}", tick_done, new_tick_start, elapsed);

        Self::handle_rx_slots(queue, self.dltime, self.tick, ul_rx_sender.as_ref(), rx);
    }

    fn rx_tpc_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        // TPC SAP not implemented yet. Log instead of crashing the PHY worker.
        unimplemented_log!("rx_tpc_prim: TPC SAP not implemented");
    }

    fn process_control_commands(&mut self) {
        let commands: Vec<ControlCommand> = self
            .control
            .as_ref()
            .map(|cep| {
                let mut commands = Vec::new();
                while let Some(cmd) = cep.try_recv() {
                    commands.push(cmd);
                }
                commands
            })
            .unwrap_or_default();

        for cmd in commands {
            match cmd {
                ControlCommand::RunTxCalibration { calibration_path } => {
                    tracing::warn!("PHY: TX DC/IQ calibration requested path={}", calibration_path);
                    if !self.carrier_inhibited_applied {
                        let err = "RF carrier must be hard-inhibited in PHY before TX calibration".to_string();
                        tracing::error!("PHY: TX DC/IQ calibration refused: {}", err);
                        crate::rf_calibration::mark_failed(err);
                        continue;
                    }
                    crate::rf_calibration::mark_calibrating(&calibration_path);
                    match self.rxtxdev.run_tx_calibration(&calibration_path) {
                        Ok(()) => crate::rf_calibration::mark_calibrated("PHY calibration finished"),
                        Err(err) => {
                            tracing::error!("PHY: TX DC/IQ calibration failed: {}", err);
                            crate::rf_calibration::mark_failed(err);
                        }
                    }
                }
                other => tracing::warn!("PHY: ignoring unsupported control command {:?}", other),
            }
        }
    }
}

impl<D: RxTxDev + Send + 'static> TetraEntityTrait for PhyBs<D> {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Phy
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TpSap => {
                self.rx_tpsap_prim(queue, message);
            }
            Sap::TpcSap => {
                self.rx_tpc_prim(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn tick_start(&mut self, _queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        self.process_control_commands();
    }
}
