use std::net::SocketAddr;
use std::time::{Duration, Instant};

use str0m::change::SdpAnswer;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::rtp::RtpWrite;
use str0m::{Candidate, Event, Input, Output, Rtc};
use voice_hub_gpui_beta::audio::{FRAME, SAMPLE_RATE};
use voice_hub_gpui_beta::session::Voice;

fn pump(from: &mut Rtc, to: &mut Rtc, now: Instant) -> Vec<Event> {
    let mut events = vec![];
    loop {
        match from.poll_output().unwrap() {
            Output::Timeout(_) => break,
            Output::Event(event) => events.push(event),
            Output::Transmit(packet) => {
                to.handle_input(Input::Receive(
                    now,
                    Receive {
                        proto: Protocol::Udp,
                        source: packet.source,
                        destination: packet.destination,
                        contents: (&packet.contents[..]).try_into().unwrap(),
                    },
                ))
                .unwrap();
            }
        }
    }
    events
}

#[test]
fn microphone_reaches_sfu_and_forwarded_opus_is_audible() {
    let now = Instant::now();
    let server_addr: SocketAddr = "127.0.0.1:51000".parse().unwrap();
    let client_addr: SocketAddr = "127.0.0.1:51001".parse().unwrap();
    let mut server = Rtc::builder()
        .set_rtp_mode(true)
        .clear_codecs()
        .enable_opus(true, false)
        .build(now);
    server.add_local_candidate(Candidate::host(server_addr, "udp").unwrap());
    let mut changes = server.sdp_api();
    // Match the real server: recvonly microphone, sendonly fixed receive slots.
    changes.add_media(
        MediaKind::Audio,
        Direction::RecvOnly,
        Some("microphone".into()),
        None,
        None,
    );
    let slot = changes.add_media(
        MediaKind::Audio,
        Direction::SendOnly,
        Some("slot-0".into()),
        None,
        None,
    );
    let (offer, pending) = changes.apply().unwrap();
    let mut client = Voice::new(client_addr).unwrap();
    let answer = client.answer(&offer.to_sdp_string()).unwrap();
    server
        .sdp_api()
        .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).unwrap())
        .unwrap();

    let tone: [f32; FRAME] = std::array::from_fn(|i| {
        (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.1
    });
    let mut connected = false;
    let mut received_packets = 0;
    let mut peak = 0.0_f32;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        let now = Instant::now();
        server.handle_input(Input::Timeout(now)).unwrap();
        client.rtc.handle_input(Input::Timeout(now)).unwrap();
        if connected {
            client.encode(&tone).unwrap();
        }
        for _ in 0..4 {
            for event in pump(&mut server, &mut client.rtc, now) {
                if let Event::RtpPacket(packet) = event {
                    received_packets += 1;
                    server
                        .direct_api()
                        .stream_tx_by_mid(slot, None)
                        .unwrap()
                        .write_rtp(
                            RtpWrite::new(
                                packet.header.payload_type,
                                packet.seq_no,
                                packet.header.timestamp,
                                now,
                                packet.payload,
                            )
                            .marker(packet.header.marker),
                        );
                }
            }
            for event in pump(&mut client.rtc, &mut server, now) {
                if matches!(event, Event::Connected) {
                    connected = true;
                }
                client.event(event, false).unwrap();
            }
        }
        for sample in client.mixer.render() {
            peak = peak.max(sample.abs());
        }
        if received_packets > 10 && peak > 0.01 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(connected, "ICE/DTLS handshake failed");
    assert!(received_packets > 10, "microphone Opus never reached SFU");
    assert!(peak > 0.01, "forwarded Opus was not decoded and mixed");
}
