// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::time::Instant;

pub fn health_check() {
    let now = Instant::now();
    if now.duration_since(last_ping_at) >= heartbeat_interval {
        ping_seq = ping_seq.wrapping_add(1);
        let payload = ping_seq.to_be_bytes().to_vec();
        if let Err(e) = ws.send(Message::Ping(payload)) {
            return Err(format!("WebSocket ping failed: {}", e));
        }
        last_ping_at = now;
        last_ping_id = Some(ping_seq);
        last_ping_sent_at = Some(now);
    }

    if now.duration_since(last_activity_at) >= heartbeat_timeout {
        return Err("heartbeat timeout".to_string());
    }
}
