//! TPCI helpers for KNX transport-layer connection control.
//!
//! The Transport Protocol Control Information (TPCI) byte governs the
//! connection-oriented transport layer used for device-addressed
//! communication (for example the bus probe `T_Connect` +
//! `DeviceDescriptorRead` sequence). This module provides a typed
//! representation of the supported control frames together with
//! [`TpciFrame::encode`] / [`TpciFrame::decode`] helpers.

use crate::log_protocol;
use crate::logging::LogLevel;

/// TPCI control byte for `T_Connect`.
const T_CONNECT: u8 = 0x80;
/// TPCI control byte for `T_Disconnect`.
const T_DISCONNECT: u8 = 0x81;
/// Base control byte for `T_Data_Connected` (combined with `seq << 2`).
const T_DATA_CONNECTED: u8 = 0x40;
/// Base control byte for `T_ACK` (combined with `seq << 2`).
const T_ACK: u8 = 0xC2;
/// Base control byte for `T_NAK` (combined with `seq << 2`).
const T_NAK: u8 = 0xC3;

/// A KNX transport-layer (TPCI) control frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpciFrame {
    /// `T_Connect` — establish a connection-oriented transport link.
    Connect,
    /// `T_Disconnect` — tear down a connection-oriented transport link.
    Disconnect,
    /// `T_Data_Connected` — numbered data frame carrying a sequence counter.
    DataConnected {
        /// 4-bit sequence number (0..=15).
        sequence: u8,
    },
    /// `T_ACK` — positive acknowledgement of a numbered data frame.
    Ack {
        /// 4-bit sequence number (0..=15).
        sequence: u8,
    },
    /// `T_NAK` — negative acknowledgement of a numbered data frame.
    Nak {
        /// 4-bit sequence number (0..=15).
        sequence: u8,
    },
}

impl TpciFrame {
    /// Encode this transport frame into its TPCI control byte.
    #[must_use]
    pub fn encode(&self) -> u8 {
        let result = match self {
            Self::Connect => T_CONNECT,
            Self::Disconnect => T_DISCONNECT,
            Self::DataConnected { sequence } => T_DATA_CONNECTED | ((sequence & 0x0F) << 2),
            Self::Ack { sequence } => T_ACK | ((sequence & 0x0F) << 2),
            Self::Nak { sequence } => T_NAK | ((sequence & 0x0F) << 2),
        };
        log_protocol!(
            LogLevel::Trace,
            "TPCI encode: {:?} \u{2192} 0x{:02X}",
            self,
            result
        );
        result
    }

    /// Decode a TPCI control byte into a transport frame.
    ///
    /// Recognises `T_Connect` (`0x80`), `T_Disconnect` (`0x81`),
    /// `T_Data_Connected` (`0x40 | (seq << 2)`), `T_ACK` (`0xC2 | (seq << 2)`)
    /// and `T_NAK` (`0xC3 | (seq << 2)`). The most specific matching pattern
    /// is selected based on the high (bits 7..6) and low (bits 1..0) bits.
    #[must_use]
    pub fn decode(byte: u8) -> TpciFrame {
        let sequence = (byte >> 2) & 0x0F;
        let frame = match byte {
            T_CONNECT => Self::Connect,
            T_DISCONNECT => Self::Disconnect,
            // T_ACK: 0xC2 | (seq << 2) -> bits 7..6 = 11, bits 1..0 = 10.
            b if (b & 0xC3) == T_ACK => Self::Ack { sequence },
            // T_NAK: 0xC3 | (seq << 2) -> bits 7..6 = 11, bits 1..0 = 11.
            b if (b & 0xC3) == T_NAK => Self::Nak { sequence },
            // T_Data_Connected: 0x40 | (seq << 2) -> bit 7 = 0, bit 6 = 1, bits 1..0 = 00.
            b if (b & 0xC3) == T_DATA_CONNECTED => Self::DataConnected { sequence },
            _ => {
                log_protocol!(
                    LogLevel::Warn,
                    "TPCI: unrecognised control byte 0x{:02X}",
                    byte
                );
                Self::DataConnected { sequence }
            }
        };
        log_protocol!(
            LogLevel::Trace,
            "TPCI decode: 0x{:02X} \u{2192} {:?}",
            byte,
            frame
        );
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::TpciFrame;

    #[test]
    fn connect_round_trip() {
        assert_eq!(TpciFrame::Connect.encode(), 0x80);
        assert_eq!(TpciFrame::decode(0x80), TpciFrame::Connect);
    }

    #[test]
    fn disconnect_round_trip() {
        assert_eq!(TpciFrame::Disconnect.encode(), 0x81);
        assert_eq!(TpciFrame::decode(0x81), TpciFrame::Disconnect);
    }

    #[test]
    fn data_connected_round_trip() {
        for sequence in [0u8, 1, 5, 15] {
            let frame = TpciFrame::DataConnected { sequence };
            let byte = frame.encode();
            assert_eq!(byte, 0x40 | (sequence << 2));
            assert_eq!(TpciFrame::decode(byte), frame);
        }
    }

    #[test]
    fn ack_round_trip() {
        for sequence in [0u8, 3, 7, 15] {
            let frame = TpciFrame::Ack { sequence };
            let byte = frame.encode();
            assert_eq!(byte, 0xC2 | (sequence << 2));
            assert_eq!(TpciFrame::decode(byte), frame);
        }
    }

    #[test]
    fn nak_round_trip() {
        for sequence in [0u8, 2, 9, 15] {
            let frame = TpciFrame::Nak { sequence };
            let byte = frame.encode();
            assert_eq!(byte, 0xC3 | (sequence << 2));
            assert_eq!(TpciFrame::decode(byte), frame);
        }
    }

    #[test]
    fn data_connected_known_bytes() {
        assert_eq!(
            TpciFrame::decode(0x40),
            TpciFrame::DataConnected { sequence: 0 }
        );
        assert_eq!(
            TpciFrame::decode(0x44),
            TpciFrame::DataConnected { sequence: 1 }
        );
    }

    #[test]
    fn ack_known_bytes() {
        assert_eq!(TpciFrame::decode(0xC2), TpciFrame::Ack { sequence: 0 });
        assert_eq!(TpciFrame::decode(0xC6), TpciFrame::Ack { sequence: 1 });
    }

    #[test]
    fn nak_known_bytes() {
        assert_eq!(TpciFrame::decode(0xC3), TpciFrame::Nak { sequence: 0 });
        assert_eq!(TpciFrame::decode(0xC7), TpciFrame::Nak { sequence: 1 });
    }

    #[test]
    fn sequence_is_masked_to_four_bits() {
        // Only the low nibble of the sequence is significant.
        let frame = TpciFrame::DataConnected { sequence: 0x10 };
        assert_eq!(frame.encode(), 0x40);
    }
}
