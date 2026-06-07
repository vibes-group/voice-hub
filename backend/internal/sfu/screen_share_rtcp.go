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
	r.forwardRTCPToPublisher(session, sender, func(p *rtcp.ReceiverReport) {
		if len(p.Reports) > 0 {
			// A compound RR may carry blocks for both video and audio SSRCs
			// (RFC 3550 §6.4.1). Take the worst loss across all blocks —
			// conservative bias toward downgrade when any leg is hurting.
			var worst uint8
			for _, rep := range p.Reports {
				if rep.FractionLost > worst {
					worst = rep.FractionLost
				}
			}
			// FractionLost is fixed-point /256 (RFC 3550 §6.4.1) — convert
			// to per-mille for the int comparisons in the decision loop.
			sub.lossPerMille.Store(uint32(worst) * 1000 / 256)
		}
	})
}
