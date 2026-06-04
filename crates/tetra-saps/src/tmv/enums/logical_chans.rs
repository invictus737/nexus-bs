/// Logical channels as defined in the standard
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogicalChannel {
    /// Access Assignment CHannel
    Aach,

    /// Signalling Channel (half slot, downlink)
    SchHd,
    /// Signalling Channel (full slot)
    SchF,
    /// STealing Channel (half slot)
    Stch,
    /// Signalling Channel (half slot, uplink)
    SchHu,

    /// Traffic Channel (Voice)
    TchS,
    /// Traffic Channel (24 kbps)
    Tch24,
    /// Traffic Channel (48 kbps)
    Tch48,
    /// Traffic Channel (72 kbps)
    Tch72,

    /// Broadcast Synchronization Channel
    Bsch,
    /// Broadcast Network Channel
    Bnch,

    /// BS Linearization CHannel (downlink)
    Blch,
    /// Common Linearization Channel (uplink)
    Clch,
}

impl LogicalChannel {
    /// Returns the number of bits required to represent the logical channel
    pub fn is_traffic(self) -> bool {
        matches!(
            self,
            LogicalChannel::TchS | LogicalChannel::Tch24 | LogicalChannel::Tch48 | LogicalChannel::Tch72
        )
    }

    /// Returns true for logical channels carrying ordinary C-plane signalling
    /// or packet data in this stack. ETSI EN 300 392-2 clause 9.2.3 also
    /// categorizes LCH as CCH, but BLCH/CLCH use distinct linearization
    /// coding paths and are exposed through `is_linearization_channel`.
    pub fn is_control_channel(self) -> bool {
        match self {
            LogicalChannel::Aach | // Odd one since very different decoding, but actually part of CP
            LogicalChannel::Bsch | // Also not containing regular mac blocks but still CP
            LogicalChannel::Bnch |
            LogicalChannel::SchHd |
            LogicalChannel::SchF |
            LogicalChannel::Stch |
            LogicalChannel::SchHu => true,
            _ => false,
        }
    }

    /// Returns true if channel is a linearization channel
    pub fn is_linearization_channel(self) -> bool {
        self == LogicalChannel::Clch || self == LogicalChannel::Blch
    }

    /// Returns true if channel may be encountered on the downlink
    pub fn is_dl_channel(self) -> bool {
        match self {
            LogicalChannel::Aach
            | LogicalChannel::SchHd
            | LogicalChannel::SchF
            | LogicalChannel::Stch
            | LogicalChannel::Bsch
            | LogicalChannel::Bnch
            | LogicalChannel::Blch
            | LogicalChannel::TchS
            | LogicalChannel::Tch24
            | LogicalChannel::Tch48
            | LogicalChannel::Tch72 => true,
            LogicalChannel::SchHu | LogicalChannel::Clch => false,
        }
    }

    /// Returns true if channel may be encountered on the uplink
    pub fn is_ul_channel(self) -> bool {
        match self {
            LogicalChannel::SchHu
            | LogicalChannel::SchF
            | LogicalChannel::Stch
            | LogicalChannel::Clch
            | LogicalChannel::TchS
            | LogicalChannel::Tch24
            | LogicalChannel::Tch48
            | LogicalChannel::Tch72 => true,
            LogicalChannel::Aach | LogicalChannel::SchHd | LogicalChannel::Bsch | LogicalChannel::Bnch | LogicalChannel::Blch => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LogicalChannel;

    #[test]
    fn logical_channel_traffic_matches_etsi_tch_family() {
        for channel in [
            LogicalChannel::TchS,
            LogicalChannel::Tch24,
            LogicalChannel::Tch48,
            LogicalChannel::Tch72,
        ] {
            assert!(channel.is_traffic(), "{channel:?} should be traffic");
            assert!(!channel.is_control_channel(), "{channel:?} should not be C-plane control");
            assert!(!channel.is_linearization_channel(), "{channel:?} should not be linearization");
            assert!(channel.is_dl_channel(), "{channel:?} can be downlink");
            assert!(channel.is_ul_channel(), "{channel:?} can be uplink");
        }
    }

    #[test]
    fn logical_channel_control_matches_etsi_cch_signalling_channels() {
        for channel in [
            LogicalChannel::Aach,
            LogicalChannel::Bsch,
            LogicalChannel::Bnch,
            LogicalChannel::SchHd,
            LogicalChannel::SchF,
            LogicalChannel::SchHu,
            LogicalChannel::Stch,
        ] {
            assert!(!channel.is_traffic(), "{channel:?} should not be traffic");
            assert!(channel.is_control_channel(), "{channel:?} should be C-plane control");
            assert!(!channel.is_linearization_channel(), "{channel:?} should not be linearization");
        }
    }

    #[test]
    fn logical_channel_direction_matches_etsi_one_way_channels() {
        for channel in [
            LogicalChannel::Aach,
            LogicalChannel::Bsch,
            LogicalChannel::Bnch,
            LogicalChannel::SchHd,
        ] {
            assert!(channel.is_dl_channel(), "{channel:?} should be downlink");
            assert!(!channel.is_ul_channel(), "{channel:?} should not be uplink");
        }

        for channel in [LogicalChannel::SchHu, LogicalChannel::Clch] {
            assert!(!channel.is_dl_channel(), "{channel:?} should not be downlink");
            assert!(channel.is_ul_channel(), "{channel:?} should be uplink");
        }

        for channel in [LogicalChannel::SchF, LogicalChannel::Stch] {
            assert!(channel.is_dl_channel(), "{channel:?} can be downlink");
            assert!(channel.is_ul_channel(), "{channel:?} can be uplink");
        }
    }

    #[test]
    fn logical_channel_linearization_is_explicitly_separate() {
        for channel in [LogicalChannel::Blch, LogicalChannel::Clch] {
            assert!(!channel.is_traffic(), "{channel:?} should not be traffic");
            assert!(
                !channel.is_control_channel(),
                "{channel:?} uses the linearization path, not regular C-plane decoding"
            );
            assert!(channel.is_linearization_channel(), "{channel:?} should be linearization");
        }

        assert!(LogicalChannel::Blch.is_dl_channel());
        assert!(!LogicalChannel::Blch.is_ul_channel());
        assert!(!LogicalChannel::Clch.is_dl_channel());
        assert!(LogicalChannel::Clch.is_ul_channel());
    }
}
