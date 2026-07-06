//! Bus probe procedure for individual-address occupancy detection.
//!
//! The probe drives the connection-oriented transport layer to determine
//! whether a given [`IndividualAddress`] is already in use on the bus. It
//! sends a `T_Connect` followed by a `DeviceDescriptorRead(0)` carried over
//! `T_Data_Connected(seq=0)` to the candidate address, then waits up to six
//! seconds for one of:
//!
//! * a `DeviceDescriptorResponse` — the device answered, so the address is
//!   **occupied**;
//! * a `T_Disconnect` indication — a device rejected the connection, so the
//!   address is **occupied**;
//! * nothing (timeout) — no device responded, so the address is **free**.
//!
//! This is consumed by the auto-select procedure, which loops the probe over
//! the reserved device range `x.y.240..=254`.
//!
//! ## CEMI serialisation gotcha
//!
//! [`CemiFrame::serialize`](crate::protocol::cemi::CemiFrame) only emits the
//! TPCI octet when the NPDU length is greater than zero. A bare
//! `T_Connect`/`T_Disconnect` frame carries a TPCI octet but **no** APCI/data,
//! so it cannot be produced through `CemiFrame::new`. The probe therefore
//! hand-assembles the raw CEMI `L_Data.req` byte vectors here (see
//! `build_ldata_req_cemi`). The same limitation affects parsing of an
//! incoming `T_Disconnect` indication, so `parse_probe_cemi` recovers the
//! dropped TPCI octet directly from the raw bytes.
//!
//! ## Source address
//!
//! The [`Tunnel`] does not expose the individual address the gateway assigned
//! to the connection, so the probe uses the broadcast source address `0.0.0`.
//! KNX/IP gateways overwrite the source address of outgoing `L_Data.req`
//! frames with the tunnel's assigned address, so this choice is safe.

use std::time::Duration;

use super::address_registry::AddressRegistry;
use super::tunnel::Tunnel;
use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::address::IndividualAddress;
use crate::protocol::cemi::CemiFrame;
use crate::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingRequest};
use crate::protocol::management::DeviceDescriptorRead;
use crate::protocol::tpci::TpciFrame;

/// CEMI message code for an `L_Data.req`.
const MC_L_DATA_REQ: u8 = 0x11;
/// Control byte 1: standard frame, no-repeat, no-system-broadcast, normal
/// priority. Mirrors the flags produced by `CemiFrame::new`.
const CTRL1_STD_NORMAL: u8 = 0xB4;
/// Control byte 2: individual destination address (bit 7 = 0), hop count 6.
const CTRL2_INDIVIDUAL_HOP6: u8 = 0x60;
/// Maximum time to wait for a device to react to the probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

/// First device number in the reserved auto-select range (`x.y.240`).
const AUTO_SELECT_FIRST_DEVICE: u8 = 240;
/// Last device number in the reserved auto-select range (`x.y.254`).
const AUTO_SELECT_LAST_DEVICE: u8 = 254;

/// Classification of an incoming CEMI frame during a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeSignal {
    /// The frame proves the address is occupied (descriptor response or
    /// disconnect).
    Occupied,
    /// The frame is unrelated to the probe.
    Other,
}

/// Probe `addr` on the bus reachable through `conn`.
///
/// Returns `Ok(true)` if the address appears occupied (a device responded or
/// disconnected) and `Ok(false)` if the probe timed out with no reaction
/// (address free).
///
/// # Errors
///
/// Genuine transport failures (send/receive errors on `conn`) propagate as `Err`.
pub async fn probe_address(conn: &Tunnel, addr: IndividualAddress) -> Result<bool> {
    // The gateway overwrites the source address of outgoing L_Data.req frames,
    // so the broadcast source 0.0.0 is sufficient.
    let src = IndividualAddress::broadcast();

    // 1. Establish the connection-oriented link with T_Connect.
    let tconnect = build_tconnect_cemi(src, addr);
    conn.send_frame(&wrap_cemi(conn, tconnect)).await?;

    // 2. Ask the device for its descriptor over T_Data_Connected(seq=0).
    let ddr = build_ddr_cemi(src, addr);
    conn.send_frame(&wrap_cemi(conn, ddr)).await?;

    // 3. Wait up to PROBE_TIMEOUT for a reaction. A timeout means "free".
    let occupied = match tokio::time::timeout(PROBE_TIMEOUT, recv_until_signal(conn)).await {
        Ok(Ok(occupied)) => occupied,
        Ok(Err(e)) => return Err(e),
        Err(_elapsed) => false,
    };

    // 4. Best-effort teardown of the connection-oriented link.
    let tdisconnect = build_tdisconnect_cemi(src, addr);
    let _ = conn.send_frame(&wrap_cemi(conn, tdisconnect)).await;

    log_transport!(
        LogLevel::Debug,
        "Probing address {}: {}",
        addr,
        if occupied { "occupied" } else { "free" }
    );

    Ok(occupied)
}

/// Receive frames until one proves the address occupied.
///
/// Loops on [`Tunnel::recv_frame`], acknowledging incoming `TunnellingRequest`s
/// when the tunnel requires it, and returns `Ok(true)` as soon as a probe
/// signal (descriptor response or disconnect) is observed. Returns `Err` only
/// on a transport failure. The caller bounds this with a timeout.
async fn recv_until_signal(conn: &Tunnel) -> Result<bool> {
    loop {
        let data = conn.recv_frame().await?;

        let Ok(frame) = KnxIpFrame::parse(&data) else {
            continue; // ignore unparseable frames
        };

        if frame.header.service_type != ServiceType::TunnellingRequest {
            continue;
        }

        let Ok(request) = TunnellingRequest::parse(&frame.body) else {
            continue;
        };

        // Mirror the normal receive path: acknowledge the request on UDP.
        if conn.send_acks {
            let _ = conn
                .send_tunnelling_ack(
                    request.communication_channel_id,
                    request.sequence_counter,
                    0,
                )
                .await;
        }

        if let Ok(cemi) = parse_probe_cemi(&request.raw_cemi)
            && classify_cemi(&cemi) == ProbeSignal::Occupied
        {
            return Ok(true);
        }
    }
}

/// Wrap a raw CEMI byte vector in a `TunnellingRequest` + `KnxIpFrame`,
/// returning the serialised KNX/IP frame ready for [`Tunnel::send_frame`].
fn wrap_cemi(conn: &Tunnel, raw_cemi: Vec<u8>) -> Vec<u8> {
    let request = TunnellingRequest::new(conn.channel_id(), conn.next_sequence(), raw_cemi);
    KnxIpFrame::new(ServiceType::TunnellingRequest, request.serialize()).serialize()
}

/// Hand-assemble an `L_Data.req` CEMI frame addressed to an individual address.
///
/// `tpci` is the single TPCI control octet and `apci` is the APCI/data that
/// follows it. The NPDU length octet counts only the `apci` bytes (the leading
/// TPCI octet is implied), matching `CemiFrame::parse`'s expectation that
/// `tpdu_length = data_length + 1`.
fn build_ldata_req_cemi(
    src: IndividualAddress,
    dst: IndividualAddress,
    tpci: u8,
    apci: &[u8],
) -> Vec<u8> {
    let mut cemi = Vec::with_capacity(10 + apci.len());
    cemi.push(MC_L_DATA_REQ);
    cemi.push(0x00); // no additional info
    cemi.push(CTRL1_STD_NORMAL);
    cemi.push(CTRL2_INDIVIDUAL_HOP6);
    cemi.extend_from_slice(&src.raw().to_be_bytes());
    cemi.extend_from_slice(&dst.raw().to_be_bytes());
    cemi.push(apci.len() as u8); // NPDU length = APCI octets after the TPCI octet
    cemi.push(tpci);
    cemi.extend_from_slice(apci);
    cemi
}

/// Build the `T_Connect` CEMI (TPCI `0x80`, no APCI, NPDU length 0).
fn build_tconnect_cemi(src: IndividualAddress, dst: IndividualAddress) -> Vec<u8> {
    build_ldata_req_cemi(src, dst, TpciFrame::Connect.encode(), &[])
}

/// Build the `T_Disconnect` CEMI (TPCI `0x81`, no APCI, NPDU length 0).
fn build_tdisconnect_cemi(src: IndividualAddress, dst: IndividualAddress) -> Vec<u8> {
    build_ldata_req_cemi(src, dst, TpciFrame::Disconnect.encode(), &[])
}

/// Build the `DeviceDescriptorRead(0)` CEMI carried over
/// `T_Data_Connected(seq=0)`.
///
/// `DeviceDescriptorRead { descriptor: 0 }.encode()` yields `[0x03, 0x00]`; the
/// APCI high bits (`0x03`) are folded into the TPCI octet
/// (`0x40 | 0x03 = 0x43`) and the remaining byte (`0x00`) becomes the APCI
/// payload, so NPDU length is 1.
fn build_ddr_cemi(src: IndividualAddress, dst: IndividualAddress) -> Vec<u8> {
    let ddr = DeviceDescriptorRead { descriptor: 0 }.encode();
    let tpci = TpciFrame::DataConnected { sequence: 0 }.encode() | (ddr[0] & 0x03);
    build_ldata_req_cemi(src, dst, tpci, &ddr[1..])
}

/// Parse a CEMI frame from raw bytes, recovering the TPCI octet for zero-length
/// NPDUs.
///
/// `CemiFrame::parse` does not populate `tpci` when the NPDU length is zero
/// (the serialisation gotcha), which would hide a `T_Disconnect` indication.
/// This helper restores the dropped octet directly from the raw bytes so that
/// [`classify_cemi`] can detect it.
fn parse_probe_cemi(raw: &[u8]) -> Result<CemiFrame> {
    let mut frame = CemiFrame::parse(raw)?;
    if frame.data_length == 0 && frame.tpci == 0 {
        let ai_len = raw[1] as usize;
        let ext = usize::from(frame.extended_control_field.is_some());
        // MC(1) + AI-len octet(1) + AI bytes + control(2) + ext(0/1) + src(2) + dst(2)
        let npdu_len_idx = 2 + ai_len + 2 + ext + 4;
        let tpci_idx = npdu_len_idx + 1;
        if tpci_idx < raw.len() {
            frame.tpci = raw[tpci_idx];
        }
    }
    Ok(frame)
}

/// Classify an incoming CEMI frame as a probe response or unrelated traffic.
///
/// An address is proven occupied by either:
/// * a `DeviceDescriptorResponse` — connected-data TPCI (`tpci & 0x03 == 0x03`)
///   with APCI high bits `0x40` in the first APCI byte (APCI `0x0340`), or
/// * a `T_Disconnect` indication (`TpciFrame::decode(tpci) == Disconnect`).
fn classify_cemi(frame: &CemiFrame) -> ProbeSignal {
    // DeviceDescriptorResponse: APCI 0x0340 over connected data.
    if (frame.tpci & 0x03) == 0x03 && frame.apci_data.first().is_some_and(|b| (b & 0xC0) == 0x40) {
        return ProbeSignal::Occupied;
    }

    // T_Disconnect indication.
    if TpciFrame::decode(frame.tpci) == TpciFrame::Disconnect {
        return ProbeSignal::Occupied;
    }

    ProbeSignal::Other
}

/// Enumerate the reserved auto-select candidate addresses for `area.line`,
/// i.e. `area.line.240..=area.line.254`.
///
/// This is the pure, side-effect-free core of [`auto_select_address`] and is
/// unit-tested independently of any bus I/O.
fn candidate_addresses(area: u8, line: u8) -> impl Iterator<Item = IndividualAddress> {
    (AUTO_SELECT_FIRST_DEVICE..=AUTO_SELECT_LAST_DEVICE)
        .map(move |device| IndividualAddress::new(area, line, device))
}

/// Automatically select a free individual address in the reserved device range
/// `area.line.240..=area.line.254`.
///
/// Iterates the candidate addresses in order. For each candidate:
/// * if [`AddressRegistry::is_available`] is `false` the candidate is skipped
///   without touching the bus (it is already claimed or known-occupied);
/// * otherwise the address is [`probe_address`]d on the bus. A free probe
///   result (`Ok(false)`) yields the selected address; an occupied result
///   (`Ok(true)`) moves on to the next candidate.
///
/// Returns the first free address.
///
/// This function performs **selection only** — it does not claim the returned
/// address in `registry`. The caller is responsible for claiming it.
///
/// # Errors
///
/// Returns [`TransportError::InvalidConfiguration`] if every candidate is
/// unavailable or occupied and the range is exhausted, or if [`probe_address`]
/// fails with a genuine transport error.
pub async fn auto_select_address(
    conn: &Tunnel,
    registry: &AddressRegistry,
    area: u8,
    line: u8,
) -> Result<IndividualAddress> {
    let mut probed_count: usize = 0;

    for addr in candidate_addresses(area, line) {
        // Skip addresses already claimed or known to be occupied without
        // generating any bus traffic.
        if !registry.is_available(addr) {
            log_transport!(
                LogLevel::Debug,
                "Auto-select: skipping {} (not available in registry)",
                addr
            );
            continue;
        }

        probed_count += 1;
        log_transport!(
            LogLevel::Debug,
            "Auto-select: probing candidate {} (attempt {})",
            addr,
            probed_count
        );

        if !probe_address(conn, addr).await? {
            log_transport!(
                LogLevel::Info,
                "Auto-selected address {} (probed {} candidates)",
                addr,
                probed_count
            );
            return Ok(addr);
        }
    }

    Err(TransportError::InvalidConfiguration {
        details: format!(
            "No free individual address available in range {area}.{line}.{AUTO_SELECT_FIRST_DEVICE}..={area}.{line}.{AUTO_SELECT_LAST_DEVICE} (probed {probed_count} candidates)"
        ),
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::{Address, GroupAddress};
    use crate::protocol::cemi::MessageCode;

    fn src() -> IndividualAddress {
        IndividualAddress::broadcast()
    }

    fn dst() -> IndividualAddress {
        IndividualAddress::new(1, 1, 240)
    }

    #[test]
    fn address_probe_tconnect_cemi_layout() {
        let cemi = build_tconnect_cemi(src(), dst());
        // [MC, ai_len, ctrl1, ctrl2, src_hi, src_lo, dst_hi, dst_lo, npdu_len, tpci]
        assert_eq!(cemi.len(), 10);
        assert_eq!(
            cemi[0], MC_L_DATA_REQ,
            "message code must be L_Data.req (0x11)"
        );
        assert_eq!(cemi[8], 0x00, "NPDU length must be 0 for T_Connect");
        assert_eq!(cemi[9], 0x80, "TPCI octet must be T_Connect (0x80)");
        // Destination must be 1.1.240 encoded big-endian.
        assert_eq!(u16::from_be_bytes([cemi[6], cemi[7]]), dst().raw());
    }

    #[test]
    fn address_probe_ddr_cemi_layout() {
        let cemi = build_ddr_cemi(src(), dst());
        // [MC, ai_len, ctrl1, ctrl2, src_hi, src_lo, dst_hi, dst_lo, npdu_len, tpci, apci0]
        assert_eq!(cemi.len(), 11);
        assert_eq!(cemi[0], MC_L_DATA_REQ);
        assert_eq!(
            cemi[8], 0x01,
            "NPDU length must be 1 for DeviceDescriptorRead(0)"
        );
        assert_eq!(
            cemi[9], 0x43,
            "TPCI octet must be T_Data_Connected(0) | APCI hi = 0x43"
        );
        assert_eq!(cemi[10], 0x00, "APCI payload byte must be 0x00");
    }

    #[test]
    fn address_probe_tconnect_cemi_is_unparseable_tpci_but_ddr_round_trips() {
        // The DDR frame (NPDU length 1) round-trips through CemiFrame::parse.
        let ddr = build_ddr_cemi(src(), dst());
        let parsed = CemiFrame::parse(&ddr).unwrap();
        assert_eq!(parsed.tpci, 0x43);
        assert_eq!(parsed.apci_data, vec![0x00]);
    }

    #[test]
    fn address_probe_classify_device_descriptor_response_is_occupied() {
        // Hand-build a DeviceDescriptorResponse CEMI: TPCI 0x43 (connected data,
        // seq 0, APCI hi 0x03) + APCI [0x40, 0x07, 0xB0] (APCI lo 0x40 = 0x0340).
        let raw = build_ldata_req_cemi(src(), dst(), 0x43, &[0x40, 0x07, 0xB0]);
        let frame = parse_probe_cemi(&raw).unwrap();
        assert_eq!(classify_cemi(&frame), ProbeSignal::Occupied);
    }

    #[test]
    fn address_probe_classify_disconnect_is_occupied() {
        // A T_Disconnect indication has NPDU length 0; parse_probe_cemi must
        // recover the dropped TPCI octet (0x81) so it is detected.
        let raw = build_tdisconnect_cemi(src(), dst());
        let frame = parse_probe_cemi(&raw).unwrap();
        assert_eq!(
            frame.tpci, 0x81,
            "TPCI octet must be recovered for zero-length NPDU"
        );
        assert_eq!(classify_cemi(&frame), ProbeSignal::Occupied);
    }

    #[test]
    fn address_probe_classify_disconnect_frame_struct_is_occupied() {
        // classify_cemi is pure: a CemiFrame with tpci 0x81 is occupied.
        let mut frame = CemiFrame::new(
            MessageCode::LDataInd,
            src(),
            Address::Individual(dst()),
            vec![],
        );
        frame.tpci = TpciFrame::Disconnect.encode();
        assert_eq!(classify_cemi(&frame), ProbeSignal::Occupied);
    }

    #[test]
    fn address_probe_classify_group_value_is_other() {
        // An unrelated group-value write (TPCI 0x00) must not be occupied.
        let group = Address::Group(GroupAddress::try_from_raw(0x0801).unwrap());
        let frame = CemiFrame::new(MessageCode::LDataInd, src(), group, vec![0x00, 0x81]);
        assert_eq!(frame.tpci, 0x00);
        assert_eq!(classify_cemi(&frame), ProbeSignal::Other);
    }

    #[test]
    fn address_probe_classify_ack_is_other() {
        // A bare T_ACK (0xC2) must not be misclassified as a descriptor response.
        let mut frame = CemiFrame::new(
            MessageCode::LDataInd,
            src(),
            Address::Individual(dst()),
            vec![],
        );
        frame.tpci = TpciFrame::Ack { sequence: 0 }.encode();
        assert_eq!(classify_cemi(&frame), ProbeSignal::Other);
    }

    #[test]
    fn auto_select_candidate_addresses_cover_240_to_254() {
        let candidates: Vec<IndividualAddress> = candidate_addresses(3, 5).collect();
        // The reserved range x.y.240..=254 is exactly 15 addresses.
        assert_eq!(candidates.len(), 15);
        assert_eq!(
            candidates.first().copied(),
            Some(IndividualAddress::new(3, 5, 240))
        );
        assert_eq!(
            candidates.last().copied(),
            Some(IndividualAddress::new(3, 5, 254))
        );
        // Area and line are preserved for every candidate; devices are contiguous.
        for (offset, addr) in candidates.iter().enumerate() {
            assert_eq!(addr.area(), 3);
            assert_eq!(addr.line(), 5);
            assert_eq!(addr.device(), 240 + offset as u8);
        }
    }

    #[tokio::test]
    async fn auto_select_errors_when_all_candidates_unavailable() {
        // Mark every candidate in the range as known-occupied so is_available()
        // is false for all of them. probe_address is therefore never called and
        // no bus/network I/O occurs, yet the range is exhausted -> error.
        let registry = AddressRegistry::new();
        for addr in candidate_addresses(1, 1) {
            registry.add_known_occupied(addr);
        }

        // A tunnel that is never connected is fine: it must never be used
        // because every candidate is skipped before any probe (no socket I/O).
        let tunnel = Tunnel::new_udp("127.0.0.1:3671".parse().unwrap());

        let result = auto_select_address(&tunnel, &registry, 1, 1).await;
        assert!(result.is_err(), "exhausted range must return an error");
    }
}
