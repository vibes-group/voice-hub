package presence

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"voice-hub/backend/internal/sfu/protocol"
)

// readSSEFrame reads one SSE event frame from r, returning the event name and
// raw data bytes. Returns io.EOF when the stream ends.
func readSSEFrame(r *bufio.Reader) (event string, data []byte, err error) {
	for {
		line, err := r.ReadString('\n')
		if err != nil {
			return "", nil, err
		}
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			if event != "" || data != nil {
				return event, data, nil
			}
			continue
		}
		if len(line) > 0 && line[0] == ':' {
			// SSE comment (e.g. ": heartbeat") — skip
			continue
		}
		field, value, _ := strings.Cut(line, ": ")
		switch field {
		case "event":
			event = value
		case "data":
			data = []byte(value)
		}
	}
}

type fakeRoom struct {
	mu    sync.Mutex
	peers []protocol.PeerInfo
}

type countingRoom struct {
	peers []protocol.PeerInfo
	calls int
}

func (f *countingRoom) Peers() []protocol.PeerInfo {
	f.calls++
	return f.peers
}

func (f *fakeRoom) Peers() []protocol.PeerInfo {
	f.mu.Lock()
	defer f.mu.Unlock()
	out := make([]protocol.PeerInfo, len(f.peers))
	copy(out, f.peers)
	return out
}

func (f *fakeRoom) set(peers []protocol.PeerInfo) {
	f.mu.Lock()
	f.peers = peers
	f.mu.Unlock()
}

func newHub(t *testing.T, rooms map[string]RoomLister) (*Hub, context.CancelFunc) {
	t.Helper()
	h := New(func() map[string]RoomLister { return rooms })
	ctx, cancel := context.WithCancel(context.Background())
	go h.Run(ctx)
	return h, cancel
}

func startServer(t *testing.T, h *Hub) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(h.ServeSSE))
}

// connect opens an SSE connection and returns a bufio.Reader over the body.
// The caller must close resp.Body when done.
func connect(t *testing.T, srv *httptest.Server) (*http.Response, *bufio.Reader) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	t.Cleanup(cancel)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, srv.URL, nil)
	if err != nil {
		t.Fatalf("new request: %v", err)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	if resp.StatusCode != http.StatusOK {
		resp.Body.Close()
		t.Fatalf("status %d", resp.StatusCode)
	}
	return resp, bufio.NewReader(resp.Body)
}

func mustReadFrame(t *testing.T, r *bufio.Reader) (event string, data []byte) {
	t.Helper()
	ev, d, err := readSSEFrame(r)
	if err != nil {
		t.Fatalf("readSSEFrame: %v", err)
	}
	return ev, d
}

func mustReadSnapshot(t *testing.T, r *bufio.Reader) protocol.PresenceSnapshotPayload {
	t.Helper()
	event, data := mustReadFrame(t, r)
	if event != protocol.PresenceSnapshotEvent {
		t.Fatalf("event = %q, want %q", event, protocol.PresenceSnapshotEvent)
	}
	var p protocol.PresenceSnapshotPayload
	if err := json.Unmarshal(data, &p); err != nil {
		t.Fatalf("snapshot unmarshal: %v", err)
	}
	return p
}

func TestServeSSESendsInitialSnapshot(t *testing.T) {
	room := &fakeRoom{peers: []protocol.PeerInfo{{ID: "abc", DisplayName: "Alice"}}}
	h, cancel := newHub(t, map[string]RoomLister{"room1": room})
	defer cancel()
	srv := startServer(t, h)
	defer srv.Close()

	resp, br := connect(t, srv)
	defer resp.Body.Close()

	snap := mustReadSnapshot(t, br)
	got := snap.Rooms["room1"].Peers
	if len(got) != 1 || got[0].ID != "abc" || got[0].DisplayName != "Alice" {
		t.Fatalf("initial snapshot peers = %+v", got)
	}
}

func TestPeerJoinedBroadcasts(t *testing.T) {
	room := &fakeRoom{}
	h, cancel := newHub(t, map[string]RoomLister{"room1": room})
	defer cancel()
	srv := startServer(t, h)
	defer srv.Close()

	const n = 3
	resps := make([]*http.Response, n)
	brs := make([]*bufio.Reader, n)
	for i := range n {
		resp, br := connect(t, srv)
		resps[i], brs[i] = resp, br
		defer resp.Body.Close()
		mustReadSnapshot(t, br) // drain initial
	}

	peer := protocol.PeerInfo{ID: "p1", DisplayName: "Bob"}
	h.PeerJoined("room1", peer)

	for i, br := range brs {
		event, data := mustReadFrame(t, br)
		if event != protocol.PresencePeerJoinedEvent {
			t.Fatalf("sub %d: event = %q, want %q", i, event, protocol.PresencePeerJoinedEvent)
		}
		var p protocol.PresencePeerJoinedPayload
		if err := json.Unmarshal(data, &p); err != nil {
			t.Fatalf("sub %d: unmarshal: %v", i, err)
		}
		if p.Room != "room1" || p.Peer.ID != "p1" || p.Peer.DisplayName != "Bob" {
			t.Fatalf("sub %d: payload = %+v", i, p)
		}
	}
}

func TestPeerLeftBroadcasts(t *testing.T) {
	room := &fakeRoom{peers: []protocol.PeerInfo{{ID: "p1"}}}
	h, cancel := newHub(t, map[string]RoomLister{"room1": room})
	defer cancel()
	srv := startServer(t, h)
	defer srv.Close()

	const n = 3
	resps := make([]*http.Response, n)
	brs := make([]*bufio.Reader, n)
	for i := range n {
		resp, br := connect(t, srv)
		resps[i], brs[i] = resp, br
		defer resp.Body.Close()
		mustReadSnapshot(t, br)
	}

	h.PeerLeft("room1", "p1")

	for i, br := range brs {
		event, data := mustReadFrame(t, br)
		if event != protocol.PresencePeerLeftEvent {
			t.Fatalf("sub %d: event = %q, want %q", i, event, protocol.PresencePeerLeftEvent)
		}
		var p protocol.PresencePeerLeftPayload
		if err := json.Unmarshal(data, &p); err != nil {
			t.Fatalf("sub %d: unmarshal: %v", i, err)
		}
		if p.Room != "room1" || p.ID != "p1" {
			t.Fatalf("sub %d: payload = %+v", i, p)
		}
	}
}

func TestPeerUpdatedBroadcasts(t *testing.T) {
	room := &fakeRoom{peers: []protocol.PeerInfo{{ID: "p1"}}}
	h, cancel := newHub(t, map[string]RoomLister{"room1": room})
	defer cancel()
	srv := startServer(t, h)
	defer srv.Close()

	const n = 3
	resps := make([]*http.Response, n)
	brs := make([]*bufio.Reader, n)
	for i := range n {
		resp, br := connect(t, srv)
		resps[i], brs[i] = resp, br
		defer resp.Body.Close()
		mustReadSnapshot(t, br)
	}

	updated := protocol.PeerInfo{ID: "p1", DisplayName: "Alice Updated", SelfMuted: true}
	h.PeerUpdated("room1", updated)

	for i, br := range brs {
		event, data := mustReadFrame(t, br)
		if event != protocol.PresencePeerUpdatedEvent {
			t.Fatalf("sub %d: event = %q, want %q", i, event, protocol.PresencePeerUpdatedEvent)
		}
		var p protocol.PresencePeerUpdatedPayload
		if err := json.Unmarshal(data, &p); err != nil {
			t.Fatalf("sub %d: unmarshal: %v", i, err)
		}
		if p.Room != "room1" || p.Peer.ID != "p1" || p.Peer.DisplayName != "Alice Updated" || !p.Peer.SelfMuted {
			t.Fatalf("sub %d: payload = %+v", i, p)
		}
	}
}

// TestNewSubReceivesSnapshotNotPastDeltas verifies that a subscriber who
// connects after some deltas were fired gets a snapshot reflecting current
// state — not a replay of past deltas.
func TestNewSubReceivesSnapshotNotPastDeltas(t *testing.T) {
	room := &fakeRoom{}
	h, cancel := newHub(t, map[string]RoomLister{"room1": room})
	defer cancel()
	srv := startServer(t, h)
	defer srv.Close()

	room.set([]protocol.PeerInfo{{ID: "p1", DisplayName: "Alice"}})
	h.PeerJoined("room1", protocol.PeerInfo{ID: "p1", DisplayName: "Alice"})
	time.Sleep(20 * time.Millisecond)

	resp, br := connect(t, srv)
	defer resp.Body.Close()

	event, data := mustReadFrame(t, br)
	if event != protocol.PresenceSnapshotEvent {
		t.Fatalf("first frame event = %q, want %q", event, protocol.PresenceSnapshotEvent)
	}
	var snap protocol.PresenceSnapshotPayload
	if err := json.Unmarshal(data, &snap); err != nil {
		t.Fatalf("snapshot unmarshal: %v", err)
	}
	peers := snap.Rooms["room1"].Peers
	if len(peers) != 1 || peers[0].ID != "p1" {
		t.Fatalf("snapshot peers = %+v", peers)
	}
}

// TestBurstEventsDoNotDeadlock fires many events rapidly and verifies all
// subscribers drain without deadlock.
func TestBurstEventsDoNotDeadlock(t *testing.T) {
	room := &fakeRoom{}
	h, cancel := newHub(t, map[string]RoomLister{"room1": room})
	defer cancel()
	srv := startServer(t, h)
	defer srv.Close()

	const numSubs = 5
	resps := make([]*http.Response, numSubs)
	brs := make([]*bufio.Reader, numSubs)
	for i := range numSubs {
		resp, br := connect(t, srv)
		resps[i], brs[i] = resp, br
		defer resp.Body.Close()
		mustReadSnapshot(t, br)
	}

	const burst = 50
	for i := range burst {
		h.PeerJoined("room1", protocol.PeerInfo{ID: strings.Repeat("x", i+1)})
	}

	// Exact count not asserted: slow readers get a snapshot-resync instead of
	// every delta, so per-sub frame counts vary.
	deadline := time.Now().Add(500 * time.Millisecond)
	var wg sync.WaitGroup
	for i := range numSubs {
		wg.Add(1)
		go func(br *bufio.Reader) {
			defer wg.Done()
			for time.Now().Before(deadline) {
				_, _, err := readSSEFrame(br)
				if err != nil {
					return
				}
			}
		}(brs[i])
	}
	wg.Wait()
}

func TestSlowSubscriberGetsSnapshotResync(t *testing.T) {
	room := &fakeRoom{peers: []protocol.PeerInfo{{ID: "final"}}}
	h := New(func() map[string]RoomLister {
		return map[string]RoomLister{"room1": room}
	})

	sub := &subscriber{out: make(chan []byte, outBufLen)}
	staleFrame := mustSSEFrame(t, protocol.PresencePeerJoinedEvent, protocol.PresencePeerJoinedPayload{
		Room: "room1",
		Peer: protocol.PeerInfo{ID: "stale"},
	})
	for range outBufLen {
		sub.out <- staleFrame
	}
	h.subs[sub] = struct{}{}

	h.fanout(mustSSEFrame(t, protocol.PresencePeerLeftEvent, protocol.PresencePeerLeftPayload{
		Room: "room1",
		ID:   "stale",
	}))

	if len(sub.out) != 1 {
		t.Fatalf("queued messages = %d, want 1 snapshot", len(sub.out))
	}
	frame := <-sub.out
	br := bufio.NewReader(strings.NewReader(string(frame)))
	event, data, err := readSSEFrame(br)
	if err != nil {
		t.Fatalf("readSSEFrame: %v", err)
	}
	if event != protocol.PresenceSnapshotEvent {
		t.Fatalf("event = %q, want %q", event, protocol.PresenceSnapshotEvent)
	}
	var snap protocol.PresenceSnapshotPayload
	if err := json.Unmarshal(data, &snap); err != nil {
		t.Fatalf("snapshot unmarshal: %v", err)
	}
	peers := snap.Rooms["room1"].Peers
	if len(peers) != 1 || peers[0].ID != "final" {
		t.Fatalf("snapshot peers = %+v", peers)
	}
}

func TestOverflowBuildsOneSnapshotForAllSlowSubscribers(t *testing.T) {
	room := &countingRoom{peers: []protocol.PeerInfo{{ID: "final"}}}
	h := New(func() map[string]RoomLister {
		return map[string]RoomLister{"room1": room}
	})

	for range 3 {
		sub := &subscriber{out: make(chan []byte, outBufLen)}
		for range outBufLen {
			sub.out <- []byte("stale")
		}
		h.subs[sub] = struct{}{}
	}
	h.fanout([]byte("delta"))

	if room.calls != 1 {
		t.Fatalf("room Peers calls = %d, want one shared snapshot", room.calls)
	}
	for sub := range h.subs {
		if len(sub.out) != 1 {
			t.Fatalf("queued messages = %d, want 1 snapshot", len(sub.out))
		}
	}
}

func BenchmarkOverflowResyncFanout(b *testing.B) {
	for _, numSubs := range []int{1, 16, 64} {
		b.Run(fmt.Sprintf("subscribers=%d", numSubs), func(b *testing.B) {
			room := &countingRoom{peers: make([]protocol.PeerInfo, 32)}
			h := New(func() map[string]RoomLister {
				return map[string]RoomLister{"room1": room}
			})
			for range numSubs {
				sub := &subscriber{out: make(chan []byte, outBufLen)}
				for range outBufLen {
					sub.out <- []byte("stale")
				}
				h.subs[sub] = struct{}{}
			}
			b.ReportAllocs()
			for b.Loop() {
				h.fanout([]byte("delta"))
				for sub := range h.subs {
					for len(sub.out) < outBufLen {
						sub.out <- []byte("stale")
					}
				}
			}
		})
	}
}

// TestNotifyBeforeAnySubscriberIsSafe verifies that firing events with no
// subscribers does not panic or block.
func TestNotifyBeforeAnySubscriberIsSafe(t *testing.T) {
	room := &fakeRoom{}
	h, cancel := newHub(t, map[string]RoomLister{"room1": room})
	defer cancel()

	for range 10 {
		h.PeerJoined("room1", protocol.PeerInfo{ID: "x"})
		h.PeerLeft("room1", "x")
		h.PeerUpdated("room1", protocol.PeerInfo{ID: "x"})
	}
	time.Sleep(20 * time.Millisecond)
}

func mustSSEFrame(t *testing.T, event string, payload any) []byte {
	t.Helper()
	data, err := json.Marshal(payload)
	if err != nil {
		t.Fatalf("payload marshal: %v", err)
	}
	return fmt.Appendf(nil, "event: %s\ndata: %s\n\n", event, data)
}
