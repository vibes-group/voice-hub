//! Signaling protocol types. The TypeScript mirror lives in
//! frontend/src/sfu/protocol.ts and is updated by hand.
//!
//! Every message is wrapped in an [`Envelope`]: `{"event": "<name>", "data": <payload>}`.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

#[derive(Serialize, Deserialize)]
pub struct Envelope<'a> {
    pub event: &'a str,
    #[serde(borrow, default, skip_serializing_if = "Option::is_none")]
    pub data: Option<&'a RawValue>,
}

/// Serializes `{"event":..,"data":..}` in one pass.
pub fn envelope<T: Serialize>(event: &str, data: &T) -> String {
    #[derive(Serialize)]
    struct Out<'a, T> {
        event: &'a str,
        data: &'a T,
    }
    serde_json::to_string(&Out { event, data }).expect("serializable payload")
}

/// Discriminates which PeerConnection an offer/answer/candidate targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PcKind {
    #[serde(rename = "audio")]
    Audio,
    #[serde(rename = "screen-pub")]
    ScreenPub,
    #[serde(rename = "screen-sub")]
    ScreenSub,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenVideoCodec {
    Av1,
    Vp9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScreenShareMode {
    #[default]
    Sharp,
    Motion,
}

/// Lenient mode field: unknown strings fall back to sharp at the call site,
/// so they must not fail the whole message.
fn de_mode<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<ScreenShareMode>, D::Error> {
    let s = Option::<String>::deserialize(d)?;
    Ok(match s.as_deref() {
        Some("sharp") => Some(ScreenShareMode::Sharp),
        Some("motion") => Some(ScreenShareMode::Motion),
        _ => None,
    })
}

/// `offer` / `answer` data: `{pc, publisherId?, type, sdp}`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SdpEnvelope {
    pub pc: PcKind,
    #[serde(rename = "publisherId", default, skip_serializing_if = "String::is_empty")]
    pub publisher_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub sdp: String,
}

/// `candidate` data: the browser's RTCIceCandidateInit plus routing fields.
#[derive(Debug, Serialize, Deserialize)]
pub struct CandidateEnvelope {
    pub pc: PcKind,
    #[serde(rename = "publisherId", default, skip_serializing_if = "String::is_empty")]
    pub publisher_id: String,
    #[serde(default)]
    pub candidate: String,
    #[serde(rename = "sdpMid", default)]
    pub sdp_mid: Option<String>,
    #[serde(rename = "sdpMLineIndex", default)]
    pub sdp_mline_index: Option<u16>,
    #[serde(rename = "usernameFragment", default)]
    pub username_fragment: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PeerInfo {
    pub id: String,
    #[serde(rename = "displayName", skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(rename = "clientId", skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    #[serde(rename = "selfMuted", skip_serializing_if = "is_false")]
    pub self_muted: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub deafened: bool,
    #[serde(rename = "chatOnly", skip_serializing_if = "is_false")]
    pub chat_only: bool,
    #[serde(rename = "screenSharing", skip_serializing_if = "is_false")]
    pub screen_sharing: bool,
    #[serde(rename = "screenSharingHasAudio", skip_serializing_if = "is_false")]
    pub screen_sharing_has_audio: bool,
    #[serde(rename = "screenSharingVideoCodec", skip_serializing_if = "Option::is_none")]
    pub screen_sharing_video_codec: Option<ScreenVideoCodec>,
}

#[derive(Serialize)]
pub struct Welcome<'a> {
    pub id: &'a str,
    pub peers: &'a [PeerInfo],
}

#[derive(Serialize)]
pub struct PeerLeft<'a> {
    pub id: &'a str,
}

/// First client message; a 10-second server timeout applies.
#[derive(Debug, Default, Deserialize)]
pub struct Hello {
    #[serde(rename = "displayName", default)]
    pub display_name: String,
    #[serde(rename = "clientId", default)]
    pub client_id: String,
    #[serde(rename = "chatOnly", default)]
    pub chat_only: bool,
}

/// `audio-slots` (S→C): who now owns each listed audio slot.
/// `slot` is the msid stream ID of an audio m-line; `peerId` null means the
/// slot fell silent. Sent before forwarding into a slot starts, and after
/// the `peer-left` of its previous owner.
#[derive(Serialize)]
pub struct AudioSlots<'a> {
    pub slots: &'a [AudioSlot<'a>],
}

#[derive(Serialize)]
pub struct AudioSlot<'a> {
    pub slot: &'a str,
    #[serde(rename = "peerId")]
    pub peer_id: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct SetDisplayName {
    #[serde(rename = "displayName", default)]
    pub display_name: String,
}

#[derive(Deserialize)]
pub struct SetState {
    #[serde(rename = "selfMuted", default)]
    pub self_muted: bool,
    #[serde(default)]
    pub deafened: bool,
}

#[derive(Serialize)]
pub struct PeerState<'a> {
    pub id: &'a str,
    #[serde(rename = "selfMuted")]
    pub self_muted: bool,
    pub deafened: bool,
}

// --- Presence (cross-room SSE) ---

pub const PRESENCE_SNAPSHOT_EVENT: &str = "presence-snapshot";
pub const PRESENCE_PEER_JOINED_EVENT: &str = "presence-peer-joined";
pub const PRESENCE_PEER_LEFT_EVENT: &str = "presence-peer-left";
pub const PRESENCE_PEER_UPDATED_EVENT: &str = "presence-peer-updated";

#[derive(Serialize)]
pub struct PresenceRoom {
    pub peers: Vec<PeerInfo>,
}

#[derive(Serialize)]
pub struct PresenceSnapshot {
    pub rooms: std::collections::BTreeMap<String, PresenceRoom>,
}

#[derive(Serialize)]
pub struct PresencePeer<'a> {
    pub room: &'a str,
    pub peer: &'a PeerInfo,
}

#[derive(Serialize)]
pub struct PresencePeerLeft<'a> {
    pub room: &'a str,
    pub id: &'a str,
}

// --- Chat ---

/// Limit on the UTF-8 byte length of a chat message's text.
pub const CHAT_MAX_BYTES: usize = 2000;
pub const CHAT_MAX_ATTACHMENTS: usize = 10;

#[derive(Deserialize)]
pub struct PingClient {
    #[serde(default)]
    pub to: String,
}

#[derive(Serialize)]
pub struct PingServer<'a> {
    pub from: &'a str,
    #[serde(rename = "fromName")]
    pub from_name: &'a str,
}

/// Metadata for one attachment; the bytes travel over HTTP. Relayed once the
/// upload ID is validated; only the declared fields are kept.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    #[serde(rename = "uploadId", default)]
    pub upload_id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub height: i64,
    #[serde(rename = "blurThumb", default, skip_serializing_if = "String::is_empty")]
    pub blur_thumb: String,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

#[derive(Deserialize)]
pub struct ChatSend {
    #[serde(default)]
    pub text: String,
    #[serde(rename = "clientMsgId", default)]
    pub client_msg_id: String,
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
}

#[derive(Serialize)]
pub struct Chat<'a> {
    pub id: &'a str,
    pub from: &'a str,
    pub text: &'a str,
    pub ts: i64,
    #[serde(rename = "clientMsgId", skip_serializing_if = "str::is_empty")]
    pub client_msg_id: &'a str,
    #[serde(rename = "senderName", skip_serializing_if = "str::is_empty")]
    pub sender_name: &'a str,
    #[serde(skip_serializing_if = "<[Attachment]>::is_empty")]
    pub attachments: &'a [Attachment],
}

#[derive(Deserialize)]
pub struct ChatDelete {
    #[serde(default)]
    pub id: String,
}

#[derive(Serialize)]
pub struct ChatDeleted<'a> {
    pub id: &'a str,
}

// --- Screen share ---
//
// C→S screen-share-start      publisher offer (second PC) + hasSystemAudio + mode
// S→C screen-share-started    resume token; queued before the answer, so it arrives first
// S→all screen-share-available after the first video packet, not on start
// C→S screen-share-stop       no payload
// S→all screen-share-ended    on stop, publisher PC failure or grace expiry
// C→S screen-share-resume     token; reattaches within the 5 s grace after a WS drop
// C→S screen-share-subscribe / -unsubscribe    per-viewer PC, SFU offers
// S→C screen-share-encode-pause / -resume      dynacast: no viewers → pause all layers
// S→C screen-share-error      reason (+ publisherId when tied to one)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ScreenShareReason {
    #[serde(rename = "not-found")]
    NotFound,
    #[serde(rename = "invalid-token")]
    InvalidToken,
    #[serde(rename = "already-publishing")]
    AlreadyPublishing,
    #[serde(rename = "internal")]
    Internal,
}

#[derive(Deserialize)]
pub struct ScreenShareStart {
    #[serde(default)]
    pub sdp: String,
    #[serde(rename = "hasSystemAudio", default)]
    pub has_system_audio: bool,
    #[serde(default, deserialize_with = "de_mode")]
    pub mode: Option<ScreenShareMode>,
}

#[derive(Deserialize)]
pub struct ScreenShareResume {
    #[serde(rename = "sessionToken", default)]
    pub session_token: String,
}

#[derive(Serialize)]
pub struct ScreenShareStarted<'a> {
    #[serde(rename = "sessionToken")]
    pub session_token: &'a str,
}

#[derive(Serialize)]
pub struct ScreenShareAvailable<'a> {
    #[serde(rename = "publisherId")]
    pub publisher_id: &'a str,
    #[serde(rename = "hasSystemAudio")]
    pub has_system_audio: bool,
    #[serde(rename = "videoCodec", skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<ScreenVideoCodec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ScreenShareMode>,
}

#[derive(Serialize)]
pub struct ScreenShareEnded<'a> {
    #[serde(rename = "publisherId")]
    pub publisher_id: &'a str,
}

#[derive(Serialize)]
pub struct ScreenShareError<'a> {
    #[serde(rename = "publisherId", skip_serializing_if = "str::is_empty")]
    pub publisher_id: &'a str,
    pub reason: ScreenShareReason,
}

#[derive(Deserialize)]
pub struct ScreenShareSubscribe {
    #[serde(rename = "publisherId", default)]
    pub publisher_id: String,
    #[serde(rename = "preferredTemporalLayer", default)]
    pub preferred_temporal_layer: u8,
}

#[derive(Deserialize)]
pub struct ScreenShareUnsubscribe {
    #[serde(rename = "publisherId", default)]
    pub publisher_id: String,
}

#[derive(Deserialize)]
pub struct ScreenShareModeChange {
    #[serde(default, deserialize_with = "de_mode")]
    pub mode: Option<ScreenShareMode>,
}

#[derive(Serialize)]
pub struct ScreenShareModeChanged<'a> {
    #[serde(rename = "publisherId")]
    pub publisher_id: &'a str,
    pub mode: ScreenShareMode,
}

/// Dynacast pause/resume: which temporal layers the publisher may stop/restart.
#[derive(Serialize)]
pub struct ScreenShareEncodeLayers<'a> {
    pub layers: &'a [i32],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_info_omits_defaults() {
        let p = PeerInfo { id: "a".into(), ..Default::default() };
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"id":"a"}"#);
        let p = PeerInfo {
            id: "a".into(),
            display_name: "n".into(),
            client_id: "c".into(),
            self_muted: true,
            deafened: true,
            chat_only: true,
            screen_sharing: true,
            screen_sharing_has_audio: true,
            screen_sharing_video_codec: Some(ScreenVideoCodec::Vp9),
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"id":"a","displayName":"n","clientId":"c","selfMuted":true,"deafened":true,"chatOnly":true,"screenSharing":true,"screenSharingHasAudio":true,"screenSharingVideoCodec":"vp9"}"#
        );
    }

    #[test]
    fn envelopes() {
        assert_eq!(envelope("peer-left", &PeerLeft { id: "x" }), r#"{"event":"peer-left","data":{"id":"x"}}"#);
        let env: Envelope = serde_json::from_str(r#"{"event":"screen-share-stop"}"#).unwrap();
        assert_eq!(env.event, "screen-share-stop");
        assert!(env.data.is_none());
        let sd: SdpEnvelope = serde_json::from_str(r#"{"pc":"audio","type":"answer","sdp":"v=0"}"#).unwrap();
        assert_eq!(sd.pc, PcKind::Audio);
        assert!(serde_json::from_str::<SdpEnvelope>(r#"{"pc":"bogus","type":"answer","sdp":""}"#).is_err());
        let m: ScreenShareModeChange = serde_json::from_str(r#"{"mode":"weird"}"#).unwrap();
        assert!(m.mode.is_none());
    }

    /// Wire-format fixtures shared with the frontend's Vitest suite
    /// (frontend/src/sfu/protocol.test.ts). A diff here is a protocol change.
    fn fixture(name: &str) -> serde_json::Value {
        let path = format!("{}/src/sfu/testdata/{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn same<T: Serialize>(name: &str, v: &T) {
        assert_eq!(serde_json::to_value(v).unwrap(), fixture(name), "fixture {name}");
    }

    fn parses<T: for<'de> Deserialize<'de>>(name: &str) -> T {
        serde_json::from_value(fixture(name)).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
    }

    #[test]
    fn golden_server_to_client() {
        let alice = PeerInfo {
            id: "11223344aabbccdd".into(),
            display_name: "Alice".into(),
            client_id: "cli_alice_uuid".into(),
            ..Default::default()
        };
        let lurker = PeerInfo {
            id: "aabbccddeeff0011".into(),
            display_name: "Lurker".into(),
            client_id: "cli_lurker_uuid".into(),
            chat_only: true,
            ..Default::default()
        };
        let bare = PeerInfo { id: "99887766ffeeddcc".into(), ..Default::default() };
        same("welcome", &Welcome { id: "abc12345def56789", peers: &[alice.clone(), bare] });
        same("welcome-with-lurker", &Welcome { id: "abc12345def56789", peers: &[alice.clone(), lurker.clone()] });
        same("peer-joined", &alice);
        same("peer-joined-lurker", &lurker);
        same("peer-info", &PeerInfo { display_name: "Bob".into(), ..alice });
        same("peer-left", &PeerLeft { id: "11223344aabbccdd" });
        same(
            "audio-slots",
            &AudioSlots {
                slots: &[AudioSlot { slot: "slot-0", peer_id: Some("11223344aabbccdd") }, AudioSlot { slot: "slot-1", peer_id: None }],
            },
        );
        same("peer-state", &PeerState { id: "11223344aabbccdd", self_muted: true, deafened: false });
        let chat = Chat {
            id: "01JXXXXXXXXXXXXXXXXXXXXXXX",
            from: "11223344aabbccdd",
            text: "hello room",
            ts: 1746403200000,
            client_msg_id: "01JXXXXXXXXXXXXXXXXXXXXXXX",
            sender_name: "",
            attachments: &[],
        };
        same("chat", &chat);
        same("chat-with-sender", &Chat { from: "aabbccddeeff0011", text: "I am running late", sender_name: "Bob", ..chat });
        let sdp = |pc, publisher_id: &str, kind: &str| SdpEnvelope {
            pc,
            publisher_id: publisher_id.into(),
            kind: kind.into(),
            sdp: "v=0\r\n".into(),
        };
        same("offer-audio", &sdp(PcKind::Audio, "", "offer"));
        same("answer-screen-sub", &sdp(PcKind::ScreenSub, "11223344aabbccdd", "answer"));
        same("answer-screen-pub", &sdp(PcKind::ScreenPub, "", "answer"));
        let token = "dGVzdC10b2tlbi0zMmJ5dGUtYmFzZTY0XzAwMDAwMA";
        same("screen-share-started", &ScreenShareStarted { session_token: token });
        same(
            "screen-share-available",
            &ScreenShareAvailable {
                publisher_id: "11223344aabbccdd",
                has_system_audio: true,
                video_codec: Some(ScreenVideoCodec::Av1),
                mode: None,
            },
        );
        same("screen-share-ended", &ScreenShareEnded { publisher_id: "11223344aabbccdd" });
        same("screen-share-error", &ScreenShareError { publisher_id: "11223344aabbccdd", reason: ScreenShareReason::AlreadyPublishing });
        same("screen-share-encode-pause", &ScreenShareEncodeLayers { layers: &[0, 1, 2] });
        same("screen-share-encode-resume", &ScreenShareEncodeLayers { layers: &[0, 1, 2] });
    }

    #[test]
    fn golden_client_to_server() {
        let h: Hello = parses("hello");
        assert_eq!((h.display_name.as_str(), h.client_id.as_str(), h.chat_only), ("Alice", "cli_alice_uuid", false));
        assert!(parses::<Hello>("hello-chat-only").chat_only);
        assert_eq!(parses::<SetDisplayName>("set-displayname").display_name, "Bob");
        let st: SetState = parses("set-state");
        assert!(st.self_muted && !st.deafened);
        let cs: ChatSend = parses("chat-send");
        assert_eq!((cs.text.as_str(), cs.client_msg_id.as_str()), ("hello room", "01JXXXXXXXXXXXXXXXXXXXXXXX"));
        let o: SdpEnvelope = parses("offer-screen-pub");
        assert_eq!((o.pc, o.kind.as_str()), (PcKind::ScreenPub, "offer"));
        let c: CandidateEnvelope = parses("candidate-screen-pub");
        assert_eq!((c.pc, c.candidate.as_str(), c.sdp_mid.as_deref()), (PcKind::ScreenPub, "candidate:1 1 UDP 1 1.1.1.1 1 typ host", None));
        same("candidate-screen-pub", &c);
        let s: ScreenShareStart = parses("screen-share-start");
        assert!(s.has_system_audio && s.sdp == "v=0\r\n" && s.mode.is_none());
        assert_eq!(parses::<ScreenShareResume>("screen-share-resume").session_token, "dGVzdC10b2tlbi0zMmJ5dGUtYmFzZTY0XzAwMDAwMA");
        let sub: ScreenShareSubscribe = parses("screen-share-subscribe");
        assert_eq!((sub.publisher_id.as_str(), sub.preferred_temporal_layer), ("11223344aabbccdd", 2));
        assert_eq!(parses::<ScreenShareUnsubscribe>("screen-share-unsubscribe").publisher_id, "11223344aabbccdd");
    }

    #[test]
    fn peer_state_keeps_false_fields() {
        assert_eq!(
            serde_json::to_string(&PeerState { id: "x", self_muted: false, deafened: false }).unwrap(),
            r#"{"id":"x","selfMuted":false,"deafened":false}"#
        );
    }

    #[test]
    fn encode_layers_is_number_array() {
        assert_eq!(serde_json::to_string(&ScreenShareEncodeLayers { layers: &[0, 1, 2] }).unwrap(), r#"{"layers":[0,1,2]}"#);
    }
}
