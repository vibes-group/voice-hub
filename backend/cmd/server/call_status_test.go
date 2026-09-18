package main

import (
	"testing"

	"voice-hub/backend/internal/sfu/protocol"
)

func TestCallInProgress(t *testing.T) {
	lurker := protocol.PeerInfo{ID: "l", ChatOnly: true}
	speaker := protocol.PeerInfo{ID: "s"}

	for _, tc := range []struct {
		name  string
		peers []protocol.PeerInfo
		want  bool
	}{
		{name: "empty room"},
		{name: "one alone", peers: []protocol.PeerInfo{speaker}},
		{name: "one and a crowd of lurkers", peers: []protocol.PeerInfo{speaker, lurker, lurker, lurker, lurker, lurker}},
		{name: "lurkers only", peers: []protocol.PeerInfo{lurker, lurker}},
		{name: "two talking", peers: []protocol.PeerInfo{speaker, speaker}, want: true},
		{name: "two talking among lurkers", peers: []protocol.PeerInfo{lurker, speaker, lurker, speaker}, want: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := callInProgress(tc.peers); got != tc.want {
				t.Fatalf("callInProgress = %v, want %v", got, tc.want)
			}
		})
	}
}
