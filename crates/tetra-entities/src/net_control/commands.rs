// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

// ---------------------------------------------------------------------------
// Command / CommandResponse — concrete enums sent through the channel
//
// The command server sends a Command; the stack processes it and returns
// a CommandResponse.  Placeholder variants are provided for now.
// ---------------------------------------------------------------------------

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

/// EN 300 392-2 table 29.21 assigns PID 0x04 to WAP over SDS Type4
/// without SDS-TL transfer service. Use this when the bytes after the PID are
/// application-defined WAP payload.
pub const WAP_WDP_PROTOCOL_ID: u8 = 0x04;

/// EN 300 392-2 table 29.21 assigns PID 0x84 to WAP with SDS-TL transfer
/// service. Bytes after this PID must start with an SDS-TL PDU, not raw WML.
pub const WAP_SDS_TL_PROTOCOL_ID: u8 = 0x84;

/// SDS-TL TRANSFER flags for WAP with no delivery report request, no
/// storage/forward control, and no short-form-report recommendation.
pub const WAP_SDS_TL_TRANSFER_FLAGS_NO_REPORT: u8 = 0x00;

/// Message shown by the default MVP WAP page.
pub const WAP_MVP_MESSAGE_TEXT: &str = "Hello! You are running Nexus-BS. Gretings and 73! from Chris YO3TCO!";

/// Default MVP WAP page body used by the operator-facing control shortcut.
///
/// This is intentionally compact WML: byte-aligned SDS Type4 allows at most
/// 255 payload octets after the 11-bit length bound, including the WAP PID.
/// The two-card timer loop gives old WAP browsers a simple flashing effect.
pub const WAP_MVP_PAGE_TEXT: &str = "<wml><card id=\"a\" ontimer=\"#b\"><timer value=\"6\"/><p><b>Hello! You are running Nexus-BS. Gretings and 73! from Chris YO3TCO!</b></p></card><card id=\"b\" ontimer=\"#a\"><timer value=\"6\"/><p><big>*** FLASH ***</big><br/>ON AIR 73 YO3TCO</p></card></wml>";

/// Optional color/blink page for WAP 2.0 or terminal WAP browsers.
///
/// WML 1.x does not standardize color; this compact HTML-style page is a
/// pragmatic operator shortcut for clients that render color attributes or
/// blink text while still falling back to readable text.
pub const WAP_MVP_COLOR_PAGE_TEXT: &str = "<html><body bgcolor=\"#000\" text=\"#0f0\"><p><blink><font color=\"red\"><b>*** ON AIR ***</b></font></blink><br/>Hello! You are running Nexus-BS. Gretings and 73! from Chris YO3TCO!</p></body></html>";

pub const WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES: usize = 255;

pub fn wap_sds_type4_payload(page_text: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + page_text.len());
    payload.push(WAP_WDP_PROTOCOL_ID);
    payload.extend_from_slice(page_text.as_bytes());
    payload
}

pub fn wap_sds_tl_transfer_type4_payload(page_text: &str, message_reference: u8) -> Vec<u8> {
    let mut payload = Vec::with_capacity(3 + page_text.len());
    payload.push(WAP_SDS_TL_PROTOCOL_ID);
    payload.push(WAP_SDS_TL_TRANSFER_FLAGS_NO_REPORT);
    payload.push(message_reference);
    payload.extend_from_slice(page_text.as_bytes());
    payload
}

/// Command received from the remote command server.
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum ControlCommand {
    /// Send an SDS for local delivery
    /// `payload` is bare text bytes, not a prebuilt SDS-TL Type4 payload. Valid
    /// UTF-8 is encoded to the SDS-TL text coding scheme by CMCE; invalid UTF-8
    /// is preserved byte-for-byte as ISO/IEC 8859-1 text.
    SendSds {
        handle: u32,
        source_ssi: u32,
        dest_ssi: u32,
        dest_is_group: bool,
        len_bits: u16,
        payload: Vec<u8>,
    },

    /// Send a raw SDS user-defined-data payload for local delivery.
    /// `sdti` is the EN 300 392-2 short data type identifier: 0/1/2 are
    /// fixed 16/32/64-bit user data, and 3 is Type4 with `len_bits` bits.
    /// For `sdti = 3`, `payload` is the complete Type4 user data including
    /// the protocol identifier. Standard application PIDs are carried
    /// opaquely; for example WAP/WCMP `0x04/0x05` carry application-defined
    /// payloads directly, while `0x84/0x85` require the caller to include the
    /// SDS-TL PDU bytes after the PID.
    SendRawSds {
        handle: u32,
        source_ssi: u32,
        dest_ssi: u32,
        dest_is_group: bool,
        sdti: u8,
        len_bits: u16,
        payload: Vec<u8>,
    },

    /// Send a pre-coded SDS status for local delivery.
    ///
    /// EN 300 392-2 clause 13.2 separates user-defined short messages from
    /// pre-defined status messages. This command maps to D-STATUS, not to
    /// D-SDS-DATA Type1 user data.
    SendStatus {
        handle: u32,
        source_ssi: u32,
        dest_ssi: u32,
        dest_is_group: bool,
        status_number: u16,
    },

    /// Forcibly deregister a terminal from the BS
    KickMs { issi: u32 },

    /// Restart the Nexus-BS service (systemctl restart nexus-bs)
    RestartService,

    /// Stop the Nexus-BS service (systemctl stop nexus-bs)
    ShutdownService,

    /// Power off the Linux host running Nexus-BS.
    PowerOffHost,

    /// Stop the Nexus-BS core now and let systemd bring it back after RestartSec.
    ///
    /// This intentionally exits the process instead of delaying inside the RF
    /// core, so volatile buffers, call state, and radio runtime caches are
    /// cleared before the next start.
    StopGoService { start_delay_secs: u64 },

    /// Runtime RF carrier inhibit. This is volatile operator state, not a
    /// persisted config change: restart returns to carrier active.
    SetRfCarrierInhibit { inhibited: bool },

    /// Run destructive local TX DC/IQ calibration in PHY and write calibration.toml.
    ///
    /// This is a local maintenance command, not an air-interface TETRA PDU.
    RunTxCalibration { calibration_path: String },

    /// Add a live SDS message to the broadcast queue.
    /// The message will be transmitted to all MSs on the cell at the next HMD interval,
    /// round-robining with the static Home Mode Display text.
    /// `repeat_count = 0` means repeat indefinitely; `> 0` auto-removes after N transmissions.
    AddLiveSds {
        text: String,
        protocol_id: u8,
        source_issi: u32,
        repeat_count: u32,
    },

    /// Remove a live SDS message from the queue by its ID.
    DeleteLiveSds { id: u32 },

    /// Remove all live SDS messages from the queue.
    ClearLiveSds,

    /// Placeholder command A.
    CommandA { handle: u32, parameter: u32 },
    /// Placeholder command B.
    TestCmdB {
        handle: u32,
        source_ssi: u32,
        is_group: bool,
        payload: Vec<u8>,
    },
}

/// Response sent back after processing a [`ControlCommand`].
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum ControlResponse {
    CommandAResponse {
        handle: u32,
        result: u32,
    },
    SendSdsResponse {
        handle: u32,
        success: bool,
    },
    /// Response for raw Type1..Type4 SDS requests, including WAP-over-SDS
    /// payloads using the EN 300 392-2 table 29.21 WAP/WCMP protocol IDs.
    SendRawSdsResponse {
        handle: u32,
        success: bool,
    },
    SendStatusResponse {
        handle: u32,
        success: bool,
    },
    KickMsResponse {
        issi: u32,
        success: bool,
    },
}
