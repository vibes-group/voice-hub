//! str0m `Rtc` construction: codecs, header extensions and ICE setup shared
//! by every PeerConnection the SFU creates.

use std::time::{Duration, Instant};

use str0m::format::{Codec, CodecConfig, CodecSpec, FormatParams, PayloadParams};
use str0m::media::{Frequency, Pt};
use str0m::rtp::{Extension, ExtensionSerializer, ExtensionValues};
use str0m::{Candidate, IceCreds, Rtc, RtcConfig};

use super::dd;
use super::net::MuxSocket;
use crate::util::{hex, random_bytes};

pub const PT_OPUS: u8 = 111;
pub const PT_AV1: u8 = 45;
pub const PT_AV1_RTX: u8 = 46;
pub const PT_VP9: u8 = 98;
pub const PT_VP9_RTX: u8 = 99;

/// Free slot in str0m's standard extension map.
const DD_EXT_ID: u8 = 12;
/// MID's ID in str0m's standard extension map.
const MID_EXT_ID: u8 = 4;

/// NACK history per outgoing video stream: well over one RTT of packets.
const VIDEO_SEND_BUFFER: usize = 256;

/// Raw DD extension bytes, parsed later by the session's stateful parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdBytes(pub Vec<u8>);

#[derive(Debug)]
struct DdExt;

impl ExtensionSerializer for DdExt {
    // Never written: forwarded packets drop publisher extensions.
    fn write_to(&self, _buf: &mut [u8], _ev: &ExtensionValues) -> usize {
        0
    }

    fn parse_value(&self, buf: &[u8], ev: &mut ExtensionValues) -> bool {
        ev.user_values.set(DdBytes(buf.to_vec()));
        true
    }

    fn is_video(&self) -> bool {
        true
    }

    fn is_audio(&self) -> bool {
        false
    }
}

/// Builds an `Rtc` in RTP mode with the SFU's codec set and a host
/// candidate on `socket`. Returns it with its local ICE ufrag.
pub fn build(socket: &MuxSocket, stats: bool) -> (Rtc, String) {
    build_cfg(socket, stats, true, false)
}

/// An audio PC negotiates MID only: it runs no bandwidth estimation and
/// forwarded packets drop publisher extensions, so transport-wide CC and
/// audio level would cost per-packet bookkeeping and feedback for nothing.
pub fn build_audio(socket: &MuxSocket) -> (Rtc, String) {
    build_cfg(socket, false, true, true)
}

/// `av1 = false` leaves VP9 as the only video codec (load clients).
pub fn build_with(socket: &MuxSocket, stats: bool, av1: bool) -> (Rtc, String) {
    build_cfg(socket, stats, av1, false)
}

fn build_cfg(socket: &MuxSocket, stats: bool, av1: bool, audio_only: bool) -> (Rtc, String) {
    let ufrag = hex(&random_bytes::<4>());
    let creds = IceCreds { ufrag: ufrag.clone(), pass: hex(&random_bytes::<16>()) };
    let mut cfg = RtcConfig::new()
        .set_rtp_mode(true)
        .set_local_ice_credentials(creds)
        .clear_codecs()
        .set_send_buffer_video(VIDEO_SEND_BUFFER)
        .set_extension(DD_EXT_ID, Extension::with_serializer(dd::RTP_EXTENSION_URI, DdExt));
    if stats {
        cfg = cfg.set_stats_interval(Some(Duration::from_secs(1)));
    }
    if audio_only {
        cfg = cfg.clear_extension_map().set_extension(MID_EXT_ID, Extension::RtpMid);
    }
    let spec = |codec, clock_rate, channels, format| CodecSpec { codec, clock_rate, channels, format };
    let mut params = vec![
        PayloadParams::new(
            Pt::new_with_value(PT_OPUS),
            None,
            spec(
                Codec::Opus,
                Frequency::FORTY_EIGHT_KHZ,
                Some(2),
                FormatParams {
                    min_p_time: Some(10),
                    use_inband_fec: Some(true),
                    use_dtx: Some(true),
                    stereo: Some(false),
                    ..Default::default()
                },
            ),
        ),
        // AV1 stays preferred by client-side codec ordering; VP9 is the fallback.
        PayloadParams::new(
            Pt::new_with_value(PT_AV1),
            Some(Pt::new_with_value(PT_AV1_RTX)),
            spec(
                Codec::Av1,
                Frequency::NINETY_KHZ,
                None,
                FormatParams { level_idx: Some(5), profile: Some(0), tier: Some(0), ..Default::default() },
            ),
        ),
        PayloadParams::new(
            Pt::new_with_value(PT_VP9),
            Some(Pt::new_with_value(PT_VP9_RTX)),
            spec(Codec::Vp9, Frequency::NINETY_KHZ, None, FormatParams::default()),
        ),
    ];
    if !av1 {
        params.retain(|p| p.spec().codec != Codec::Av1);
    }
    // Browsers use PLI; a forwarded FIR would be ignored (only MediaSSRC is
    // rewritten), so FIR is not offered.
    for p in &mut params {
        p.set_fb_fir(false);
        p.set_fb_remb(false);
    }
    *cfg.codec_config() = CodecConfig::new_from_payload_params(params);
    let mut rtc = cfg.build(Instant::now());
    let cand = Candidate::host(socket.public_addr, "udp").expect("valid host candidate");
    rtc.add_local_candidate(cand);
    (rtc, ufrag)
}

pub fn codec_of_pt(rtc: &Rtc, pt: Pt) -> Option<Codec> {
    rtc.codec_config().params().iter().find(|p| p.pt() == pt).map(|p| p.spec().codec)
}
