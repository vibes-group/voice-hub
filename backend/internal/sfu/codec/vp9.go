package codec

import (
	"errors"

	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"

	"voice-hub/backend/internal/sfu/dd"
	"voice-hub/backend/internal/sfu/protocol"
)

// vp9 parses the VP9 RTP payload format (RFC 8741) into a dd.Descriptor.
// VP9 carries no chain concept, so ChainDiffs is always empty — the SFU's
// ChainTracker degrades to a pure keyframe-gated bootstrap check, which is
// the most we can enforce without an upstream signal. Temporal-layer drops
// still work because TID is in the VP9 RTP descriptor when L=1
// (scalability indicator present).
//
// FrameNumber is the 7- or 15-bit VP9 picture ID when present, otherwise 0.
// The forwarder's ChainTracker only uses it for the "seen" ring; since
// ChainDiffs is empty the ring is never queried, so a stuck 0 is harmless.
type vp9 struct{}

func newVP9() *vp9 { return &vp9{} }

func (*vp9) Name() protocol.ScreenVideoCodec       { return protocol.ScreenVideoCodecVP9 }
func (*vp9) SupportsTemporalFiltering() bool       { return true }
func (*vp9) OnTrackNegotiated(*webrtc.RTPReceiver) {}

var errVP9ShortPayload = errors.New("vp9: payload too short")

// Parse decodes the VP9 RTP descriptor at the head of pkt.Payload. Layout
// (first byte): I P L F B E V Z. Subsequent fields are present per the
// flag bits. We only consume what the forwarder needs (picture ID, TID,
// SID, keyframe flag, last-in-frame flag).
func (*vp9) Parse(pkt *rtp.Packet) (*dd.Descriptor, error) {
	p := pkt.Payload
	if len(p) < 1 {
		return nil, errVP9ShortPayload
	}
	head := p[0]
	iBit := head&0x80 != 0
	pBit := head&0x40 != 0
	lBit := head&0x20 != 0
	fBit := head&0x10 != 0
	bBit := head&0x08 != 0
	eBit := head&0x04 != 0

	off := 1
	var pictureID uint16
	if iBit {
		if off >= len(p) {
			return nil, errVP9ShortPayload
		}
		if p[off]&0x80 != 0 {
			if off+1 >= len(p) {
				return nil, errVP9ShortPayload
			}
			pictureID = (uint16(p[off]&0x7f) << 8) | uint16(p[off+1])
			off += 2
		} else {
			pictureID = uint16(p[off] & 0x7f)
			off++
		}
	}
	var tid, sid uint8
	if lBit {
		if off >= len(p) {
			return nil, errVP9ShortPayload
		}
		layers := p[off]
		tid = (layers >> 5) & 0x07
		sid = (layers >> 1) & 0x07
		off++
		if !fBit {
			if off >= len(p) {
				return nil, errVP9ShortPayload
			}
			off++
		}
	}

	// Keyframe markers in VP9 RTP: !P (not inter-predicted) AND B
	// (start of frame). The B gate excludes intermediate packets of the
	// same keyframe from re-arming the tracker; those packets will be
	// admitted via the post-keyframe permissive branch in ChainTracker.
	return &dd.Descriptor{
		FrameNumber:   pictureID,
		TemporalLayer: tid,
		SpatialLayer:  sid,
		IsKeyframe:    !pBit && bBit,
		IsLastInFrame: eBit,
	}, nil
}
