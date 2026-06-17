// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::ComponentTest;
use tetra_config::bluestation::{CfgWapIp, DEFAULT_WAP_IP_MAX_REQUEST_PAYLOAD_BYTES, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Direction, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner, debug};
use tetra_entities::cmce::cmce_bs::CmceBs;
use tetra_entities::llc::components::fcs;
use tetra_entities::sndcp::ip::{bitbuffer_npdu_octets, build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
use tetra_entities::sndcp::pdch::{
    SndcpPacketDataPlanInput, SndcpPacketDataResourceRequest, SndcpPdchAllocationPolicy, SndcpPdchManager,
    SndcpPhaseModulationResourceRequest, packet_data_plan_to_lower_channel_allocation,
};
use tetra_entities::sndcp::pdp::{
    SndcpActivateAddressDemand, SndcpActivatePdpContextDemand, SndcpDeactivation, decode_activate_pdp_context_accept,
    decode_deactivate_pdp_context_demand, encode_activate_pdp_context_demand, encode_deactivate_pdp_context_accept,
    encode_deactivate_pdp_context_demand,
};
use tetra_entities::sndcp::sndcp_bs::{
    NetworkPduKind, SndcpDecode, SndcpEncodeError, SndcpRuntimeHandoffDecision, SndcpRuntimeHandoffPolicy, SndcpRuntimePduClass,
    decode_ltpd_sdu, encode_sn_unitdata,
};
use tetra_entities::sndcp::transfer::{
    SN_PDU_TYPE_DATA, SN_PDU_TYPE_END_OF_DATA, SndcpDataTransmitRequest, SndcpDataTransmitResponseResult, SndcpEndOfData,
    SndcpNotSupported, SndcpReconnect, SndcpTransferControl, SndcpTransferRejectCause, decode_data_transmit_response, decode_end_of_data,
    encode_data_transmit_request, encode_end_of_data, encode_not_supported, encode_reconnect,
};
use tetra_entities::sndcp::unitdata::decode_sn_user_data_pdu;
use tetra_entities::sndcp::wap_ip::DEFAULT_WAP_WSP_STATUS_MAX_BYTES;
use tetra_entities::umac::umac_bs::UmacBs;
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::u_disconnect::UDisconnect;
use tetra_pdus::cmce::pdus::u_setup::USetup;
use tetra_pdus::llc::enums::llc_pdu_type::LlcPduType;
use tetra_pdus::llc::pdus::al_ack::AlAck;
use tetra_pdus::llc::pdus::al_data::AlData;
use tetra_pdus::llc::pdus::al_setup::AlSetup;
use tetra_pdus::llc::pdus::bl_ack::BlAck;
use tetra_pdus::llc::pdus::bl_data::BlData;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_saps::SapMsg;
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::ltpd::{LtpdMleConfigureInd, LtpdMleConfigureReq, LtpdMleReportInd, LtpdMleUnitdataInd, LtpdMleUnitdataReq};
use tetra_saps::sapmsg::SapMsgInner;
use tetra_saps::sn::{SnAddress, SnPacketDataMsType};
use tetra_saps::tla::TlaTlUnitdataIndBl;
use tetra_saps::tma::{TmaReport, TmaReportInd, TmaUnitdataInd};

fn build_ltpd_ind(sap: Sap, sdu: BitBuffer) -> SapMsg {
    build_ltpd_ind_on_link(sap, sdu, 1, 2)
}

fn build_ltpd_ind_on_link(sap: Sap, sdu: BitBuffer, endpoint_id: u32, link_id: u32) -> SapMsg {
    SapMsg {
        sap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleUnitdataInd(LtpdMleUnitdataInd {
            sdu,
            endpoint_id,
            link_id,
            received_tetra_address: TetraAddress::new(1000001, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_ltpd_report_ind(handle: i32, transfer_result: i32) -> SapMsg {
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleReportInd(LtpdMleReportInd { handle, transfer_result }),
    }
}

fn build_ltpd_configure_req() -> SapMsg {
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleConfigureReq(LtpdMleConfigureReq {
            chan_change_accepted: Some(false),
            chan_change_handle: 9,
            call_release: -1,
            endpoint_id: 1,
            encryption_flag: false,
            ms_default_data_prio: -1,
            layer2_data_prio_lifetime: -1,
            layer2_data_prio_signalling_delay: -1,
            data_prio_random_access_delay_factor: -1,
            data_class_info: -1,
            schedule_repetition_info: -1,
            sndcp_status: 0,
        }),
    }
}

fn build_ltpd_configure_ind() -> SapMsg {
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleConfigureInd(LtpdMleConfigureInd {
            received_tetra_address: Some(TetraAddress::issi(1000001)),
            endpoint_id: 1,
            chan_change_responce_required: true,
            chan_change_handle: 10,
            reason_for_config_indication: 3,
            conflicting_endpoint_id: 2,
        }),
    }
}

fn build_sn_unitdata(nsapi: u8, pcomp: u8, dcomp: u8, n_pdu: &[u8]) -> BitBuffer {
    build_sn_user_data(4, nsapi, pcomp, dcomp, n_pdu)
}

fn build_sn_data(nsapi: u8, pcomp: u8, dcomp: u8, n_pdu: &[u8]) -> BitBuffer {
    build_sn_user_data(SN_PDU_TYPE_DATA, nsapi, pcomp, dcomp, n_pdu)
}

fn build_sn_user_data(sn_pdu_type: u8, nsapi: u8, pcomp: u8, dcomp: u8, n_pdu: &[u8]) -> BitBuffer {
    let mut sdu = BitBuffer::new(16 + n_pdu.len() * 8);
    sdu.write_bits(sn_pdu_type as u64, 4);
    sdu.write_bits(nsapi as u64, 4);
    sdu.write_bits(pcomp as u64, 4);
    sdu.write_bits(dcomp as u64, 4);
    for byte in n_pdu {
        sdu.write_bits(*byte as u64, 8);
    }
    sdu.seek(0);
    sdu
}

fn build_tla_sndcp_unitdata_ind(sndcp_sdu: BitBuffer) -> SapMsg {
    let sdu_len = sndcp_sdu.get_len();
    let mut tl_sdu = BitBuffer::new(3 + sdu_len);
    tl_sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
    let mut sndcp_sdu = BitBuffer::from_bitbuffer(&sndcp_sdu);
    sndcp_sdu.seek(0);
    tl_sdu.copy_bits(&mut sndcp_sdu, sdu_len);
    tl_sdu.seek(0);

    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlUnitdataIndBl(TlaTlUnitdataIndBl {
            main_address: TetraAddress::new(1000001, SsiType::Issi),
            link_id: 2,
            endpoint_id: 1,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: Some(tl_sdu),
            scrambling_code: 0,
            fcs_flag: false,
            air_interface_encryption: 0,
            chan_change_resp_req: false,
            chan_change_handle: None,
            chan_info: None,
            report: None,
        }),
    }
}

fn build_sn_pdu(sn_pdu_type: u8) -> BitBuffer {
    let mut sdu = BitBuffer::new(8);
    sdu.write_bits(sn_pdu_type as u64, 4);
    sdu.write_bits(0, 4);
    sdu.seek(0);
    sdu
}

fn build_tma_report(req_handle: i32, report: TmaReport) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaReportInd(TmaReportInd { req_handle, report }),
    }
}

fn build_al_setup_ind(addr: TetraAddress, endpoint_id: u32, al_number: u8) -> SapMsg {
    let setup = AlSetup {
        acknowledged_service: true,
        advanced_link_number: al_number,
        max_tl_sdu_len_code: 6,
        connection_width: false,
        advanced_link_symmetry: false,
        uplink_timeslots: None,
        downlink_timeslots: None,
        throughput_code: 6,
        window_size_code: 1,
        max_tl_sdu_retransmissions: 3,
        max_segment_retransmissions: 3,
        setup_report: AlSetup::SETUP_REPORT_SERVICE_DEFINITION,
        ns: None,
        augmented: None,
    };
    let mut pdu = BitBuffer::new_autoexpand(32);
    setup.to_bitbuf(&mut pdu);
    pdu.seek(0);

    build_tma_unitdata_ind(addr, endpoint_id, pdu)
}

fn build_bl_ack_ind(addr: TetraAddress, endpoint_id: u32, nr: u8) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(8);
    BlAck { has_fcs: false, nr }.to_bitbuf(&mut pdu);
    pdu.seek(0);

    build_tma_unitdata_ind(addr, endpoint_id, pdu)
}

fn build_al_ack_ind(addr: TetraAddress, endpoint_id: u32, nr: u8) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(16);
    AlAck::complete(nr).to_bitbuf(&mut pdu);
    pdu.seek(0);

    build_tma_unitdata_ind(addr, endpoint_id, pdu)
}

fn build_al_final_ar_ind(addr: TetraAddress, endpoint_id: u32, ns: u8, payload: &BitBuffer) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(64);
    AlData {
        final_segment: true,
        acknowledgement_requested: true,
        ns,
        ss: 0,
    }
    .to_bitbuf(&mut pdu);

    let payload_start = pdu.get_len_written();
    let mut payload = BitBuffer::from_bitbuffer(payload);
    payload.seek(0);
    let payload_len = payload.get_len();
    pdu.copy_bits(&mut payload, payload_len);
    let fcs_value = fcs::compute_fcs(&pdu, payload_start, pdu.get_len());
    pdu.write_bits(fcs_value as u64, 32);
    pdu.seek(0);

    build_tma_unitdata_ind(addr, endpoint_id, pdu)
}

fn build_tma_unitdata_ind(addr: TetraAddress, endpoint_id: u32, pdu: BitBuffer) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_mle_prefixed_sndcp_sdu(sndcp_sdu: BitBuffer) -> BitBuffer {
    let sdu_len = sndcp_sdu.get_len();
    let mut tl_sdu = BitBuffer::new(3 + sdu_len);
    tl_sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
    let mut sndcp_sdu = BitBuffer::from_bitbuffer(&sndcp_sdu);
    sndcp_sdu.seek(0);
    tl_sdu.copy_bits(&mut sndcp_sdu, sdu_len);
    tl_sdu.seek(0);
    tl_sdu
}

fn llc_pdu_type_from_tma_req(msg: &SapMsg) -> Option<LlcPduType> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    prim.pdu.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok())
}

fn bl_data_ns_from_tma_req(msg: &SapMsg) -> Option<u8> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    BlData::from_bitbuf(&mut pdu).ok().map(|pdu| pdu.ns)
}

fn al_data_ns_from_tma_req(msg: &SapMsg) -> Option<u8> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    AlData::from_bitbuf(&mut pdu).ok().map(|pdu| pdu.ns)
}

fn tma_req_handle(msg: &SapMsg) -> Option<i32> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    Some(prim.req_handle)
}

fn sndcp_user_data_from_al_tma_reqs(msgs: &[SapMsg]) -> Option<tetra_entities::sndcp::unitdata::SnUnitdata> {
    let mut ns = None;
    let mut final_ss = None;
    let mut segments: BTreeMap<u8, BitBuffer> = BTreeMap::new();

    for msg in msgs {
        let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
            continue;
        };
        let mut pdu = prim.pdu.clone();
        let Ok(header) = AlData::from_bitbuf(&mut pdu) else {
            continue;
        };
        if ns.get_or_insert(header.ns) != &header.ns {
            return None;
        }
        if header.final_segment {
            if !header.acknowledgement_requested {
                return None;
            }
            final_ss = Some(header.ss);
        }
        segments.entry(header.ss).or_insert_with(|| BitBuffer::from_bitbuffer_pos(&pdu));
    }

    let final_ss = final_ss?;
    let mut complete = BitBuffer::new_autoexpand(segments.values().map(BitBuffer::get_len).sum::<usize>());
    for ss in 0..=final_ss {
        let segment = segments.get(&ss)?;
        let mut segment = BitBuffer::from_bitbuffer(segment);
        let bits = segment.get_len_remaining();
        complete.copy_bits(&mut segment, bits);
    }
    complete.seek(0);

    if complete.get_len_remaining() < 32 || !fcs::check_fcs(&complete) {
        return None;
    }
    let payload_end = complete.get_raw_end().checked_sub(32)?;
    complete.set_raw_end(payload_end);
    if complete.read_bits(3)? != MleProtocolDiscriminator::Sndcp.into_raw() as u64 {
        return None;
    }
    let remaining_bits = complete.get_len_remaining();
    let mut sndcp_sdu = BitBuffer::new(remaining_bits);
    sndcp_sdu.copy_bits(&mut complete, remaining_bits);
    sndcp_sdu.seek(0);
    decode_sn_user_data_pdu(&sndcp_sdu).ok()
}

fn build_dynamic_ipv4_activation_demand(nsapi: u8) -> BitBuffer {
    build_dynamic_ipv4_activation_demand_with_ms_type(nsapi, SnPacketDataMsType::TypeAParallel)
}

fn build_dynamic_ipv4_activation_demand_with_ms_type(nsapi: u8, packet_data_ms_type: SnPacketDataMsType) -> BitBuffer {
    encode_activate_pdp_context_demand(&SndcpActivatePdpContextDemand {
        sndcp_version: 1,
        nsapi,
        address: SndcpActivateAddressDemand::Ipv4Dynamic,
        packet_data_ms_type,
        pcomp_negotiation: 0,
    })
    .expect("minimal IPv4 PDP activation demand should encode")
}

fn build_wap_status_sn_unitdata(nsapi: u8) -> BitBuffer {
    let n_pdu =
        build_ipv4_udp_npdu([10, 0, 0, 82], [10, 0, 0, 1], 49_152, 9200, b"", 0x2260, 32).expect("WAP IPv4/UDP probe N-PDU should build");
    build_sn_unitdata(nsapi, 0, 0, &n_pdu)
}

fn build_wap_status_sn_unitdata_from(nsapi: u8, source: [u8; 4], payload: &[u8]) -> BitBuffer {
    let n_pdu =
        build_ipv4_udp_npdu(source, [10, 0, 0, 1], 49_152, 9200, payload, 0x2260, 32).expect("WAP IPv4/UDP probe N-PDU should build");
    build_sn_unitdata(nsapi, 0, 0, &n_pdu)
}

fn build_wap_status_sn_data_from(nsapi: u8, source: [u8; 4], payload: &[u8]) -> BitBuffer {
    let n_pdu =
        build_ipv4_udp_npdu(source, [10, 0, 0, 1], 49_152, 9200, payload, 0x2260, 32).expect("WAP IPv4/UDP probe N-PDU should build");
    build_sn_data(nsapi, 0, 0, &n_pdu)
}

fn build_wtp_wsp_get_payload(transaction_id: u16, uri: &str) -> Vec<u8> {
    assert!(uri.len() < 128, "test WSP GET URI should fit one uintvar octet");
    let mut payload = Vec::with_capacity(5 + uri.len());
    payload.push(0x0a);
    payload.extend_from_slice(&(transaction_id & 0x7fff).to_be_bytes());
    payload.push(0x12);
    payload.push(0x40);
    payload.push(uri.len() as u8);
    payload.extend_from_slice(uri.as_bytes());
    payload
}

fn build_wtp_wsp_connect_payload(transaction_id: u16, capabilities: &[u8]) -> Vec<u8> {
    assert!(
        capabilities.len() < 128,
        "test WSP Connect capabilities should fit one uintvar octet"
    );
    let mut payload = Vec::with_capacity(7 + capabilities.len());
    payload.push(0x0a);
    payload.extend_from_slice(&(transaction_id & 0x7fff).to_be_bytes());
    payload.push(0x12);
    payload.push(0x01);
    payload.push(0x10);
    payload.push(capabilities.len() as u8);
    payload.push(0x00);
    payload.extend_from_slice(capabilities);
    payload
}

fn enable_wap_ip_status_mvp(config: &mut tetra_config::bluestation::StackConfig) {
    mark_wap_status_health_ok_for_test();
    config.cell.sndcp_service = true;
    config.cell.wap_ip = Some(CfgWapIp {
        enabled: true,
        address: [10, 0, 0, 1],
        port: 9200,
        response_ttl: 32,
        dynamic_pool_prefix: [10, 0, 0],
        dynamic_pool_first_host: 2,
        dynamic_pool_last_host: 254,
        allow_static_ipv4: true,
        accept_empty_probe: true,
        accept_root_path: true,
        accept_status_path: true,
        accept_status_wml_path: true,
        max_request_payload_bytes: DEFAULT_WAP_IP_MAX_REQUEST_PAYLOAD_BYTES,
        assume_pdch_ready_after_data_transmit: true,
    });
}

fn mark_wap_status_health_ok_for_test() {
    let registry = tetra_entities::health::registry();
    registry.mark_router_tick(0, Duration::from_millis(1));
    registry.set_brew_status(true, 1);
}

fn submit_voice_subscriber_update(test: &mut ComponentTest, issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi, groups, action }),
    });
}

fn register_voice_subscriber(test: &mut ComponentTest, issi: u32, gssi: u32) {
    submit_voice_subscriber_update(test, issi, Vec::new(), BrewSubscriberAction::Register);
    test.run_stack(Some(1));
    submit_voice_subscriber_update(test, issi, vec![gssi], BrewSubscriberAction::Affiliate);
    test.run_stack(Some(1));
}

fn build_group_u_setup_msg(calling_issi: u32, dest_gssi: u32) -> SapMsg {
    let u_setup = USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2Mp,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(dest_gssi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_setup.to_bitbuf(&mut sdu).expect("test group U-SETUP should serialize");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::issi(calling_issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_group_u_disconnect_msg(calling_issi: u32, call_id: u16) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(32);
    UDisconnect {
        call_identifier: call_id,
        disconnect_cause: DisconnectCause::UserRequestedDisconnection,
        facility: None,
        proprietary: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("test U-DISCONNECT should serialize");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 2,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::issi(calling_issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn cmce_bs_mut(test: &mut ComponentTest) -> &mut CmceBs {
    test.router
        .get_entity(TetraEntity::Cmce)
        .expect("CMCE entity should be registered")
        .as_any_mut()
        .downcast_mut::<CmceBs>()
        .expect("registered CMCE entity should be CmceBs")
}

fn cmce_debug_active_call_ids(test: &mut ComponentTest) -> Vec<u16> {
    cmce_bs_mut(test).debug_active_call_ids()
}

fn umac_bs_mut(test: &mut ComponentTest) -> &mut UmacBs {
    test.router
        .get_entity(TetraEntity::Umac)
        .expect("UMAC entity should be registered")
        .as_any_mut()
        .downcast_mut::<UmacBs>()
        .expect("registered UMAC entity should be UmacBs")
}

fn submit_wap_packet_data_sequence(test: &mut ComponentTest) {
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_wap_status_sn_unitdata(2)));
}

fn submit_wap_packet_data_sequence_through_mle(test: &mut ComponentTest) {
    test.submit_message(build_tla_sndcp_unitdata_ind(build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_tla_sndcp_unitdata_ind(
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_tla_sndcp_unitdata_ind(build_wap_status_sn_unitdata(2)));
}

fn build_mxp600_single_slot_data_transmit_request(nsapi: u8) -> BitBuffer {
    encode_data_transmit_request(&SndcpDataTransmitRequest {
        nsapi,
        logical_link_status: false,
        resource_request: SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 1,
            downlink_timeslots: 1,
            full_phase_modulation_capability_timeslots: 1,
            unspecified_phase_modulation_resource: false,
        }),
    })
    .expect("MXP600-style SN-DATA TRANSMIT REQUEST should encode")
}

fn build_mxp600_unspecified_four_slot_data_transmit_request(nsapi: u8) -> BitBuffer {
    encode_data_transmit_request(&SndcpDataTransmitRequest {
        nsapi,
        logical_link_status: false,
        resource_request: SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: true,
        }),
    })
    .expect("MXP600-style unspecified SN-DATA TRANSMIT REQUEST should encode")
}

fn build_mxp600_specific_four_slot_data_transmit_request(nsapi: u8) -> BitBuffer {
    encode_data_transmit_request(&SndcpDataTransmitRequest {
        nsapi,
        logical_link_status: false,
        resource_request: SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: false,
        }),
    })
    .expect("MXP600-style specific four-slot SN-DATA TRANSMIT REQUEST should encode")
}

fn build_mxp600_specific_four_slot_reconnect(nsapi: u8) -> BitBuffer {
    encode_reconnect(&SndcpReconnect {
        nsapi: Some(nsapi),
        resource_request: SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: false,
        }),
    })
    .expect("MXP600-style specific four-slot SN-RECONNECT should encode")
}

fn assert_default_dynamic_pdch_channel_allocation(allocation: &CmceChanAllocReq) {
    assert_single_slot_pdch_channel_allocation(allocation);
}

fn assert_single_slot_fallback_pdch_channel_allocation(allocation: &CmceChanAllocReq) {
    assert_eq!(allocation.usage, None);
    assert_eq!(allocation.carrier, None);
    assert_eq!(allocation.timeslots, [false, true, false, false]);
    assert!(
        !allocation.timeslots[0],
        "single-slot SNDCP PDCH fallback allocation must not include MCCH TS1"
    );
    assert!(
        !allocation.timeslots[2] && !allocation.timeslots[3],
        "single-slot SNDCP PDCH fallback must not allocate parallel TS3/TS4"
    );
    assert_eq!(allocation.alloc_type, ChanAllocType::Replace);
    assert_eq!(allocation.ul_dl_assigned, UlDlAssignment::Both);
}

fn assert_single_slot_pdch_channel_allocation(allocation: &CmceChanAllocReq) {
    assert_eq!(allocation.usage, None);
    assert_eq!(allocation.carrier, None);
    assert_eq!(allocation.timeslots, [false, true, false, false]);
    assert!(
        !allocation.timeslots[0],
        "single-slot SNDCP PDCH allocation must not include MCCH TS1"
    );
    assert!(
        !allocation.timeslots[2] && !allocation.timeslots[3],
        "single-slot phase-modulation request must not receive TS3/TS4"
    );
    assert_eq!(allocation.alloc_type, ChanAllocType::Replace);
    assert_eq!(allocation.ul_dl_assigned, UlDlAssignment::Both);
}

fn assert_quit_and_go_common_control_allocation(allocation: &CmceChanAllocReq) {
    assert_eq!(allocation.usage, None);
    assert_eq!(allocation.carrier, None);
    assert_eq!(allocation.timeslots, [false, false, false, false]);
    assert_eq!(allocation.alloc_type, ChanAllocType::QuitAndGo);
    assert_eq!(allocation.ul_dl_assigned, UlDlAssignment::Both);
}

fn assert_no_runtime_side_effects(test: &mut ComponentTest) {
    assert_eq!(test.router.get_msgqueue_len(), 0);
    assert!(test.dump_sinks().is_empty());
}

fn take_ltpd_unitdata_reqs(test: &mut ComponentTest) -> Vec<LtpdMleUnitdataReq> {
    test.dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect()
}

fn take_ltpd_configure_reqs(test: &mut ComponentTest) -> Vec<LtpdMleConfigureReq> {
    test.dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleConfigureReq(req) => Some(req),
            _ => None,
        })
        .collect()
}

fn build_subscriber_deregister(issi: u32) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: Vec::new(),
            action: BrewSubscriberAction::Deregister,
        }),
    }
}

#[test]
fn sndcp_encode_sn_unitdata_no_compression_round_trips_decoder() {
    // EN 300 392-2 clause 28.4.4.14/table 28.43 defines SN-UNITDATA as
    // SN PDU type, NSAPI, PCOMP, DCOMP and a lower-layer-length N-PDU with no
    // trailing O-bit. Keep the outbound helper aligned with the inbound
    // decoder before advertising a packet-data/WAP bearer.
    let mut n_pdu = BitBuffer::new(32);
    for byte in [0x45, 0x00, 0x00, 0x14] {
        n_pdu.write_bits(byte, 8);
    }
    n_pdu.seek(0);

    let encoded = encode_sn_unitdata(3, 0, 0, &n_pdu).expect("SN-UNITDATA encode should succeed");

    let SndcpDecode::Unitdata(unitdata) = decode_ltpd_sdu(&encoded) else {
        panic!("expected encoded SN-UNITDATA to decode");
    };

    assert_eq!(unitdata.nsapi, 3);
    assert_eq!(unitdata.pcomp, 0);
    assert_eq!(unitdata.dcomp, 0);
    assert_eq!(unitdata.network_pdu_kind, NetworkPduKind::Ipv4);
    assert_eq!(unitdata.n_pdu.to_bitstr(), n_pdu.to_bitstr());
}

#[test]
fn sndcp_decode_activate_pdp_context_demand_dynamic_ipv4() {
    // EN 300 392-2 clause 28.3.3.5 requires PDP context activation before
    // packet data transfer. The BS-side decoder can recognize the activation
    // demand, while runtime handling remains fail-closed until the full bearer
    // is implemented.
    let sdu = build_dynamic_ipv4_activation_demand(2);

    let SndcpDecode::ActivatePdpContextDemand(demand) = decode_ltpd_sdu(&sdu) else {
        panic!("expected decoded SN-ACTIVATE PDP CONTEXT DEMAND");
    };

    assert_eq!(demand.nsapi, 2);
    assert_eq!(demand.sndcp_version, 1);
    assert_eq!(demand.address, SndcpActivateAddressDemand::Ipv4Dynamic);
    assert_eq!(demand.packet_data_ms_type, SnPacketDataMsType::TypeAParallel);
}

#[test]
fn sndcp_decode_ltpd_sdu_accepts_mle_demux_cursor() {
    // EN 300 392-2 clause 18.4.1.3 routes service PDUs by a 3-bit MLE
    // protocol discriminator. The MLE entity consumes that discriminator
    // before delivering the SNDCP SDU on LTPD-SAP, so SNDCP decoding must be
    // relative to the current BitBuffer cursor.
    let sndcp_sdu = build_dynamic_ipv4_activation_demand(2);
    let mut tl_sdu = BitBuffer::new(3 + sndcp_sdu.get_len());
    tl_sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
    let mut sndcp_sdu = BitBuffer::from_bitbuffer(&sndcp_sdu);
    sndcp_sdu.seek(0);
    let sndcp_len = sndcp_sdu.get_len();
    tl_sdu.copy_bits(&mut sndcp_sdu, sndcp_len);
    tl_sdu.seek(0);
    assert_eq!(tl_sdu.read_bits(3), Some(MleProtocolDiscriminator::Sndcp.into_raw() as u64));

    let SndcpDecode::ActivatePdpContextDemand(demand) = decode_ltpd_sdu(&tl_sdu) else {
        panic!("expected cursor-relative SN-ACTIVATE PDP CONTEXT DEMAND");
    };

    assert_eq!(demand.nsapi, 2);
}

#[test]
fn sndcp_decode_deactivate_pdp_context_demand_single_nsapi() {
    let deactivation = SndcpDeactivation::Nsapi(2);
    let sdu = encode_deactivate_pdp_context_demand(&deactivation).expect("deactivation demand should encode");

    let SndcpDecode::DeactivatePdpContextDemand(decoded) = decode_ltpd_sdu(&sdu) else {
        panic!("expected decoded SN-DEACTIVATE PDP CONTEXT DEMAND");
    };

    assert_eq!(decoded, deactivation);
    assert_eq!(decode_deactivate_pdp_context_demand(&sdu), Ok(deactivation));
}

#[test]
fn sndcp_encode_sn_unitdata_rejects_unsupported_fields() {
    let mut n_pdu = BitBuffer::new(8);
    n_pdu.write_bits(0x45, 8);
    n_pdu.seek(0);

    assert_encode_error(encode_sn_unitdata(0, 0, 0, &n_pdu), SndcpEncodeError::UnsupportedNsapi(0));
    assert_encode_error(encode_sn_unitdata(15, 0, 0, &n_pdu), SndcpEncodeError::UnsupportedNsapi(15));
    assert_encode_error(
        encode_sn_unitdata(3, 1, 0, &n_pdu),
        SndcpEncodeError::UnsupportedCompression { pcomp: 1, dcomp: 0 },
    );
    assert_encode_error(encode_sn_unitdata(3, 0, 0, &BitBuffer::new(0)), SndcpEncodeError::EmptyNPdu);
}

fn assert_encode_error(result: Result<BitBuffer, SndcpEncodeError>, expected: SndcpEncodeError) {
    match result {
        Err(err) => assert_eq!(err, expected),
        Ok(_) => panic!("expected encode error {expected:?}"),
    }
}

#[test]
fn sndcp_runtime_handoff_policy_is_separate_and_disabled_by_default() {
    // EN 300 392-2 table 18.26 service advertising is not enough to make a
    // packet-data bearer safe. Runtime WAP/IP handoff needs a separate gate so
    // future PDP/SN-SAP/PDCH plumbing cannot emit traffic by accident.
    let policy = SndcpRuntimeHandoffPolicy::default();

    let activation = decode_ltpd_sdu(&build_dynamic_ipv4_activation_demand(2));
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(false, &activation),
        SndcpRuntimeHandoffDecision::DropServiceUnavailable
    );
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(true, &activation),
        SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled {
            pdu: SndcpRuntimePduClass::PdpActivationDemand
        }
    );

    let ready = decode_ltpd_sdu(
        &encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    );
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(true, &ready),
        SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled {
            pdu: SndcpRuntimePduClass::TransferControl
        }
    );

    let unitdata = decode_ltpd_sdu(&build_wap_status_sn_unitdata(2));
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(true, &unitdata),
        SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled {
            pdu: SndcpRuntimePduClass::Unitdata
        }
    );
}

#[test]
fn sndcp_ltpd_wap_packet_data_sequence_drops_without_service_advertising() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(
        vec![TetraEntity::Sndcp],
        vec![
            TetraEntity::Mle,
            TetraEntity::Llc,
            TetraEntity::Cmce,
            TetraEntity::User,
            TetraEntity::Brew,
        ],
    );

    // EN 300 392-2 clauses 17.3.5, 28.3.3.5, 28.4.4.5 and 28.4.4.14
    // describe the LTPD/SNDCP activation, READY request and SN-UNITDATA path.
    // With table 18.26 SNDCP service still unadvertised, the live runtime must
    // not synthesize PDP activation responses, WAP/IP output or lower-channel
    // allocation side effects.
    submit_wap_packet_data_sequence(&mut test);
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_ltpd_wap_packet_data_sequence_drops_even_with_direct_service_flag() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Sndcp],
        vec![
            TetraEntity::Mle,
            TetraEntity::Llc,
            TetraEntity::Cmce,
            TetraEntity::User,
            TetraEntity::Brew,
        ],
    );

    // A direct StackConfig can exercise decoders behind the parser/sysinfo
    // fail-closed gates, but runtime SNDCP still has no PDP table, SN-SAP,
    // WAP/IP handoff or MLE channel-allocation wiring.
    submit_wap_packet_data_sequence(&mut test);
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_wap_ip_mvp_answers_activation_ready_and_wml_unitdata_when_enabled() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clauses 28.3.3.5, 28.4.4.5 and 28.4.4.14: PDP
    // activation establishes the IPv4 context, SN-DATA TRANSMIT moves the
    // subscriber into READY, and SN-UNITDATA carries the WAP/IP N-PDU.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_wap_status_sn_unitdata_from(2, [10, 0, 0, 2], b"GET /status.xhtml HTTP/1.0\r\n\r\n"),
    ));
    test.deliver_all_messages();

    let mut ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 3);

    let accept = decode_activate_pdp_context_accept(&ltpd_reqs[0].sdu).expect("activation response should decode");
    assert_eq!(accept.nsapi, 2);
    assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
    assert_eq!(ltpd_reqs[0].layer2service, Layer2Service::Acknowledged);
    assert!(!ltpd_reqs[0].packet_data_flag);

    let ready = decode_data_transmit_response(&ltpd_reqs[1].sdu).expect("ready response should decode");
    assert_eq!(ready.nsapi, 2);
    assert_eq!(ready.result, SndcpDataTransmitResponseResult::Accepted);
    assert_eq!(ltpd_reqs[1].layer2service, Layer2Service::Acknowledged);
    assert!(!ltpd_reqs[1].packet_data_flag);
    let ready_alloc = ltpd_reqs[1]
        .chan_alloc
        .as_ref()
        .expect("accepted SN-DATA TRANSMIT RESPONSE should carry the MVP PDCH allocation");
    assert_default_dynamic_pdch_channel_allocation(ready_alloc);

    assert_eq!(ltpd_reqs[2].layer2service, Layer2Service::Acknowledged);
    assert!(ltpd_reqs[2].packet_data_flag);
    assert!(ltpd_reqs[2].chan_alloc.is_none());
    let unitdata = decode_sn_user_data_pdu(&ltpd_reqs.remove(2).sdu).expect("WAP response SN user data should decode");
    let response_octets = bitbuffer_npdu_octets(&unitdata.n_pdu).expect("response N-PDU should be byte aligned");
    let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
    let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
    assert_eq!(response_ip.source, [10, 0, 0, 1]);
    assert_eq!(response_ip.destination, [10, 0, 0, 2]);
    assert_eq!(response_udp.source_port, 9200);
    assert_eq!(response_udp.destination_port, 49_152);
    assert!(
        response_octets.len() <= 576,
        "WAP status SN-UNITDATA response should fit the single-slot-safe SNDCP MTU: {} bytes",
        response_octets.len()
    );
    assert!(
        response_udp.payload.len() <= DEFAULT_WAP_WSP_STATUS_MAX_BYTES,
        "raw UDP /status.xhtml should use the terminal-safe tiny XHTML budget: {} bytes",
        response_udp.payload.len()
    );
    let page = std::str::from_utf8(response_udp.payload).unwrap();
    assert!(page.contains("http://www.w3.org/1999/xhtml"));
    assert!(page.contains("<body>"));
    assert!(!page.contains("text=\"#0f0\""));
    assert!(page.contains("Nexus-BS: OK"), "page={page:?}");
    assert!(page.contains("Version:"));
    assert!(page.contains("Uptime"));
    assert!(!page.contains("Voice"));
    assert!((2..=3).contains(&page.matches("<br />").count()));
    assert!(!page.contains("<br/>"));
    assert!(!page.contains("<wml"));
    assert!(!page.contains("<card"));
}

#[test]
fn sndcp_wap_al_xhtml_e2e_waits_for_pdch_report_and_responds_over_al() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = false;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Sndcp, TetraEntity::Mle, TetraEntity::Llc],
        vec![TetraEntity::Umac],
    );

    let addr = TetraAddress::new(1000001, SsiType::Issi);
    let endpoint_id = 1;

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand(2),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert!(
        !activation_msgs.is_empty(),
        "activation should reach UMAC before data transmit: {activation_msgs:#?}"
    );
    let activation_ns = activation_msgs
        .iter()
        .find_map(bl_data_ns_from_tma_req)
        .expect("activation accept should be sent as BL-DATA");
    test.submit_message(build_bl_ack_ind(addr, endpoint_id, activation_ns));
    test.deliver_all_messages();
    let _activation_ack_drain = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    let ready_req_handle = ready_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(req) if req.chan_alloc.is_some() => {
                assert_eq!(req.endpoint_id, endpoint_id);
                assert_default_dynamic_pdch_channel_allocation(req.chan_alloc.as_ref().unwrap());
                Some(req.req_handle)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("SN-DATA TRANSMIT RESPONSE should reach UMAC with PDCH allocation: {ready_msgs:#?}"));

    test.submit_message(build_tma_report(ready_req_handle, TmaReport::SuccessReservedOrStealing));
    test.deliver_all_messages();
    let _report_drain = test.dump_sinks();

    test.submit_message(build_al_setup_ind(addr, endpoint_id, 0));
    test.deliver_all_messages();
    let setup_msgs = test.dump_sinks();
    assert!(
        setup_msgs
            .iter()
            .any(|msg| llc_pdu_type_from_tma_req(msg) == Some(LlcPduType::AlSetup)),
        "AL-SETUP should be accepted and answered before SN-DATA flows"
    );

    let request_payload = build_wtp_wsp_get_payload(0x1234, "/status.xhtml");
    let request_sndcp = build_wap_status_sn_data_from(2, [10, 0, 0, 2], &request_payload);
    let request_tl_sdu = build_mle_prefixed_sndcp_sdu(request_sndcp);
    test.submit_message(build_al_final_ar_ind(addr, endpoint_id, 0, &request_tl_sdu));
    test.run_stack(Some(1));
    let response_msgs = test.dump_sinks();

    let al_response_segments: Vec<&SapMsg> = response_msgs
        .iter()
        .filter(|msg| llc_pdu_type_from_tma_req(msg) == Some(LlcPduType::AlDataAlFinal))
        .collect();
    assert!(
        al_response_segments.len() > 1,
        "default WSP XHTML should be delivered as segmented AL-DATA/AL-FINAL-AR"
    );
    let response =
        sndcp_user_data_from_al_tma_reqs(&response_msgs).expect("WAP GET over AL SN-DATA should produce a segmented SNDCP response");
    let response_octets = bitbuffer_npdu_octets(&response.n_pdu).expect("response N-PDU should be byte aligned");
    let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
    let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
    assert_eq!(
        &response_udp.payload[..7],
        &[0x12, 0x92, 0x34, 0x04, 0x20, 0x01, 0xc5],
        "WSP GET should receive WTP Result + WSP Reply 200 application/vnd.wap.xhtml+xml"
    );
    let page = std::str::from_utf8(&response_udp.payload[7..]).expect("XHTML response should be UTF-8");

    assert_eq!(response_ip.source, [10, 0, 0, 1]);
    assert_eq!(response_ip.destination, [10, 0, 0, 2]);
    assert_eq!(response_udp.source_port, 9200);
    assert_eq!(response_udp.destination_port, 49_152);
    assert!(
        response_udp.payload.len() <= DEFAULT_WAP_WSP_STATUS_MAX_BYTES + 7,
        "WSP GET response should remain inside the negotiated WAP status budget: {} bytes",
        response_udp.payload.len()
    );
    assert!(
        response_octets.len() <= 576,
        "single-slot WAP/IP response N-PDU should fit the negotiated 576-octet SNDCP MTU: {} bytes",
        response_octets.len()
    );
    assert!(page.contains("http://www.w3.org/1999/xhtml"));
    assert!(page.contains("<body>"));
    assert!(!page.contains("text=\"#0f0\""));
    assert!(page.contains("Nexus-BS: OK"), "page={page:?}");
    assert!(page.contains("Version:"));
    assert!(page.contains("Uptime"));
    assert!(!page.contains("Voice"));
    assert!((2..=3).contains(&page.matches("<br />").count()));
    assert!(!page.contains("<br/>"));
    assert!(!page.contains("<wml"));
    assert!(!page.contains("<card"));
}

#[test]
fn sndcp_wap_al_connect_reply_e2e_acknowledges_segmented_response() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = false;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Sndcp, TetraEntity::Mle, TetraEntity::Llc],
        vec![TetraEntity::Umac],
    );

    let addr = TetraAddress::new(1000001, SsiType::Issi);
    let endpoint_id = 1;

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand(2),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    let activation_ns = activation_msgs
        .iter()
        .find_map(bl_data_ns_from_tma_req)
        .expect("activation accept should be sent as BL-DATA");
    test.submit_message(build_bl_ack_ind(addr, endpoint_id, activation_ns));
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    let ready_req_handle = ready_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(req) if req.chan_alloc.is_some() => Some(req.req_handle),
            _ => None,
        })
        .unwrap_or_else(|| panic!("SN-DATA TRANSMIT RESPONSE should reach UMAC with PDCH allocation: {ready_msgs:#?}"));
    test.submit_message(build_tma_report(ready_req_handle, TmaReport::SuccessReservedOrStealing));
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_setup_ind(addr, endpoint_id, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    let capabilities = [
        0x03, 0x80, 0x8a, 0x78, 0x03, 0x81, 0x8a, 0x78, 0x02, 0x82, 0xf0, 0x02, 0x83, 0x03, 0x09, 0x86, 0x10, b'x', b'-', b'u', b'p', b'-',
        b'1', 0x00,
    ];
    let request_payload = build_wtp_wsp_connect_payload(0x1234, &capabilities);
    let request_sndcp = build_wap_status_sn_data_from(2, [10, 0, 0, 2], &request_payload);
    let request_tl_sdu = build_mle_prefixed_sndcp_sdu(request_sndcp);
    test.submit_message(build_al_final_ar_ind(addr, endpoint_id, 0, &request_tl_sdu));
    test.run_stack(Some(1));
    let mut response_msgs = test.dump_sinks();

    let al_response_segments: Vec<&SapMsg> = response_msgs
        .iter()
        .filter(|msg| llc_pdu_type_from_tma_req(msg) == Some(LlcPduType::AlDataAlFinal))
        .collect();
    assert!(
        al_response_segments.len() > 1,
        "WSP ConnectReply should be delivered as segmented AL-DATA/AL-FINAL-AR"
    );
    assert!(
        al_response_segments.iter().all(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(req) => req.endpoint_id == endpoint_id,
            _ => false,
        }),
        "ConnectReply AL segments must preserve the packet-data endpoint"
    );
    let response_ns = al_response_segments
        .iter()
        .find_map(|msg| al_data_ns_from_tma_req(msg))
        .expect("ConnectReply AL response should have N(S)");
    let response =
        sndcp_user_data_from_al_tma_reqs(&response_msgs).expect("WAP Connect over AL SN-DATA should produce a segmented SNDCP response");
    let response_octets = bitbuffer_npdu_octets(&response.n_pdu).expect("response N-PDU should be byte aligned");
    let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
    let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
    assert_eq!(response_ip.source, [10, 0, 0, 1]);
    assert_eq!(response_ip.destination, [10, 0, 0, 2]);
    assert_eq!(response_udp.source_port, 9200);
    assert_eq!(response_udp.destination_port, 49_152);
    assert_eq!(
        &response_udp.payload[..4],
        &[0x12, 0x92, 0x34, 0x02],
        "WSP Connect should receive WTP Result + WSP ConnectReply"
    );
    assert_eq!(
        &response_udp.payload[5..],
        &[0x08, 0x00, 0x03, 0x80, 0x84, 0x21, 0x03, 0x81, 0x84, 0x21],
        "ConnectReply should keep WSP negotiation to bounded SDU sizes"
    );

    for handle in response_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report(handle, TmaReport::SuccessReservedOrStealing));
    }
    response_msgs.clear();
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_ack_ind(addr, endpoint_id, response_ns));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        complete_msgs
            .iter()
            .all(|msg| llc_pdu_type_from_tma_req(msg) != Some(LlcPduType::AlDataAlFinal)),
        "complete peer AL-ACK should not emit more ConnectReply segments"
    );

    test.run_stack(Some(20));
    let post_ack_msgs = test.dump_sinks();
    assert!(
        post_ack_msgs
            .iter()
            .all(|msg| llc_pdu_type_from_tma_req(msg) != Some(LlcPduType::AlDataAlFinal)),
        "completed ConnectReply must not be retried after scheduler ticks"
    );
}

#[test]
fn sndcp_wap_al_udp_wsp_xhtml_e2e_waits_for_pdch_report_and_responds_over_al() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = false;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Sndcp, TetraEntity::Mle, TetraEntity::Llc],
        vec![TetraEntity::Umac],
    );

    let addr = TetraAddress::new(1000001, SsiType::Issi);
    let endpoint_id = 1;
    let client_ip = [10, 0, 0, 2];

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand(2),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    let activation_ns = activation_msgs
        .iter()
        .find_map(bl_data_ns_from_tma_req)
        .expect("activation accept should be sent as BL-DATA");
    test.submit_message(build_bl_ack_ind(addr, endpoint_id, activation_ns));
    test.deliver_all_messages();
    let _activation_ack_drain = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    let ready_req_handle = ready_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(req) if req.chan_alloc.is_some() => Some(req.req_handle),
            _ => None,
        })
        .unwrap_or_else(|| panic!("SN-DATA TRANSMIT RESPONSE should carry PDCH allocation: {ready_msgs:#?}"));

    test.submit_message(build_tma_report(ready_req_handle, TmaReport::SuccessReservedOrStealing));
    test.deliver_all_messages();
    let _report_drain = test.dump_sinks();

    test.submit_message(build_al_setup_ind(addr, endpoint_id, 0));
    test.deliver_all_messages();
    let setup_msgs = test.dump_sinks();
    assert!(
        setup_msgs
            .iter()
            .any(|msg| llc_pdu_type_from_tma_req(msg) == Some(LlcPduType::AlSetup)),
        "AL-SETUP should be accepted before WAP/UDP browser traffic flows"
    );

    let get_payload = build_wtp_wsp_get_payload(0x1235, "/status.xhtml");
    let get_sndcp = build_wap_status_sn_data_from(2, client_ip, &get_payload);
    let get_tl_sdu = build_mle_prefixed_sndcp_sdu(get_sndcp);
    test.submit_message(build_al_final_ar_ind(addr, endpoint_id, 0, &get_tl_sdu));
    test.run_stack(Some(1));
    let response_msgs = test.dump_sinks();
    let response = sndcp_user_data_from_al_tma_reqs(&response_msgs).expect("WTP/WSP GET over AL SN-DATA should produce a SNDCP response");
    let response_octets = bitbuffer_npdu_octets(&response.n_pdu).expect("response N-PDU should be byte aligned");
    let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
    let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
    let response_payload = response_udp.payload;
    let page = std::str::from_utf8(&response_payload[7..]).expect("WSP XHTML response should be UTF-8");

    assert_eq!(response_ip.source, [10, 0, 0, 1]);
    assert_eq!(response_ip.destination, client_ip);
    assert_eq!(response_udp.source_port, 9200);
    assert_eq!(response_udp.destination_port, 49_152);
    assert_eq!(
        &response_payload[..7],
        &[0x12, 0x92, 0x35, 0x04, 0x20, 0x01, 0xc5],
        "WSP GET should receive WTP Result + WSP Reply(application/vnd.wap.xhtml+xml)"
    );
    assert!(page.contains("http://www.w3.org/1999/xhtml"));
    assert!(page.contains("<body>"));
    assert!(!page.contains("text=\"#0f0\""));
    assert!(page.contains("Nexus-BS: OK"), "page={page:?}");
    assert!(page.contains("Version:"));
    assert!(page.contains("Uptime"));
    assert!(!page.contains("Voice"));
    assert!((2..=3).contains(&page.matches("<br />").count()));
    assert!(!page.contains("<br/>"));
    assert!(!page.contains("<wml"));
    assert!(!page.contains("<card"));
}

#[test]
fn sndcp_created_pdch_on_ts2_does_not_block_group_voice_on_free_slot_or_reuse() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config.cell.advanced_link = true;
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = false;

    let data_issi = 1000001;
    let voice_gssi = 91;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(
        vec![
            TetraEntity::Sndcp,
            TetraEntity::Mle,
            TetraEntity::Llc,
            TetraEntity::Umac,
            TetraEntity::Cmce,
        ],
        vec![TetraEntity::Lmac],
    );
    register_voice_subscriber(&mut test, data_issi, voice_gssi);
    let _ = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2), 1, 0));
    test.run_stack(Some(6));
    let _ = test.dump_sinks();
    test.submit_message(build_bl_ack_ind(TetraAddress::issi(data_issi), 1, 0));
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
        1,
        0,
    ));
    for _ in 0..64 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        let state = test.config.state_read();
        if state.timeslot_alloc.owner(2) == Some(TimeslotOwner::PacketData) {
            break;
        }
    }

    {
        let state = test.config.state_read();
        assert_eq!(state.timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(state.timeslot_alloc.owner(3), None);
        assert_eq!(state.timeslot_alloc.owner(4), None);
    }

    // Do not deliver a TMA report/end-of-data back to SNDCP. This models a
    // stuck SNDCP/WAP bearer while preserving the real lower-stack PDCH state
    // created through SNDCP -> MLE -> LLC -> UMAC.
    test.submit_message(build_group_u_setup_msg(data_issi, voice_gssi));
    test.run_stack(Some(2));
    let _ = test.dump_sinks();
    let call_ids = cmce_debug_active_call_ids(&mut test);
    let call_id = *call_ids
        .first()
        .unwrap_or_else(|| panic!("group voice call should be active after setup: {call_ids:?}"));

    {
        let state = test.config.state_read();
        assert_eq!(
            state.timeslot_alloc.owner(2),
            Some(TimeslotOwner::PacketData),
            "one-slot group voice should not preempt TS2 packet data while another traffic slot is free"
        );
        assert_eq!(
            state.timeslot_alloc.owner(3),
            Some(TimeslotOwner::Cmce),
            "one-slot group voice should use the next free traffic slot while TS2 carries packet data"
        );
        assert_eq!(
            state.timeslot_alloc.owner(4),
            None,
            "TS4 should remain free after TS2 WAP PDCH plus one-slot voice"
        );
    }
    {
        let umac = umac_bs_mut(&mut test);
        assert!(
            umac.channel_scheduler.circuit_is_active(Direction::Dl, 3),
            "UMAC must open downlink voice on the free slot"
        );
        assert!(
            umac.channel_scheduler.circuit_is_active(Direction::Ul, 3),
            "UMAC must open uplink voice on the free slot"
        );
    }

    test.run_stack(Some(6));
    {
        let state = test.config.state_read();
        assert_eq!(
            state.timeslot_alloc.owner(2),
            Some(TimeslotOwner::PacketData),
            "stuck SNDCP must remain confined to TS2"
        );
        assert_eq!(
            state.timeslot_alloc.owner(3),
            Some(TimeslotOwner::Cmce),
            "stuck SNDCP must not steal back an active voice slot"
        );
        assert_eq!(state.timeslot_alloc.owner(4), None);
    }

    test.submit_message(build_group_u_disconnect_msg(data_issi, call_id));
    for _ in 0..64 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        let state = test.config.state_read();
        if state.timeslot_alloc.owner(3).is_none() {
            break;
        }
    }
    {
        let state = test.config.state_read();
        assert_eq!(
            state.timeslot_alloc.owner(2),
            Some(TimeslotOwner::PacketData),
            "voice release must preserve TS2 packet data"
        );
        assert_eq!(state.timeslot_alloc.owner(3), None, "voice release must free TS3");
        assert_eq!(state.timeslot_alloc.owner(4), None, "voice release must not create TS4 packet data");
    }

    test.submit_message(build_group_u_setup_msg(data_issi, voice_gssi));
    test.run_stack(Some(2));
    let _ = test.dump_sinks();
    {
        let state = test.config.state_read();
        assert_eq!(
            state.timeslot_alloc.owner(2),
            Some(TimeslotOwner::PacketData),
            "next one-slot group voice must preserve TS2 packet data while another traffic slot is free"
        );
        assert_eq!(
            state.timeslot_alloc.owner(3),
            Some(TimeslotOwner::Cmce),
            "next one-slot group voice should reuse TS3 while TS2 is packet data"
        );
        assert_eq!(state.timeslot_alloc.owner(4), None);
    }
}

#[test]
fn sndcp_wap_ts2_pdch_is_preempted_for_voice_and_resumes_on_ts2_after_release() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config.cell.advanced_link = true;
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = false;

    let data_issi = 1000001;
    let voice_gssi = 91;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(
        vec![
            TetraEntity::Sndcp,
            TetraEntity::Mle,
            TetraEntity::Llc,
            TetraEntity::Umac,
            TetraEntity::Cmce,
        ],
        vec![TetraEntity::Lmac],
    );
    register_voice_subscriber(&mut test, data_issi, voice_gssi);
    let _ = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2), 1, 0));
    test.run_stack(Some(6));
    let _ = test.dump_sinks();
    test.submit_message(build_bl_ack_ind(TetraAddress::issi(data_issi), 1, 0));
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_mxp600_single_slot_data_transmit_request(2),
        1,
        0,
    ));
    for _ in 0..96 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        if test.config.state_read().timeslot_alloc.owner(2) == Some(TimeslotOwner::PacketData) {
            break;
        }
    }
    {
        let mut state = test.config.state_write();
        assert_eq!(state.timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        state
            .timeslot_alloc
            .reserve(TimeslotOwner::Brew, 3)
            .expect("test setup occupies TS3 so voice must reclaim TS2 packet data");
        state
            .timeslot_alloc
            .reserve(TimeslotOwner::Brew, 4)
            .expect("test setup occupies TS4 so voice must reclaim TS2 packet data");
    }

    test.submit_message(build_group_u_setup_msg(data_issi, voice_gssi));
    for _ in 0..96 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        if test.config.state_read().timeslot_alloc.owner(2) == Some(TimeslotOwner::Cmce) {
            break;
        }
    }
    let call_id = *cmce_debug_active_call_ids(&mut test)
        .first()
        .expect("group voice call should be active after TS2 preemption");
    {
        let state = test.config.state_read();
        assert_eq!(state.timeslot_alloc.owner(2), Some(TimeslotOwner::Cmce));
        assert_eq!(state.timeslot_alloc.owner(3), Some(TimeslotOwner::Brew));
        assert_eq!(state.timeslot_alloc.owner(4), Some(TimeslotOwner::Brew));
    }

    test.submit_message(build_group_u_disconnect_msg(data_issi, call_id));
    for _ in 0..128 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        if test.config.state_read().timeslot_alloc.owner(2).is_none() {
            break;
        }
    }
    assert_eq!(
        test.config.state_read().timeslot_alloc.owner(2),
        None,
        "voice release must free TS2 for packet-data resume"
    );

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_mxp600_single_slot_data_transmit_request(2),
        1,
        0,
    ));
    for _ in 0..128 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        if test.config.state_read().timeslot_alloc.owner(2) == Some(TimeslotOwner::PacketData) {
            break;
        }
    }
    let state = test.config.state_read();
    assert_eq!(
        state.timeslot_alloc.owner(2),
        Some(TimeslotOwner::PacketData),
        "SNDCP/WAP reload must resume on TS2 after voice releases it"
    );
    assert_eq!(state.timeslot_alloc.owner(3), Some(TimeslotOwner::Brew));
    assert_eq!(state.timeslot_alloc.owner(4), Some(TimeslotOwner::Brew));
}

#[test]
fn sndcp_wap_reload_with_ts2_voice_busy_rejects_without_fallback_packet_data() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config.cell.advanced_link = true;
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = false;

    let data_issi = 1000001;
    let endpoint_id = 1;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(
        vec![TetraEntity::Sndcp, TetraEntity::Mle, TetraEntity::Llc, TetraEntity::Umac],
        vec![TetraEntity::Lmac],
    );
    test.config
        .state_write()
        .timeslot_alloc
        .reserve(TimeslotOwner::Cmce, 2)
        .expect("test voice owner should reserve TS2 before SNDCP reload");

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand(2),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(6));
    let _ = test.dump_sinks();
    test.submit_message(build_bl_ack_ind(TetraAddress::issi(data_issi), endpoint_id, 0));
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_mxp600_single_slot_data_transmit_request(2),
        endpoint_id,
        0,
    ));
    for _ in 0..96 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        if test.config.state_read().timeslot_alloc.owner(2) == Some(TimeslotOwner::PacketData) {
            break;
        }
    }

    let state = test.config.state_read();
    assert_eq!(
        state.timeslot_alloc.owner(2),
        Some(TimeslotOwner::Cmce),
        "SNDCP/WAP reload must not take the voice-owned TS2"
    );
    assert_eq!(
        state.timeslot_alloc.owner(3),
        None,
        "SNDCP/WAP reload must not fall back to TS3 when TS2 is voice-owned"
    );
    assert_eq!(
        state.timeslot_alloc.owner(4),
        None,
        "SNDCP/WAP reload must not fall back to TS4 when TS2 is voice-owned"
    );
}

#[test]
fn sndcp_wap_reload_with_ts4_voice_busy_still_allocates_ts2_packet_data() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config.cell.advanced_link = true;
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = false;

    let data_issi = 1000001;
    let endpoint_id = 1;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(
        vec![TetraEntity::Sndcp, TetraEntity::Mle, TetraEntity::Llc, TetraEntity::Umac],
        vec![TetraEntity::Lmac],
    );
    test.config
        .state_write()
        .timeslot_alloc
        .reserve(TimeslotOwner::Cmce, 4)
        .expect("test voice owner should reserve TS4 before SNDCP reload");

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand(2),
        endpoint_id,
        0,
    ));
    test.run_stack(Some(6));
    let _ = test.dump_sinks();
    test.submit_message(build_bl_ack_ind(TetraAddress::issi(data_issi), endpoint_id, 0));
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    test.submit_message(build_ltpd_ind_on_link(
        Sap::TlpdSap,
        build_mxp600_single_slot_data_transmit_request(2),
        endpoint_id,
        0,
    ));
    for _ in 0..96 {
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        if test.config.state_read().timeslot_alloc.owner(2) == Some(TimeslotOwner::PacketData) {
            break;
        }
    }

    let state = test.config.state_read();
    assert_eq!(
        state.timeslot_alloc.owner(4),
        Some(TimeslotOwner::Cmce),
        "SNDCP/WAP reload must not take voice-owned TS4"
    );
    assert_eq!(
        state.timeslot_alloc.owner(2),
        Some(TimeslotOwner::PacketData),
        "SNDCP/WAP reload should use TS2 when TS2 is free, even if TS4 is voice-owned"
    );
    assert_eq!(state.timeslot_alloc.owner(3), None);
}

#[test]
fn sndcp_wap_data_handoff_rejects_when_all_traffic_timeslots_are_voice_busy() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    {
        let mut state = test.config.state_write();
        for ts in 2..=4 {
            state
                .timeslot_alloc
                .reserve(TimeslotOwner::Cmce, ts)
                .expect("test voice owner should reserve every traffic TS");
        }
    }

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(2, SnPacketDataMsType::TypeBAlternating),
    ));
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_mxp600_single_slot_data_transmit_request(2)));
    test.deliver_all_messages();

    let mut ltpd_reqs = take_ltpd_unitdata_reqs(&mut test);
    assert_eq!(
        ltpd_reqs.len(),
        2,
        "activation plus fast reject should emit exactly two SNDCP responses"
    );
    let ready_response = ltpd_reqs.remove(1);
    let ready = decode_data_transmit_response(&ready_response.sdu).expect("SN-DATA TRANSMIT reject should decode");
    assert_eq!(ready.nsapi, 2);
    assert_eq!(
        ready.result,
        SndcpDataTransmitResponseResult::Rejected(SndcpTransferRejectCause::SndcpServiceTemporarilyNotAvailable)
    );
    assert!(!ready_response.packet_data_flag);
    assert!(
        ready_response.chan_alloc.is_none(),
        "all traffic slots busy by voice must fail fast without advertising a stale PDCH allocation"
    );
}

#[test]
fn sndcp_wap_ip_deregister_clears_stale_pdp_context_before_reactivation() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating),
    ));
    test.submit_message(build_subscriber_deregister(1000001));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating),
    ));
    test.deliver_all_messages();

    let ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 2);
    for req in &ltpd_reqs {
        let accept = decode_activate_pdp_context_accept(&req.sdu).expect("activation response should decode as accept");
        assert_eq!(accept.nsapi, 1);
    }
    assert_eq!(
        decode_activate_pdp_context_accept(&ltpd_reqs[1].sdu).unwrap().assigned_address,
        Some(SnAddress::Ipv4([10, 0, 0, 2]))
    );
}

#[test]
fn sndcp_wap_ip_mvp_accepts_mxp600_type_b_and_type_c_activation_when_enabled() {
    for packet_data_ms_type in [SnPacketDataMsType::TypeBAlternating, SnPacketDataMsType::TypeCIpSingleMode] {
        debug::setup_logging_verbose();
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        enable_wap_ip_status_mvp(&mut config);
        let mut test = ComponentTest::from_config(config, None);
        test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

        test.submit_message(build_ltpd_ind(
            Sap::TlpdSap,
            build_dynamic_ipv4_activation_demand_with_ms_type(1, packet_data_ms_type),
        ));
        test.deliver_all_messages();

        let ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
            .dump_sinks()
            .into_iter()
            .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
            .filter_map(|msg| match msg.msg {
                SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
                _ => None,
            })
            .collect();
        assert_eq!(ltpd_reqs.len(), 1, "unexpected response count for {packet_data_ms_type:?}");

        let accept = decode_activate_pdp_context_accept(&ltpd_reqs[0].sdu).expect("activation response should decode as accept");
        assert_eq!(accept.nsapi, 1);
        assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
        assert_eq!(ltpd_reqs[0].layer2service, Layer2Service::Acknowledged);
        assert!(!ltpd_reqs[0].packet_data_flag);
    }
}

#[test]
fn sndcp_wap_ip_mvp_accepts_mxp600_type_b_single_slot_data_transmit_request() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(2, SnPacketDataMsType::TypeBAlternating),
    ));
    let request = build_mxp600_single_slot_data_transmit_request(2);
    assert_eq!(request.get_len(), 21);
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, request));
    test.deliver_all_messages();

    let mut ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 2);
    let ready_response = ltpd_reqs.remove(1);
    let ready = decode_data_transmit_response(&ready_response.sdu)
        .expect("SN-DATA TRANSMIT RESPONSE should decode after single-slot resource request");
    assert_eq!(ready.nsapi, 2);
    assert_eq!(ready.result, SndcpDataTransmitResponseResult::Accepted);
    let allocation = ready_response
        .chan_alloc
        .as_ref()
        .expect("accepted single-slot SN-DATA TRANSMIT RESPONSE should carry PDCH allocation");
    assert_single_slot_pdch_channel_allocation(allocation);
}

#[test]
fn sndcp_wap_ip_mvp_accepts_mxp600_type_b_unspecified_four_slot_resource_request() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating),
    ));
    let request = build_mxp600_unspecified_four_slot_data_transmit_request(1);
    assert_eq!(request.get_len(), 21);
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, request));
    test.deliver_all_messages();

    let mut ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 2);
    let ready_response = ltpd_reqs.remove(1);
    let ready = decode_data_transmit_response(&ready_response.sdu)
        .expect("SN-DATA TRANSMIT RESPONSE should decode after unspecified resource request");
    assert_eq!(ready.nsapi, 1);
    assert_eq!(ready.result, SndcpDataTransmitResponseResult::Accepted);
    let allocation = ready_response
        .chan_alloc
        .as_ref()
        .expect("accepted four-slot capability SN-DATA TRANSMIT RESPONSE should carry PDCH allocation");
    assert_single_slot_fallback_pdch_channel_allocation(allocation);
}

#[test]
fn sndcp_wap_ip_mvp_accepts_mxp600_type_b_specific_four_slot_resource_request() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating),
    ));
    let request = build_mxp600_specific_four_slot_data_transmit_request(1);
    assert_eq!(request.get_len(), 21);
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, request));
    test.deliver_all_messages();

    let mut ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 2);
    let ready_response = ltpd_reqs.remove(1);
    let ready = decode_data_transmit_response(&ready_response.sdu)
        .expect("SN-DATA TRANSMIT RESPONSE should decode after specific four-slot resource request");
    assert_eq!(ready.nsapi, 1);
    assert_eq!(ready.result, SndcpDataTransmitResponseResult::Accepted);
    let allocation = ready_response
        .chan_alloc
        .as_ref()
        .expect("accepted specific four-slot SN-DATA TRANSMIT RESPONSE should carry PDCH allocation");
    assert_single_slot_fallback_pdch_channel_allocation(allocation);
}

#[test]
fn sndcp_wap_ip_end_of_data_returns_common_control_after_pdch_assignment() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating),
    ));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_mxp600_specific_four_slot_data_transmit_request(1),
    ));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .expect("SN-END OF DATA should encode"),
    ));
    test.deliver_all_messages();

    let mut ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 3);
    let ready_response = ltpd_reqs.remove(1);
    assert_single_slot_fallback_pdch_channel_allocation(
        ready_response
            .chan_alloc
            .as_ref()
            .expect("SN-DATA TRANSMIT RESPONSE should allocate PDCH"),
    );

    let end_response = ltpd_reqs.remove(1);
    let end_of_data = decode_end_of_data(&end_response.sdu).expect("SwMI SN-END OF DATA response should decode");
    assert!(!end_of_data.immediate_service_change);
    assert_quit_and_go_common_control_allocation(
        end_response
            .chan_alloc
            .as_ref()
            .expect("SN-END OF DATA response should carry common-control channel allocation"),
    );
}

#[test]
fn sndcp_wap_ip_mvp_accepts_mxp600_type_b_specific_four_slot_reconnect() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_dynamic_ipv4_activation_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating),
    ));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_mxp600_specific_four_slot_data_transmit_request(1),
    ));
    let reconnect = build_mxp600_specific_four_slot_reconnect(1);
    assert_eq!(reconnect.get_len(), 21);
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, reconnect));
    test.deliver_all_messages();

    let mut ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 3);
    let reconnect_response = ltpd_reqs.remove(2);
    let ready = decode_data_transmit_response(&reconnect_response.sdu).expect("SN-DATA TRANSMIT RESPONSE should decode after reconnect");
    assert_eq!(ready.nsapi, 1);
    assert_eq!(ready.result, SndcpDataTransmitResponseResult::Accepted);
    let allocation = reconnect_response
        .chan_alloc
        .as_ref()
        .expect("accepted SN-RECONNECT response should carry the MVP PDCH allocation");
    assert_single_slot_fallback_pdch_channel_allocation(allocation);
}

#[test]
fn sndcp_pdch_default_dynamic_policy_uses_ts2_only_and_keeps_ts1_common_control() {
    let manager = SndcpPdchManager::new();
    let plan = manager
        .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
            issi: 1000001,
            nsapi: 2,
            pdch_available: true,
            downlink_sdu_bits: 64,
            nonfragmented_sdu_capacity_bits: Some(124),
            ..SndcpPacketDataPlanInput::default()
        })
        .expect("ready packet-data subscriber should produce a new PDCH allocation plan");

    let allocation = packet_data_plan_to_lower_channel_allocation(
        &plan,
        SndcpPdchAllocationPolicy::assigned_scch_for_resource_request(SndcpPacketDataResourceRequest::None),
    )
    .expect("default single-slot PDCH bitmap should be valid")
    .expect("new PDCH plan should carry lower channel allocation");

    assert_default_dynamic_pdch_channel_allocation(&allocation.chan_alloc);
}

#[test]
fn sndcp_wap_packet_data_sequence_through_live_mle_has_no_lower_output() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Mle, TetraEntity::Sndcp],
        vec![TetraEntity::Llc, TetraEntity::Cmce, TetraEntity::User, TetraEntity::Brew],
    );

    // This exercises the real MLE protocol-discriminator demux plus the real
    // SNDCP runtime entity. Even when the raw service bit is forced for a
    // decoder test, SNDCP must not emit WAP/IP, CMCE/SDS, or lower LLC output
    // until the PDP/READY/PDCH bearer is wired deliberately.
    submit_wap_packet_data_sequence_through_mle(&mut test);
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_wap_ip_mvp_routes_through_live_mle_to_llc_when_enabled() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Mle, TetraEntity::Sndcp],
        vec![TetraEntity::Llc, TetraEntity::Cmce, TetraEntity::Brew, TetraEntity::User],
    );

    test.submit_message(build_tla_sndcp_unitdata_ind(build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_tla_sndcp_unitdata_ind(
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_tla_sndcp_unitdata_ind(build_wap_status_sn_data_from(2, [10, 0, 0, 2], b"")));
    test.deliver_all_messages();

    let sinks = test.dump_sinks();
    assert!(
        sinks
            .iter()
            .all(|msg| msg.dest != TetraEntity::Cmce && msg.dest != TetraEntity::Brew && msg.dest != TetraEntity::User)
    );

    let llc_msgs: Vec<_> = sinks
        .iter()
        .filter(|msg| msg.sap == Sap::TlaSap && msg.dest == TetraEntity::Llc)
        .collect();
    assert_eq!(llc_msgs.len(), 3, "unexpected sink messages: {sinks:#?}");
    let ready_alloc = llc_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataReqBl(req) => req.chan_alloc.as_ref(),
            _ => None,
        })
        .expect("SN-DATA TRANSMIT RESPONSE should route to LLC with a PDCH allocation");
    assert_default_dynamic_pdch_channel_allocation(ready_alloc);
    let sndcp_sdu = llc_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataReqBl(req) if req.chan_alloc.is_none() && req.link_id != 0 => {
                let mut tl_sdu = BitBuffer::from_bitbuffer(&req.tl_sdu);
                tl_sdu.seek(0);
                if tl_sdu.read_bits(3) == Some(MleProtocolDiscriminator::Sndcp.into_raw() as u64) {
                    let remaining_bits = tl_sdu.get_len() - 3;
                    let mut sndcp_sdu = BitBuffer::new(remaining_bits);
                    sndcp_sdu.copy_bits(&mut tl_sdu, remaining_bits);
                    sndcp_sdu.seek(0);
                    decode_sn_user_data_pdu(&sndcp_sdu).is_ok().then_some(sndcp_sdu)
                } else {
                    None
                }
            }
            _ => None,
        })
        .expect("WAP SN-DATA response should route as packet-data acknowledged TLA DATA over AL");

    let unitdata = decode_sn_user_data_pdu(&sndcp_sdu).expect("TLA payload should carry SN user data");
    let response_octets = bitbuffer_npdu_octets(&unitdata.n_pdu).expect("response N-PDU should be byte aligned");
    let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
    assert_eq!(response_ip.source, [10, 0, 0, 1]);
    assert_eq!(response_ip.destination, [10, 0, 0, 2]);
}

#[test]
fn sndcp_ltpd_configure_ind_clears_live_pdch_session_fail_closed() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    config
        .cell
        .wap_ip
        .as_mut()
        .expect("WAP/IP profile should be enabled")
        .assume_pdch_ready_after_data_transmit = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_configure_ind());
    test.deliver_all_messages();
    assert_no_runtime_side_effects(&mut test);

    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_mxp600_single_slot_data_transmit_request(2)));
    test.deliver_all_messages();
    let _ = test.dump_sinks();

    // Opposite-direction Configure.req is still unexpected at SNDCP, but a
    // live lower Configure.ind for the endpoint must clear stale PDCH readiness.
    test.submit_message(build_ltpd_configure_req());
    test.submit_message(build_ltpd_configure_ind());
    test.deliver_all_messages();

    let configure_reqs = take_ltpd_configure_reqs(&mut test);
    assert_eq!(configure_reqs.len(), 1);
    let req = &configure_reqs[0];
    assert_eq!(req.endpoint_id, 1);
    assert_eq!(req.sndcp_status, 2);
    assert_eq!(req.ms_default_data_prio, -1);
}

#[test]
fn sndcp_ltpd_activation_demand_decodes_but_drops_without_pdp_handler() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // This exercises only the clause 28 activation decoder. The runtime entity
    // intentionally emits no response until PDP activation/context routing is
    // implemented end-to-end and SNDCP advertising is enabled deliberately.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2)));
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_ltpd_deactivation_demand_decodes_but_drops_without_pdp_handler() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // Runtime deactivation handling remains deliberately fail-closed: decode
    // and log only, with no MLE response or CMCE/SDS side effect.
    let pdu = encode_deactivate_pdp_context_demand(&SndcpDeactivation::Nsapi(2)).expect("deactivation demand should encode");
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, pdu));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_deactivation_accept_drops_without_pending_context() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    let pdu = encode_deactivate_pdp_context_accept(&SndcpDeactivation::AllNsapis).expect("deactivation accept should encode");
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, pdu));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_decode_sn_unitdata_no_compression_ipv4_npdu() {
    // EN 300 392-2 clause 28.4.4.14 defines SN-UNITDATA as SN PDU type,
    // NSAPI, PCOMP, DCOMP, then a lower-layer-length N-PDU. This is the
    // smallest WAP-capable bearer step SNDCP can safely implement: decode an
    // uncompressed IP N-PDU, without claiming any UDP/WAP handoff.
    let sdu = build_sn_unitdata(3, 0, 0, &[0x45, 0x00, 0x00, 0x14]);

    let SndcpDecode::Unitdata(unitdata) = decode_ltpd_sdu(&sdu) else {
        panic!("expected decoded SN-UNITDATA");
    };

    assert_eq!(unitdata.nsapi, 3);
    assert_eq!(unitdata.pcomp, 0);
    assert_eq!(unitdata.dcomp, 0);
    assert_eq!(unitdata.network_pdu_kind, NetworkPduKind::Ipv4);
    assert_eq!(unitdata.n_pdu.get_len(), 32);

    let mut n_pdu = BitBuffer::from_bitbuffer(&unitdata.n_pdu);
    assert_eq!(n_pdu.read_bits(8), Some(0x45));
}

#[test]
fn sndcp_decode_sn_data_no_compression_ipv4_npdu() {
    // EN 300 392-2 clause 28.4.4.4/table 28.26 defines SN-DATA with the
    // same NSAPI/PCOMP/DCOMP/N-PDU body as SN-UNITDATA, but using
    // acknowledged service after an advanced link has been established.
    let sdu = build_sn_data(3, 0, 0, &[0x45, 0x00, 0x00, 0x14]);

    let SndcpDecode::Unitdata(unitdata) = decode_ltpd_sdu(&sdu) else {
        panic!("expected decoded SN-DATA");
    };

    assert_eq!(unitdata.nsapi, 3);
    assert_eq!(unitdata.pcomp, 0);
    assert_eq!(unitdata.dcomp, 0);
    assert_eq!(unitdata.network_pdu_kind, NetworkPduKind::Ipv4);
    assert_eq!(unitdata.n_pdu.get_len(), 32);
}

#[test]
fn sndcp_decode_distinguishes_unsupported_packet_data_cases() {
    match decode_ltpd_sdu(&build_sn_pdu(14)) {
        SndcpDecode::UnsupportedPduType(14) => {}
        other => panic!("expected unsupported SN PDU type, got {:?}", other),
    }

    match decode_ltpd_sdu(&build_sn_unitdata(15, 0, 0, &[0x45])) {
        SndcpDecode::UnsupportedNsapi(15) => {}
        other => panic!("expected reserved NSAPI rejection, got {:?}", other),
    }

    match decode_ltpd_sdu(&build_sn_unitdata(3, 1, 0, &[0x45])) {
        SndcpDecode::UnsupportedCompression { pcomp: 1, dcomp: 0 } => {}
        other => panic!("expected unsupported compression rejection, got {:?}", other),
    }
}

#[test]
fn sndcp_decode_transfer_control_pdus_without_runtime_handoff() {
    let request = encode_data_transmit_request(&SndcpDataTransmitRequest {
        nsapi: 2,
        logical_link_status: false,
        resource_request: SndcpPacketDataResourceRequest::None,
    })
    .expect("SN-DATA TRANSMIT REQUEST should encode");
    let SndcpDecode::TransferControl(SndcpTransferControl::DataTransmitRequest(decoded_request)) = decode_ltpd_sdu(&request) else {
        panic!("expected decoded SN-DATA TRANSMIT REQUEST");
    };
    assert_eq!(decoded_request.nsapi, 2);

    let end = encode_end_of_data(&SndcpEndOfData {
        immediate_service_change: true,
    })
    .expect("SN-END OF DATA should encode");
    let SndcpDecode::TransferControl(SndcpTransferControl::EndOfData(decoded_end)) = decode_ltpd_sdu(&end) else {
        panic!("expected decoded SN-END OF DATA");
    };
    assert!(decoded_end.immediate_service_change);

    let not_supported = encode_not_supported(&SndcpNotSupported {
        not_supported_pdu_type: SN_PDU_TYPE_END_OF_DATA,
    })
    .expect("SN-NOT SUPPORTED should encode");
    let SndcpDecode::TransferControl(SndcpTransferControl::NotSupported(decoded_not_supported)) = decode_ltpd_sdu(&not_supported) else {
        panic!("expected decoded SN-NOT SUPPORTED");
    };
    assert_eq!(decoded_not_supported.not_supported_pdu_type, SN_PDU_TYPE_END_OF_DATA);
}

#[test]
fn sndcp_ltpd_reserved_nsapi_and_compression_drop_without_output() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(15, 0, 0, &[0x45])));
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(3, 1, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_transfer_control_decodes_but_drops_without_bearer_state_machine() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .expect("SN-END OF DATA should encode"),
    ));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_malformed_control_pdu_drops_without_output() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_pdu(0)));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_unitdata_drops_without_output_when_service_is_not_advertised() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // Table 18.26 service advertising remains false until a full bearer is
    // available, so even a syntactically supported SN-UNITDATA is dropped.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(3, 0, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_unitdata_decodes_but_drops_without_sn_sap_handoff() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // Direct config can exercise the local decoder, but SNDCP still has no
    // SN-SAP/IP/WAP handoff in this stack. The fail-safe result is no output.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(3, 0, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_unexpected_sap_drops_without_panic() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // A malformed internal route must not crash SNDCP or synthesize packet
    // data output while the bearer is unsupported.
    test.submit_message(build_ltpd_ind(Sap::LmmSap, build_sn_unitdata(3, 0, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_report_indication_drops_without_pending_request() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_report_ind(7, 0));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}
