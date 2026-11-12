#![cfg(target_os = "windows")]

use std::borrow::Cow;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use libwebrtc::{
    MediaType,
    audio_frame::AudioFrame,
    audio_source::{AudioSourceOptions, native::NativeAudioSource},
    audio_stream::native::NativeAudioStream,
    media_stream_track::MediaStreamTrack,
    peer_connection::{AnswerOptions, OfferOptions, PeerConnectionState},
    peer_connection_factory::{
        PeerConnectionFactory, RtcConfiguration, native::PeerConnectionFactoryExt,
    },
    rtp_parameters::RtpEncodingParameters,
    rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit},
    session_description::{SdpType, SessionDescription},
    video_source::{VideoResolution, native::NativeVideoSource},
};
use str0m::change::SdpAnswer;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::rtp::RtpWrite;
use str0m::{Candidate, Event, Input, Output, Rtc};
use tokio::sync::mpsc;

#[tokio::test]
async fn native_microphone_and_receive_slot_interoperate_with_sfu() {
    // WebRTC gathers real interface candidates; a loopback-bound Windows socket cannot reach them.
    let route = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    route.connect("192.0.2.1:9").unwrap(); // Route lookup only: no packet is sent.
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    let address = std::net::SocketAddr::new(
        route.local_addr().unwrap().ip(),
        socket.local_addr().unwrap().port(),
    );
    let mut server = Rtc::builder()
        .set_rtp_mode(true)
        .clear_codecs()
        .enable_opus(true, false)
        .build(Instant::now());
    server.add_local_candidate(Candidate::host(address, "udp").unwrap());
    let mut changes = server.sdp_api();
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
    let factory = PeerConnectionFactory::default();
    let pc = factory
        .create_peer_connection(RtcConfiguration::default())
        .unwrap();
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 48000, 1, 0);
    pc.add_track(
        factory
            .create_audio_track("microphone", source.clone())
            .into(),
        &["microphone"],
    )
    .unwrap();
    let (candidates, mut candidate_rx) = mpsc::channel(128);
    pc.on_ice_candidate(Some(Box::new(move |candidate| {
        let _ = candidates.try_send(candidate.candidate());
    })));
    let (tracks, mut track_rx) = mpsc::channel(8);
    pc.on_track(Some(Box::new(move |event| {
        let _ = tracks.try_send(event);
    })));
    pc.set_remote_description(
        SessionDescription::parse(&offer.to_sdp_string(), SdpType::Offer).unwrap(),
    )
    .await
    .unwrap();
    let answer = pc.create_answer(AnswerOptions::default()).await.unwrap();
    let sdp = answer.to_string();
    pc.set_local_description(answer).await.unwrap();
    server
        .sdp_api()
        .accept_answer(pending, SdpAnswer::from_sdp_string(&sdp).unwrap())
        .unwrap();
    let mut sink: Option<NativeAudioStream> = None;
    let mut peak = 0.0f32;
    let mut received = 0;
    let mut buffer = [0u8; 65536];
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let tone: Vec<i16> = (0..480)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 4000.0) as i16)
        .collect();
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            _=tick.tick()=>{let now=Instant::now();server.handle_input(Input::Timeout(now)).unwrap();if pc.connection_state()==PeerConnectionState::Connected{source.capture_frame(&AudioFrame{data:Cow::Borrowed(&tone),sample_rate:48000,num_channels:1,samples_per_channel:480}).await.unwrap();}},
            Ok((len,from))=socket.recv_from(&mut buffer)=>{server.handle_input(Input::Receive(Instant::now(),Receive{proto:Protocol::Udp,source:from,destination:address,contents:(&buffer[..len]).try_into().unwrap()})).unwrap();},
            Some(candidate)=candidate_rx.recv()=>{if let Ok(candidate)=Candidate::from_sdp_string(&candidate) && candidate.addr().is_ipv4(){server.add_remote_candidate(candidate);}},
            Some(event)=track_rx.recv()=>{assert_eq!(event.streams.first().unwrap().id(),"slot-0");if let MediaStreamTrack::Audio(track)=event.track{sink=Some(NativeAudioStream::new(track,48000,1));}},
            Some(frame)=async{match &mut sink{Some(sink)=>sink.next().await,None=>std::future::pending().await}}=>{for sample in frame.data.iter(){peak=peak.max((*sample as f32/32768.0).abs());}},
        }
        loop {
            match server.poll_output().unwrap() {
                Output::Timeout(_) => break,
                Output::Transmit(packet) => {
                    // Failed probes are normal with multiple ICE interfaces. The test still
                    // requires a connected transport, received RTP and audible decoded PCM.
                    let _ = socket.send_to(&packet.contents, packet.destination).await;
                }
                Output::Event(Event::RtpPacket(packet)) => {
                    received += 1;
                    server
                        .direct_api()
                        .stream_tx_by_mid(slot, None)
                        .unwrap()
                        .write_rtp(
                            RtpWrite::new(
                                packet.header.payload_type,
                                packet.seq_no,
                                packet.header.timestamp,
                                Instant::now(),
                                packet.payload,
                            )
                            .marker(packet.header.marker),
                        );
                }
                Output::Event(_) => {}
            }
        }
        if received > 10 && peak > 0.01 {
            break;
        }
    }
    assert_eq!(pc.connection_state(), PeerConnectionState::Connected);
    assert!(received > 10, "Native microphone never reached SFU");
    assert!(peak > 0.01, "Native receive slot was silent");
    pc.close();
}

#[tokio::test]
async fn screen_codecs_offer_three_temporal_layers_and_dependency_descriptor() {
    let factory = PeerConnectionFactory::default();
    for codec in ["video/AV1", "video/VP9"] {
        let pc = factory
            .create_peer_connection(RtcConfiguration::default())
            .unwrap();
        let source = NativeVideoSource::new(
            VideoResolution {
                width: 1280,
                height: 720,
            },
            true,
        );
        let track = factory.create_video_track("screen", source);
        let transceiver = pc
            .add_transceiver(
                track.into(),
                RtpTransceiverInit {
                    direction: RtpTransceiverDirection::SendOnly,
                    stream_ids: vec!["screen".into()],
                    send_encodings: vec![RtpEncodingParameters {
                        scalability_mode: Some("L1T3".into()),
                        ..Default::default()
                    }],
                },
            )
            .unwrap();
        let codecs = factory
            .get_rtp_sender_capabilities(MediaType::Video)
            .codecs
            .into_iter()
            .filter(|c| c.mime_type.eq_ignore_ascii_case(codec))
            .collect::<Vec<_>>();
        assert!(!codecs.is_empty());
        transceiver.set_codec_preferences(codecs).unwrap();
        let offer = pc.create_offer(OfferOptions::default()).await.unwrap();
        let sdp = offer.to_string();
        assert!(
            sdp.contains("dependency-descriptor"),
            "{codec} did not advertise the SFU's SVC header extension"
        );
        pc.set_local_description(offer).await.unwrap();
        let encodings = transceiver.sender().parameters().encodings;
        assert!(
            encodings
                .iter()
                .any(|e| e.scalability_mode.as_deref() == Some("L1T3"))
        );
        pc.close();
    }
}
