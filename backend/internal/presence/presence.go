package presence

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"sync"
	"time"

	"voice-hub/backend/internal/sfu/protocol"
)

type RoomLister interface {
	Peers() []protocol.PeerInfo
}

type Rooms func() map[string]RoomLister

// Hub fans out presence deltas (and an initial snapshot) to all /api/presence
// subscribers over Server-Sent Events. Events are pre-framed as SSE bytes once
// and pushed onto a buffered channel; a single Run goroutine drains the channel
// and fans out under h.mu, keeping per-subscriber writes sequential and race-free.
type Hub struct {
	rooms Rooms

	// events carries pre-framed SSE bytes. Capacity sized for burst tolerance.
	// On full, the producer blocks until Run drains; per-subscriber overflow is
	// handled separately in fanout via snapshot-resync.
	events chan []byte

	mu   sync.Mutex
	subs map[*subscriber]struct{}
}

type subscriber struct {
	out chan []byte
}

const (
	eventBufLen = 256
	outBufLen   = 8
)

func New(rooms Rooms) *Hub {
	return &Hub{
		rooms:  rooms,
		events: make(chan []byte, eventBufLen),
		subs:   make(map[*subscriber]struct{}),
	}
}

// Run must be started exactly once before any PeerJoined/PeerLeft/PeerUpdated
// or ServeSSE call.
func (h *Hub) Run(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case env := <-h.events:
			h.fanout(env)
		}
	}
}

func (h *Hub) PeerJoined(roomSlug string, peer protocol.PeerInfo) {
	h.push(protocol.PresencePeerJoinedEvent, protocol.PresencePeerJoinedPayload{
		Room: roomSlug,
		Peer: peer,
	})
}

func (h *Hub) PeerLeft(roomSlug string, id string) {
	h.push(protocol.PresencePeerLeftEvent, protocol.PresencePeerLeftPayload{
		Room: roomSlug,
		ID:   id,
	})
}

func (h *Hub) PeerUpdated(roomSlug string, peer protocol.PeerInfo) {
	h.push(protocol.PresencePeerUpdatedEvent, protocol.PresencePeerUpdatedPayload{
		Room: roomSlug,
		Peer: peer,
	})
}

func (h *Hub) push(event string, payload any) {
	data, err := json.Marshal(payload)
	if err != nil {
		slog.Error("presence: marshal payload", "event", event, "err", err)
		return
	}
	frame := fmt.Appendf(nil, "event: %s\ndata: %s\n\n", event, data)
	h.events <- frame
}

func (h *Hub) fanout(frame []byte) {
	h.mu.Lock()
	subs := make([]*subscriber, 0, len(h.subs))
	for sub := range h.subs {
		subs = append(subs, sub)
	}
	h.mu.Unlock()

	// Single writer per sub.out (this goroutine), so the drain-then-push
	// resync pattern is race-free.
	for _, sub := range subs {
		select {
		case sub.out <- frame:
		default:
			h.queueResync(sub)
		}
	}
}

func (h *Hub) queueResync(sub *subscriber) {
	snap, err := h.buildSnapshot()
	if err != nil {
		slog.Error("presence: resync snapshot", "err", err)
		return
	}
	for {
		select {
		case <-sub.out:
		default:
			sub.out <- snap
			return
		}
	}
}

// SSE is unidirectional: no read pump needed.
func (h *Hub) ServeSSE(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.Header().Set("X-Accel-Buffering", "no")

	rc := http.NewResponseController(w)

	sub := &subscriber{out: make(chan []byte, outBufLen)}

	// Critical ordering: build snapshot and register sub under the same lock
	// so no delta can be lost between the two. A delta fired before Lock()
	// is reflected in the snapshot. A delta fired after Unlock() is delivered
	// via fanout. A delta fired between snapshot and Unlock() blocks at h.mu
	// in fanout, then delivers after Unlock() — the client may already have
	// the state from the snapshot; idempotent merge absorbs the duplicate.
	h.mu.Lock()
	snapBytes, err := h.buildSnapshot()
	if err != nil {
		h.mu.Unlock()
		slog.Error("presence: initial snapshot", "err", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}
	sub.out <- snapBytes
	h.subs[sub] = struct{}{}
	h.mu.Unlock()

	defer func() {
		h.mu.Lock()
		delete(h.subs, sub)
		h.mu.Unlock()
	}()

	heartbeat := time.NewTicker(25 * time.Second)
	defer heartbeat.Stop()

	ctx := r.Context()
	for {
		select {
		case <-ctx.Done():
			return
		case frame := <-sub.out:
			if _, err := w.Write(frame); err != nil {
				return
			}
			if err := rc.Flush(); err != nil {
				return
			}
		case <-heartbeat.C:
			if _, err := w.Write([]byte(": heartbeat\n\n")); err != nil {
				return
			}
			if err := rc.Flush(); err != nil {
				return
			}
		}
	}
}

func (h *Hub) buildSnapshot() ([]byte, error) {
	rooms := h.rooms()
	payload := protocol.PresenceSnapshotPayload{
		Rooms: make(map[string]protocol.PresenceRoom, len(rooms)),
	}
	for slug, room := range rooms {
		payload.Rooms[slug] = protocol.PresenceRoom{Peers: room.Peers()}
	}
	data, err := json.Marshal(payload)
	if err != nil {
		return nil, err
	}
	return fmt.Appendf(nil, "event: %s\ndata: %s\n\n", protocol.PresenceSnapshotEvent, data), nil
}

