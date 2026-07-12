package sfu

import (
	"testing"

	"github.com/pion/rtp"
)

func audioRTPWithExtensions() []byte {
	pkt := &rtp.Packet{}
	pkt.Version = 2
	pkt.PayloadType = 111
	pkt.SequenceNumber = 1
	pkt.SSRC = 0xDEADBEEF
	_ = pkt.Header.SetExtension(1, []byte{0xAB})
	_ = pkt.Header.SetExtension(3, []byte{0x01, 0x02})
	pkt.Payload = make([]byte, 160)
	b, _ := pkt.Marshal()
	return b
}

func BenchmarkOnTrackExtensionStrip_Nil(b *testing.B) {
	buf := audioRTPWithExtensions()
	pkt := &rtp.Packet{}
	b.ReportAllocs()
	for b.Loop() {
		if err := pkt.Unmarshal(buf); err != nil {
			b.Fatal(err)
		}
		pkt.Extension = false
		pkt.Extensions = nil
	}
}

func BenchmarkOnTrackExtensionStrip_Retain(b *testing.B) {
	buf := audioRTPWithExtensions()
	pkt := &rtp.Packet{}
	b.ReportAllocs()
	for b.Loop() {
		if err := pkt.Unmarshal(buf); err != nil {
			b.Fatal(err)
		}
		pkt.Extension = false
		pkt.Extensions = pkt.Extensions[:0]
	}
}

func TestOnTrackExtensionStripRetainsExtensionBuffer(t *testing.T) {
	buf := audioRTPWithExtensions()
	pkt := &rtp.Packet{}
	if err := pkt.Unmarshal(buf); err != nil {
		t.Fatal(err)
	}
	if cap(pkt.Extensions) == 0 {
		t.Fatal("test packet did not produce an extension buffer")
	}

	pkt.Extension = false
	pkt.Extensions = pkt.Extensions[:0]
	if got := cap(pkt.Extensions); got == 0 {
		t.Fatal("stripping extensions discarded reusable buffer")
	}
	if err := pkt.Unmarshal(buf); err != nil {
		t.Fatal(err)
	}
	if got := len(pkt.Extensions); got != 2 {
		t.Fatalf("decoded extensions = %d, want 2", got)
	}
}
