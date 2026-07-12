package sfu

import (
	"context"
	"testing"

	"github.com/pion/webrtc/v4"
)

func newSignalingTestPeer(t *testing.T) *peer {
	t.Helper()
	pc, err := webrtc.NewPeerConnection(webrtc.Configuration{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = pc.Close() })
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	return &peer{
		id:     "peer",
		pc:     pc,
		out:    make(chan []byte, 4),
		ctx:    ctx,
		cancel: cancel,
	}
}

func TestSyncOnePeerSendsInitialOffer(t *testing.T) {
	p := newSignalingTestPeer(t)
	r := &Room{}
	if retry := r.syncOnePeer(p, nil); retry {
		t.Fatal("initial sync requested retry")
	}
	if len(p.out) != 1 {
		t.Fatalf("initial offers = %d, want 1", len(p.out))
	}
	if !p.syncInitialized {
		t.Fatal("peer was not marked initialized")
	}
}

func TestSyncOnePeerSkipsUnchangedInitializedPeer(t *testing.T) {
	p := newSignalingTestPeer(t)
	p.syncInitialized = true
	r := &Room{}
	if retry := r.syncOnePeer(p, nil); retry {
		t.Fatal("unchanged sync requested retry")
	}
	if len(p.out) != 0 {
		t.Fatalf("unchanged offers = %d, want 0", len(p.out))
	}
	if got := p.pc.SignalingState(); got != webrtc.SignalingStateStable {
		t.Fatalf("signaling state = %s, want stable", got)
	}
}
