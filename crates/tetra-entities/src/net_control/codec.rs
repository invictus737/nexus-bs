// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

//! Command codec — bitcode-based and JSON-based serialization of
//! [`Command`]s and [`CommandResponse`]s.

use crate::{
    net_control::commands::{ControlCommand, ControlResponse},
    network::transports::NetworkError,
};

// ---------------------------------------------------------------------------
// Codecs
// ---------------------------------------------------------------------------

/// Codec for commands using bitcode for serialization.
#[derive(Default)]
pub struct ControlCodecBitcode;

impl ControlCodecBitcode {
    /// Encode a [`Command`] to bitcode bytes.
    pub fn encode_command(&self, cmd: &ControlCommand) -> Vec<u8> {
        bitcode::encode(cmd)
    }

    /// Decode bitcode bytes into a [`Command`].
    pub fn decode_command(&self, payload: &[u8]) -> Result<ControlCommand, NetworkError> {
        bitcode::decode(payload).map_err(|e| NetworkError::SerializationError(format!("command decode: {}", e)))
    }

    /// Encode a [`CommandResponse`] to bitcode bytes.
    pub fn encode_response(&self, resp: &ControlResponse) -> Vec<u8> {
        bitcode::encode(resp)
    }

    /// Decode bitcode bytes into a [`CommandResponse`].
    pub fn decode_response(&self, payload: &[u8]) -> Result<ControlResponse, NetworkError> {
        bitcode::decode(payload).map_err(|e| NetworkError::SerializationError(format!("command response decode: {}", e)))
    }
}

/// Codec for commands using JSON for serialization.
#[derive(Default)]
pub struct ControlCodecJson;

impl ControlCodecJson {
    /// Encode a [`Command`] to JSON bytes.
    pub fn encode_command(&self, cmd: &ControlCommand) -> Vec<u8> {
        serde_json::to_vec(cmd).unwrap_or_default()
    }

    /// Decode JSON bytes into a [`Command`].
    pub fn decode_command(&self, payload: &[u8]) -> Result<ControlCommand, NetworkError> {
        serde_json::from_slice(payload).map_err(|e| NetworkError::SerializationError(format!("command decode: {}", e)))
    }

    /// Encode a [`CommandResponse`] to JSON bytes.
    pub fn encode_response(&self, resp: &ControlResponse) -> Vec<u8> {
        serde_json::to_vec(resp).unwrap_or_default()
    }

    /// Decode JSON bytes into a [`CommandResponse`].
    pub fn decode_response(&self, payload: &[u8]) -> Result<ControlResponse, NetworkError> {
        serde_json::from_slice(payload).map_err(|e| NetworkError::SerializationError(format!("command response decode: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_bitcode_command_a() {
        let codec = ControlCodecBitcode;
        let cmd = ControlCommand::CommandA {
            handle: 1,
            parameter: 1234,
        };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::CommandA { handle, parameter } = decoded else {
            panic!("expected CommandA");
        };
        assert_eq!(handle, 1);
        assert_eq!(parameter, 1234);
    }

    #[test]
    fn test_roundtrip_json_command_a() {
        let codec = ControlCodecJson;
        let cmd = ControlCommand::CommandA {
            handle: 1,
            parameter: 1234,
        };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::CommandA { handle, parameter } = decoded else {
            panic!("expected CommandA");
        };
        assert_eq!(handle, 1);
        assert_eq!(parameter, 1234);
    }

    #[test]
    fn test_roundtrip_bitcode_rf_carrier_inhibit() {
        let codec = ControlCodecBitcode;
        let cmd = ControlCommand::SetRfCarrierInhibit { inhibited: true };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::SetRfCarrierInhibit { inhibited } = decoded else {
            panic!("expected SetRfCarrierInhibit");
        };
        assert!(inhibited);
    }

    #[test]
    fn test_roundtrip_json_rf_carrier_inhibit() {
        let codec = ControlCodecJson;
        let cmd = ControlCommand::SetRfCarrierInhibit { inhibited: false };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::SetRfCarrierInhibit { inhibited } = decoded else {
            panic!("expected SetRfCarrierInhibit");
        };
        assert!(!inhibited);
    }

    #[test]
    fn test_roundtrip_bitcode_send_raw_sds() {
        let codec = ControlCodecBitcode;
        let cmd = ControlCommand::SendRawSds {
            handle: 7,
            source_ssi: 9999,
            dest_ssi: 0x00FF_FFFF,
            dest_is_group: false,
            sdti: 3,
            len_bits: 20,
            payload: vec![0xDC, 0xA0, 0x00],
        };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::SendRawSds {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            sdti,
            len_bits,
            payload,
        } = decoded
        else {
            panic!("expected SendRawSds");
        };
        assert_eq!(handle, 7);
        assert_eq!(source_ssi, 9999);
        assert_eq!(dest_ssi, 0x00FF_FFFF);
        assert!(!dest_is_group);
        assert_eq!(sdti, 3);
        assert_eq!(len_bits, 20);
        assert_eq!(payload, vec![0xDC, 0xA0, 0x00]);
    }

    #[test]
    fn test_roundtrip_json_send_raw_sds() {
        let codec = ControlCodecJson;
        let cmd = ControlCommand::SendRawSds {
            handle: 8,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 0,
            len_bits: 16,
            payload: vec![0x82, 0x10],
        };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::SendRawSds {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            sdti,
            len_bits,
            payload,
        } = decoded
        else {
            panic!("expected SendRawSds");
        };
        assert_eq!(handle, 8);
        assert_eq!(source_ssi, 9999);
        assert_eq!(dest_ssi, 2000001);
        assert!(!dest_is_group);
        assert_eq!(sdti, 0);
        assert_eq!(len_bits, 16);
        assert_eq!(payload, vec![0x82, 0x10]);
    }

    #[test]
    fn test_roundtrip_bitcode_send_status() {
        let codec = ControlCodecBitcode;
        let cmd = ControlCommand::SendStatus {
            handle: 9,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            status_number: 0x8210,
        };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::SendStatus {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            status_number,
        } = decoded
        else {
            panic!("expected SendStatus");
        };
        assert_eq!(handle, 9);
        assert_eq!(source_ssi, 9999);
        assert_eq!(dest_ssi, 2000001);
        assert!(!dest_is_group);
        assert_eq!(status_number, 0x8210);
    }

    #[test]
    fn test_roundtrip_json_send_status() {
        let codec = ControlCodecJson;
        let cmd = ControlCommand::SendStatus {
            handle: 10,
            source_ssi: 9999,
            dest_ssi: 100,
            dest_is_group: true,
            status_number: 0x9001,
        };
        let bytes = codec.encode_command(&cmd);
        let decoded = codec.decode_command(&bytes).unwrap();
        let ControlCommand::SendStatus {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            status_number,
        } = decoded
        else {
            panic!("expected SendStatus");
        };
        assert_eq!(handle, 10);
        assert_eq!(source_ssi, 9999);
        assert_eq!(dest_ssi, 100);
        assert!(dest_is_group);
        assert_eq!(status_number, 0x9001);
    }

    #[test]
    fn test_roundtrip_bitcode_response() {
        let codec = ControlCodecBitcode;
        let resp = ControlResponse::CommandAResponse { handle: 1, result: 42 };
        let bytes = codec.encode_response(&resp);
        let decoded = codec.decode_response(&bytes).unwrap();
        let ControlResponse::CommandAResponse { handle, result } = decoded else {
            panic!("expected CommandAResponse");
        };
        assert_eq!(handle, 1);
        assert_eq!(result, 42);
    }

    #[test]
    fn test_roundtrip_json_response() {
        let codec = ControlCodecJson;
        let resp = ControlResponse::SendSdsResponse { handle: 2, success: true };
        let bytes = codec.encode_response(&resp);
        let decoded = codec.decode_response(&bytes).unwrap();
        let ControlResponse::SendSdsResponse { handle, success } = decoded else {
            panic!("expected SendSdsResponse");
        };
        assert_eq!(handle, 2);
        assert!(success);
    }

    #[test]
    fn test_roundtrip_json_send_raw_sds_response() {
        let codec = ControlCodecJson;
        let resp = ControlResponse::SendRawSdsResponse { handle: 73, success: true };
        let bytes = codec.encode_response(&resp);
        let decoded = codec.decode_response(&bytes).unwrap();
        let ControlResponse::SendRawSdsResponse { handle, success } = decoded else {
            panic!("expected SendRawSdsResponse");
        };
        // EN 300 392-2 table 29.21 WAP-over-SDS uses raw Type4 payloads with
        // WAP/WCMP PIDs. Keep the operator response distinct from text SDS so
        // WAP delivery can be confirmed without implying an SNDCP/IP bearer.
        assert_eq!(handle, 73);
        assert!(success);
    }

    #[test]
    fn test_roundtrip_bitcode_send_raw_sds_response() {
        let codec = ControlCodecBitcode;
        let resp = ControlResponse::SendRawSdsResponse {
            handle: 74,
            success: false,
        };
        let bytes = codec.encode_response(&resp);
        let decoded = codec.decode_response(&bytes).unwrap();
        let ControlResponse::SendRawSdsResponse { handle, success } = decoded else {
            panic!("expected SendRawSdsResponse");
        };
        assert_eq!(handle, 74);
        assert!(!success);
    }

    #[test]
    fn test_roundtrip_json_send_status_response() {
        let codec = ControlCodecJson;
        let resp = ControlResponse::SendStatusResponse { handle: 10, success: true };
        let bytes = codec.encode_response(&resp);
        let decoded = codec.decode_response(&bytes).unwrap();
        let ControlResponse::SendStatusResponse { handle, success } = decoded else {
            panic!("expected SendStatusResponse");
        };
        assert_eq!(handle, 10);
        assert!(success);
    }

    #[test]
    fn test_decode_invalid_bytes() {
        let codec = ControlCodecBitcode;
        // Use truncated bytes that cannot form a valid Command
        assert!(codec.decode_command(&[]).is_err());
    }
}
