package sfu

import (
	"github.com/pion/rtcp"
	"github.com/pion/webrtc/v4"
)

// forwardRTCPToPublisher relays PLI/FIR from a subscriber sender to the
// publisher PC, rewriting MediaSSRC to the publisher's video SSRC. When
// onReport is non-nil it is called for every ReceiverReport seen, allowing
// callers to harvest per-subscriber loss stats.
func (r *Room) forwardRTCPToPublisher(
	session *ScreenShareSession,
	sender *webrtc.RTPSender,
	onReport func(*rtcp.ReceiverReport),
) {
	buf := make([]byte, 1500)
	for {
		n, _, err := sender.Read(buf)
		if err != nil {
			return
		}
		pkts, err := rtcp.Unmarshal(buf[:n])
		if err != nil {
			continue
		}
		var forward []rtcp.Packet
		for _, pkt := range pkts {
			switch p := pkt.(type) {
			case *rtcp.PictureLossIndication, *rtcp.FullIntraRequest:
				forward = append(forward, pkt)
			case *rtcp.ReceiverReport:
				if onReport != nil {
					onReport(p)
				}
			}
		}
		if len(forward) == 0 {
			continue
		}
		pubSSRC := session.publisherVideoSSRC.Load()
		if pubSSRC == 0 {
			continue
		}
		for _, pkt := range forward {
			switch p := pkt.(type) {
			case *rtcp.PictureLossIndication:
				p.MediaSSRC = pubSSRC
			case *rtcp.FullIntraRequest:
				p.MediaSSRC = pubSSRC
			}
		}
		_ = session.publisherPC.WriteRTCP(forward)
	}
}

// forwardScreenVideoRTCPToPublisher relays PLI/FIR from a subscriber's video
// sender to the publisher's PC and harvests ReceiverReport.FractionLost into
// sub.lossPerMille for the auto-downgrade loop.
func (r *Room) forwardScreenVideoRTCPToPublisher(session *ScreenShareSession, sub *screenSubscriber, sender *webrtc.RTPSender) {
	rtxSSRC := uint32(sender.GetParameters().Encodings[0].RTX.SSRC)
	r.forwardRTCPToPublisher(session, sender, func(p *rtcp.ReceiverReport) {
		if worst, ok := worstFractionLost(p, rtxSSRC); ok {
			// FractionLost is fixed-point /256 (RFC 3550 §6.4.1) — convert
			// to per-mille for the int comparisons in the decision loop.
			sub.lossPerMille.Store(uint32(worst) * 1000 / 256)
		}
	})
}

// worstFractionLost returns the worst loss across the report blocks of p.
// A compound RR may carry blocks for both video and audio SSRCs (RFC 3550
// §6.4.1); the worst is a conservative bias toward downgrade when any leg is
// hurting. The RTX block is skipped: pion's NACK responder takes an RTX seq#
// for every buffered packet, not every sent one, so that block reports near
// total loss. ok is false when no block is left.
func worstFractionLost(p *rtcp.ReceiverReport, rtxSSRC uint32) (worst uint8, ok bool) {
	for _, rep := range p.Reports {
		if rtxSSRC != 0 && rep.SSRC == rtxSSRC {
			continue
		}
		ok = true
		worst = max(worst, rep.FractionLost)
	}
	return worst, ok
}
