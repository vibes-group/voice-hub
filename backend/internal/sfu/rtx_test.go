package sfu

import (
	"bytes"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/pion/interceptor"
	"github.com/pion/interceptor/pkg/nack"
	"github.com/pion/rtcp"
	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"
)

func TestForwardable(t *testing.T) {
	rtx := interceptor.Attributes{}
	rtx.Set(webrtc.AttributeRtxSsrc, uint32(1234))
	for _, tc := range []struct {
		name  string
		pkt   *rtp.Packet
		attrs interceptor.Attributes
		want  bool
	}{
		{"media", &rtp.Packet{Payload: []byte{1}}, nil, true},
		{"media with attrs", &rtp.Packet{Payload: []byte{1}}, interceptor.Attributes{}, true},
		{"padding only", &rtp.Packet{Header: rtp.Header{Padding: true, PaddingSize: 200}}, nil, false},
		{"from RTX", &rtp.Packet{Payload: []byte{1}}, rtx, false},
	} {
		if got := forwardable(tc.pkt, tc.attrs); got != tc.want {
			t.Errorf("%s: forwardable = %v, want %v", tc.name, got, tc.want)
		}
	}
}

func TestWorstFractionLostSkipsRTX(t *testing.T) {
	rr := func(blocks ...rtcp.ReceptionReport) *rtcp.ReceiverReport {
		return &rtcp.ReceiverReport{Reports: blocks}
	}
	for _, tc := range []struct {
		name   string
		rr     *rtcp.ReceiverReport
		rtx    uint32
		want   uint8
		wantOK bool
	}{
		{"empty", rr(), 9, 0, false},
		{"only RTX", rr(rtcp.ReceptionReport{SSRC: 9, FractionLost: 128}), 9, 0, false},
		{"worst of media", rr(
			rtcp.ReceptionReport{SSRC: 1, FractionLost: 10},
			rtcp.ReceptionReport{SSRC: 9, FractionLost: 200},
			rtcp.ReceptionReport{SSRC: 2, FractionLost: 30},
		), 9, 30, true},
		{"no RTX negotiated", rr(rtcp.ReceptionReport{SSRC: 0, FractionLost: 5}), 0, 5, true},
	} {
		got, ok := worstFractionLost(tc.rr, tc.rtx)
		if got != tc.want || ok != tc.wantOK {
			t.Errorf("%s: got (%d, %v), want (%d, %v)", tc.name, got, ok, tc.want, tc.wantOK)
		}
	}
}

var testVP9 = webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP9, ClockRate: 90000, SDPFmtpLine: "profile-id=0"}

type readPacket struct {
	pkt   *rtp.Packet
	attrs interceptor.Attributes
}

// newPeerAPI mimics a browser: default codecs (with RTX) plus the given
// interceptors, nothing else.
func newPeerAPI(t *testing.T, factories ...interceptor.Factory) *webrtc.API {
	t.Helper()
	m := &webrtc.MediaEngine{}
	if err := m.RegisterDefaultCodecs(); err != nil {
		t.Fatal(err)
	}
	ir := &interceptor.Registry{}
	for _, f := range factories {
		ir.Add(f)
	}
	return webrtc.NewAPI(webrtc.WithMediaEngine(m), webrtc.WithInterceptorRegistry(ir))
}

func newPC(t *testing.T, api *webrtc.API) *webrtc.PeerConnection {
	t.Helper()
	pc, err := api.NewPeerConnection(webrtc.Configuration{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = pc.Close() })
	return pc
}

// connectPCs runs a non-trickle offer/answer and returns the offer SDP.
func connectPCs(t *testing.T, offerer, answerer *webrtc.PeerConnection) string {
	t.Helper()
	offer, err := offerer.CreateOffer(nil)
	if err != nil {
		t.Fatal(err)
	}
	gathered := webrtc.GatheringCompletePromise(offerer)
	if err = offerer.SetLocalDescription(offer); err != nil {
		t.Fatal(err)
	}
	<-gathered
	if err = answerer.SetRemoteDescription(*offerer.LocalDescription()); err != nil {
		t.Fatal(err)
	}
	answer, err := answerer.CreateAnswer(nil)
	if err != nil {
		t.Fatal(err)
	}
	gathered = webrtc.GatheringCompletePromise(answerer)
	if err = answerer.SetLocalDescription(answer); err != nil {
		t.Fatal(err)
	}
	<-gathered
	if err = offerer.SetRemoteDescription(*answerer.LocalDescription()); err != nil {
		t.Fatal(err)
	}
	return offerer.LocalDescription().SDP
}

// waitConnected blocks until every pc is connected, so test packets are not
// lost to the DTLS handshake.
func waitConnected(t *testing.T, pcs ...*webrtc.PeerConnection) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for _, pc := range pcs {
		for pc.ConnectionState() != webrtc.PeerConnectionStateConnected {
			if time.Now().After(deadline) {
				t.Fatal("not connected")
			}
			time.Sleep(10 * time.Millisecond)
		}
	}
}

func testPayload(seq uint16) []byte { return []byte{byte(seq >> 8), byte(seq), 0xAA, 0xBB, 0xCC} }

func videoPacket(seq uint16) *rtp.Packet {
	return &rtp.Packet{
		Header:  rtp.Header{Version: 2, SequenceNumber: seq, Timestamp: uint32(seq) * 3000, Marker: true},
		Payload: testPayload(seq),
	}
}

// addVideoTrack adds a VP9 track to pc and keeps its sender's RTCP flowing
// through the interceptors (the NACK responder only sees what somebody reads).
func addVideoTrack(t *testing.T, pc *webrtc.PeerConnection) (*webrtc.TrackLocalStaticRTP, *webrtc.RTPSender) {
	t.Helper()
	track, err := webrtc.NewTrackLocalStaticRTP(testVP9, "video", "pub")
	if err != nil {
		t.Fatal(err)
	}
	sender, err := pc.AddTrack(track)
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		for {
			if _, _, err := sender.ReadRTCP(); err != nil {
				return
			}
		}
	}()
	return track, sender
}

// sendVideo writes numbered packets until the test ends; the packet with
// sequence number padSeq (if non-zero) is padding only.
func sendVideo(t *testing.T, track *webrtc.TrackLocalStaticRTP, padSeq uint16) {
	stop := make(chan struct{})
	done := make(chan struct{})
	t.Cleanup(func() { close(stop); <-done })
	go func() {
		defer close(done)
		tick := time.NewTicker(5 * time.Millisecond)
		defer tick.Stop()
		for seq := uint16(1); ; seq++ {
			select {
			case <-stop:
				return
			case <-tick.C:
			}
			pkt := videoPacket(seq)
			if seq == padSeq {
				pkt.Padding, pkt.Header.PaddingSize, pkt.Payload = true, 100, nil
			}
			_ = track.WriteRTP(pkt)
		}
	}()
}

func readTrack(pc *webrtc.PeerConnection) (<-chan *webrtc.TrackRemote, <-chan readPacket) {
	tracks := make(chan *webrtc.TrackRemote, 1)
	pkts := make(chan readPacket, 1024)
	pc.OnTrack(func(tr *webrtc.TrackRemote, _ *webrtc.RTPReceiver) {
		tracks <- tr
		for {
			pkt, attrs, err := tr.ReadRTP()
			if err != nil {
				return
			}
			select {
			case pkts <- readPacket{pkt, attrs}:
			default:
			}
		}
	})
	return tracks, pkts
}

func waitTrack(t *testing.T, tracks <-chan *webrtc.TrackRemote) *webrtc.TrackRemote {
	t.Helper()
	select {
	case tr := <-tracks:
		return tr
	case <-time.After(10 * time.Second):
		t.Fatal("no remote track")
		return nil
	}
}

func waitPacket(t *testing.T, pkts <-chan readPacket, what string, match func(readPacket) bool) readPacket {
	t.Helper()
	deadline := time.After(10 * time.Second)
	for {
		select {
		case p := <-pkts:
			if match(p) {
				return p
			}
		case <-deadline:
			t.Fatalf("timed out waiting for %s", what)
		}
	}
}

func nackFor(mediaSSRC webrtc.SSRC, seqs ...uint16) []rtcp.Packet {
	return []rtcp.Packet{&rtcp.TransportLayerNack{
		SenderSSRC: 1,
		MediaSSRC:  uint32(mediaSSRC),
		Nacks:      rtcp.NackPairsFromSequenceNumbers(seqs),
	}}
}

// A subscriber's NACK is answered on the RTX SSRC with the RTX payload type
// and the original sequence number in the OSN field.
func TestSubscriberNACKAnsweredOnRTX(t *testing.T) {
	r, err := NewRoom(Config{})
	if err != nil {
		t.Fatal(err)
	}
	sfu := newPC(t, r.api)
	sub := newPC(t, newPeerAPI(t))
	track, sender := addVideoTrack(t, sfu)
	tracks, pkts := readTrack(sub)

	offer := connectPCs(t, sfu, sub)
	waitConnected(t, sfu, sub)
	enc := sender.GetParameters().Encodings[0]
	if enc.RTX.SSRC == 0 {
		t.Fatal("sender has no RTX SSRC")
	}
	if want := "a=ssrc-group:FID " + ssrcString(enc.SSRC) + " " + ssrcString(enc.RTX.SSRC); !strings.Contains(offer, want) {
		t.Fatalf("offer lacks %q:\n%s", want, offer)
	}

	sendVideo(t, track, 0)
	remote := waitTrack(t, tracks)
	waitPacket(t, pkts, "seq 20", func(p readPacket) bool { return p.pkt.SequenceNumber >= 20 })
	if err := sub.WriteRTCP(nackFor(remote.SSRC(), 5)); err != nil {
		t.Fatal(err)
	}

	got := waitPacket(t, pkts, "RTX retransmit", func(p readPacket) bool {
		return p.attrs.Get(webrtc.AttributeRtxSsrc) != nil
	})
	if ssrc := got.attrs.Get(webrtc.AttributeRtxSsrc); ssrc != uint32(enc.RTX.SSRC) {
		t.Errorf("RTX SSRC = %v, want %d", ssrc, enc.RTX.SSRC)
	}
	rtxPT := got.attrs.Get(webrtc.AttributeRtxPayloadType)
	if pt, ok := rtxPT.(uint8); !ok || webrtc.PayloadType(pt) == remote.PayloadType() {
		t.Errorf("RTX payload type = %v, media payload type %d", rtxPT, remote.PayloadType())
	}
	if got.pkt.SequenceNumber != 5 || !bytes.Equal(got.pkt.Payload, testPayload(5)) {
		t.Errorf("retransmit seq %d payload %x, want seq 5 payload %x",
			got.pkt.SequenceNumber, got.pkt.Payload, testPayload(5))
	}
}

// Retransmits must not reach the primary stream's sender report: RTX
// packets carry their own seq# and an old timestamp, and counting them there
// skews PacketCount and the SR RTP time receivers use for A/V sync.
func TestRTXNotCountedInSenderReport(t *testing.T) {
	r, err := NewRoom(Config{})
	if err != nil {
		t.Fatal(err)
	}
	sfu := newPC(t, r.api)
	sub := newPC(t, newPeerAPI(t))
	track, sender := addVideoTrack(t, sfu)

	reports := make(chan *rtcp.SenderReport, 16)
	rtxSeen := make(chan struct{}, 1)
	sub.OnTrack(func(tr *webrtc.TrackRemote, recv *webrtc.RTPReceiver) {
		go func() {
			for {
				pkts, _, err := recv.ReadRTCP()
				if err != nil {
					return
				}
				for _, p := range pkts {
					if sr, ok := p.(*rtcp.SenderReport); ok {
						select {
						case reports <- sr:
						default:
						}
					}
				}
			}
		}()
		for {
			_, attrs, err := tr.ReadRTP()
			if err != nil {
				return
			}
			if attrs.Get(webrtc.AttributeRtxSsrc) != nil {
				select {
				case rtxSeen <- struct{}{}:
				default:
				}
			}
		}
	})
	connectPCs(t, sfu, sub)

	write := func(seq uint16) {
		if err := track.WriteRTP(videoPacket(seq)); err != nil {
			t.Fatal(err)
		}
	}
	waitConnected(t, sfu, sub)
	const sent = 100
	for seq := uint16(1); seq <= sent; seq++ {
		write(seq)
		time.Sleep(time.Millisecond)
	}
	seqs := make([]uint16, 0, 20)
	for seq := uint16(10); seq < 30; seq++ {
		seqs = append(seqs, seq)
	}
	if err := sub.WriteRTCP(nackFor(sender.GetParameters().Encodings[0].SSRC, seqs...)); err != nil {
		t.Fatal(err)
	}
	// ReadRTP surfaces queued RTX packets only once a media packet arrives,
	// so keep media flowing until one shows up.
	written := uint16(sent)
	deadline := time.After(5 * time.Second)
	for seen := false; !seen; {
		written++
		write(written)
		select {
		case <-rtxSeen:
			seen = true
		case <-time.After(20 * time.Millisecond):
		case <-deadline:
			t.Fatal("no retransmit reached the subscriber")
		}
	}
	time.Sleep(100 * time.Millisecond)
	for len(reports) > 0 {
		<-reports
	}
	select {
	case sr := <-reports:
		if sr.PacketCount != uint32(written) {
			t.Errorf("SR PacketCount = %d, want %d (retransmits leaked into the media SR)", sr.PacketCount, written)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("no sender report")
	}
}

// What a publisher sends on its RTX SSRC comes out of ReadRTP unwrapped
// (original seq#, media PT and SSRC) and is only recognisable by the rtx_*
// attributes; forwardable must reject it, as well as padding-only probes.
func TestPublisherRTXAndPaddingNotForwardable(t *testing.T) {
	r, err := NewRoom(Config{})
	if err != nil {
		t.Fatal(err)
	}
	responder, err := nack.NewResponderInterceptor()
	if err != nil {
		t.Fatal(err)
	}
	pub := newPC(t, newPeerAPI(t, responder))
	sfu := newPC(t, r.api)
	track, sender := addVideoTrack(t, pub)
	tracks, pkts := readTrack(sfu)
	connectPCs(t, pub, sfu)
	waitConnected(t, pub, sfu)
	if sender.GetParameters().Encodings[0].RTX.SSRC == 0 {
		t.Fatal("publisher has no RTX SSRC")
	}

	const padSeq = 15
	sendVideo(t, track, padSeq)
	remote := waitTrack(t, tracks)

	pad := waitPacket(t, pkts, "padding-only packet", func(p readPacket) bool { return p.pkt.SequenceNumber == padSeq })
	if forwardable(pad.pkt, pad.attrs) {
		t.Errorf("padding-only packet is forwardable: %+v", pad.pkt.Header)
	}
	media := waitPacket(t, pkts, "seq 20", func(p readPacket) bool { return p.pkt.SequenceNumber >= 20 })
	if !forwardable(media.pkt, media.attrs) {
		t.Errorf("media packet not forwardable: %+v", media.pkt.Header)
	}

	if err := sfu.WriteRTCP(nackFor(remote.SSRC(), 5)); err != nil {
		t.Fatal(err)
	}
	got := waitPacket(t, pkts, "unwrapped RTX", func(p readPacket) bool { return p.pkt.SequenceNumber == 5 })
	if got.attrs.Get(webrtc.AttributeRtxSsrc) == nil {
		t.Fatalf("seq 5 came back without rtx attributes: %v", got.attrs)
	}
	if got.pkt.SSRC != uint32(remote.SSRC()) || webrtc.PayloadType(got.pkt.PayloadType) != remote.PayloadType() ||
		!bytes.Equal(got.pkt.Payload, testPayload(5)) {
		t.Errorf("RTX not unwrapped to media form: ssrc %d pt %d payload %x", got.pkt.SSRC, got.pkt.PayloadType, got.pkt.Payload)
	}
	if forwardable(got.pkt, got.attrs) {
		t.Error("unwrapped RTX packet is forwardable")
	}
}

func ssrcString(s webrtc.SSRC) string { return strconv.FormatUint(uint64(s), 10) }
