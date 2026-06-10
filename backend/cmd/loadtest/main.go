// Command loadtest drives synthetic load against a running voice-hub
// server. It authenticates via POST /api/login, opens N pion-backed
// peers via /ws/{room}, performs the full SDP/ICE exchange, optionally
// publishes a fake Opus track, and reports rolling per-second stats.
// Exits non-zero when peers fail to connect or no RTP flows, so it
// doubles as a CI smoke gate.
//
// Example:
//
//	# server side
//	APP_ADMIN_PASSWORD=<admin-password> PUBLIC_IP=127.0.0.1 \
//	    go run ./cmd/server
//
//	# loadtest side (separate terminal)
//	go run ./cmd/loadtest \
//	    -target=http://localhost:8080 -password=<admin-password> \
//	    -peers=20 -publish -duration=30s
//
// The tool prints one stats line per second:
//
//	t=Xs connected=N errors=N rtpRecv/s=N msg/s=N goroutines=N
//
// Use to find the point at which the server starts dropping peers,
// failing handshakes, or exhausting CPU/memory.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/coder/websocket"
	"github.com/pion/interceptor"
	"github.com/pion/webrtc/v4"
	"github.com/pion/webrtc/v4/pkg/media"

	"voice-hub/backend/internal/sfu/protocol"
)

func main() {
	var (
		target        = flag.String("target", "http://localhost:8080", "voice-hub base URL (http:// or https://)")
		room          = flag.String("room", "room1", "room slug to join")
		password      = flag.String("password", "", "admin password for /api/login (mutually exclusive with -password-stdin)")
		passwordStdin = flag.Bool("password-stdin", false, "read password from stdin (use this to avoid leaving the secret in argv / shell history)")
		peers         = flag.Int("peers", 10, "number of synthetic peers")
		duration      = flag.Duration("duration", 30*time.Second, "how long to run after all peers connect")
		publish       = flag.Bool("publish", false, "have ALL peers publish a fake Opus track at 50 pps (overridden by -publishers)")
		publishers    = flag.Int("publishers", -1, "if >= 0, only the first K peers publish; the rest are listeners. Models a real room (few speakers, many listeners)")
		ramp          = flag.Duration("ramp", 0, "spread peer connect over this duration (0 = all at once)")
	)
	flag.Parse()

	if *passwordStdin {
		if *password != "" {
			fmt.Fprintln(os.Stderr, "error: -password and -password-stdin are mutually exclusive")
			os.Exit(2)
		}
		buf, err := io.ReadAll(os.Stdin)
		if err != nil {
			fmt.Fprintf(os.Stderr, "error: read password from stdin: %v\n", err)
			os.Exit(2)
		}
		*password = strings.TrimRight(string(buf), "\r\n")
	}

	if *password == "" {
		fmt.Fprintln(os.Stderr, "error: -password or -password-stdin is required")
		flag.Usage()
		os.Exit(2)
	}

	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer cancel()

	httpClient := &http.Client{Timeout: 10 * time.Second}

	cookie, err := login(ctx, httpClient, *target, *password)
	if err != nil {
		log.Fatalf("login: %v", err)
	}
	wsURL := strings.Replace(*target, "http", "ws", 1) + "/ws/" + *room
	log.Printf("logged in, ws=%s", wsURL)

	api := newAPI()

	stats := &runStats{}

	// Stats printer ticks every second until ctx cancelled.
	statsCtx, stopStats := context.WithCancel(ctx)
	defer stopStats()
	go statsPrinter(statsCtx, stats)

	clients := make([]*loadClient, *peers)
	var wg sync.WaitGroup
	connectStart := time.Now()
	for i := range *peers {
		if *ramp > 0 && i > 0 {
			delay := *ramp / time.Duration(*peers)
			select {
			case <-time.After(delay):
			case <-ctx.Done():
				return
			}
		}
		shouldPublish := *publish
		if *publishers >= 0 {
			shouldPublish = i < *publishers
		}
		wg.Add(1)
		go func(i int, pub bool) {
			defer wg.Done()
			c, err := dialClient(ctx, wsURL, fmt.Sprintf("p%d", i), fmt.Sprintf("cid-%d", i), cookie, api, pub, stats)
			if err != nil {
				stats.connectErrors.Add(1)
				log.Printf("peer %d: %v", i, err)
				return
			}
			stats.connected.Add(1)
			clients[i] = c
		}(i, shouldPublish)
	}
	wg.Wait()
	connectDur := time.Since(connectStart)
	log.Printf("connect phase: %d/%d peers in %v (errors=%d)",
		stats.connected.Load(), *peers, connectDur, stats.connectErrors.Load())

	// Start a publishLoop for every connected peer that has an
	// audioTrack (set during dialClient when shouldPublish was true).
	stop := make(chan struct{})
	var pubWg sync.WaitGroup
	pubCount := 0
	for _, c := range clients {
		if c == nil || c.audioTrack == nil {
			continue
		}
		pubCount++
		pubWg.Add(1)
		go func(c *loadClient) {
			defer pubWg.Done()
			c.publishLoop(stop)
		}(c)
	}
	if pubCount > 0 {
		log.Printf("publishing: %d/%d connected peers", pubCount, stats.connected.Load())
	}
	select {
	case <-time.After(*duration):
	case <-ctx.Done():
	}
	close(stop)
	pubWg.Wait()

	stopStats()
	for _, c := range clients {
		if c != nil {
			c.close()
		}
	}
	stats.print(int(time.Since(connectStart).Seconds()))

	connected := stats.connected.Load()
	if connected < int64(*peers) {
		log.Printf("FAIL: connected %d/%d peers", connected, *peers)
		os.Exit(1)
	}
	if pubCount > 0 && connected > int64(pubCount) && stats.rtpRecv.Load() == 0 {
		log.Print("FAIL: publishers were sending but listeners received no RTP")
		os.Exit(1)
	}
}

// runStats holds rolling counters reported by the stats printer.
type runStats struct {
	connected     atomic.Int64
	connectErrors atomic.Int64
	rtpRecv       atomic.Int64
	wsRecv        atomic.Int64
	wsSendErrors  atomic.Int64
	disconnected  atomic.Int64
}

func (s *runStats) print(t int) {
	log.Printf("FINAL t=%ds connected=%d errors=%d wsRecv=%d rtpRecv=%d wsSendErr=%d disconnected=%d",
		t, s.connected.Load(), s.connectErrors.Load(),
		s.wsRecv.Load(), s.rtpRecv.Load(),
		s.wsSendErrors.Load(), s.disconnected.Load())
}

func statsPrinter(ctx context.Context, s *runStats) {
	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()
	var prevRTP, prevWS int64
	t0 := time.Now()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			rtp := s.rtpRecv.Load()
			ws := s.wsRecv.Load()
			drtp := rtp - prevRTP
			dws := ws - prevWS
			prevRTP, prevWS = rtp, ws
			log.Printf("t=%ds connected=%d errors=%d rtpRecv/s=%d wsRecv/s=%d disc=%d",
				int(time.Since(t0).Seconds()),
				s.connected.Load()-s.disconnected.Load(),
				s.connectErrors.Load(), drtp, dws, s.disconnected.Load())
		}
	}
}

func login(ctx context.Context, c *http.Client, target, password string) (string, error) {
	form := url.Values{}
	form.Set("password", password)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, target+"/api/login", strings.NewReader(form.Encode()))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := c.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusNoContent {
		return "", fmt.Errorf("login: status %d", resp.StatusCode)
	}
	for _, ck := range resp.Cookies() {
		if ck.Name == "vh_session" {
			return ck.Name + "=" + ck.Value, nil
		}
	}
	return "", fmt.Errorf("login: no vh_session cookie in response")
}

func newAPI() *webrtc.API {
	m := &webrtc.MediaEngine{}
	if err := m.RegisterDefaultCodecs(); err != nil {
		log.Fatalf("register codecs: %v", err)
	}
	ir := &interceptor.Registry{}
	return webrtc.NewAPI(webrtc.WithMediaEngine(m), webrtc.WithInterceptorRegistry(ir))
}

type loadClient struct {
	id         string
	pc         *webrtc.PeerConnection
	ws         *websocket.Conn
	audioTrack *webrtc.TrackLocalStaticSample
	writeMu    sync.Mutex
	ctx        context.Context
	cancel     context.CancelFunc
	stats      *runStats
}

func dialClient(parent context.Context, wsURL, displayName, clientID, cookie string, api *webrtc.API, publish bool, stats *runStats) (*loadClient, error) {
	ctx, cancel := context.WithCancel(parent)
	hdr := http.Header{"Cookie": []string{cookie}}
	ws, _, err := websocket.Dial(ctx, wsURL, &websocket.DialOptions{HTTPHeader: hdr})
	if err != nil {
		cancel()
		return nil, fmt.Errorf("ws dial: %w", err)
	}
	ws.SetReadLimit(1 << 20)

	pc, err := api.NewPeerConnection(webrtc.Configuration{})
	if err != nil {
		ws.Close(websocket.StatusInternalError, "")
		cancel()
		return nil, fmt.Errorf("new pc: %w", err)
	}

	c := &loadClient{id: clientID, pc: pc, ws: ws, ctx: ctx, cancel: cancel, stats: stats}

	if publish {
		track, err := webrtc.NewTrackLocalStaticSample(
			webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeOpus, ClockRate: 48000, Channels: 2},
			"audio", clientID,
		)
		if err != nil {
			ws.Close(websocket.StatusInternalError, "")
			cancel()
			return nil, fmt.Errorf("new track: %w", err)
		}
		if _, err := pc.AddTrack(track); err != nil {
			ws.Close(websocket.StatusInternalError, "")
			cancel()
			return nil, fmt.Errorf("add track: %w", err)
		}
		c.audioTrack = track
	}

	pc.OnICECandidate(func(ice *webrtc.ICECandidate) {
		if ice == nil {
			return
		}
		b, err := json.Marshal(protocol.CandidateEnvelope{PC: protocol.PCAudio, ICECandidateInit: ice.ToJSON()})
		if err != nil {
			return
		}
		_ = c.write(protocol.Envelope{Event: "candidate", Data: b})
	})
	pc.OnTrack(func(t *webrtc.TrackRemote, _ *webrtc.RTPReceiver) {
		buf := make([]byte, 1500)
		for {
			if _, _, err := t.Read(buf); err != nil {
				return
			}
			c.stats.rtpRecv.Add(1)
		}
	})
	pc.OnConnectionStateChange(func(s webrtc.PeerConnectionState) {
		if s == webrtc.PeerConnectionStateFailed || s == webrtc.PeerConnectionStateClosed {
			c.stats.disconnected.Add(1)
		}
	})

	hello, _ := json.Marshal(protocol.HelloPayload{DisplayName: displayName, ClientID: clientID})
	if err := c.write(protocol.Envelope{Event: "hello", Data: hello}); err != nil {
		c.close()
		return nil, fmt.Errorf("hello: %w", err)
	}

	go c.readLoop()
	return c, nil
}

func (c *loadClient) write(env protocol.Envelope) error {
	raw, err := json.Marshal(env)
	if err != nil {
		return err
	}
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	wctx, cancel := context.WithTimeout(c.ctx, 5*time.Second)
	defer cancel()
	if err := c.ws.Write(wctx, websocket.MessageText, raw); err != nil {
		c.stats.wsSendErrors.Add(1)
		return err
	}
	return nil
}

func (c *loadClient) readLoop() {
	for {
		_, raw, err := c.ws.Read(c.ctx)
		if err != nil {
			return
		}
		c.stats.wsRecv.Add(1)
		var env protocol.Envelope
		if err := json.Unmarshal(raw, &env); err != nil {
			continue
		}
		switch env.Event {
		case "offer":
			var sd webrtc.SessionDescription
			if err := json.Unmarshal(env.Data, &sd); err != nil {
				continue
			}
			if err := c.pc.SetRemoteDescription(sd); err != nil {
				continue
			}
			answer, err := c.pc.CreateAnswer(nil)
			if err != nil {
				continue
			}
			if err := c.pc.SetLocalDescription(answer); err != nil {
				continue
			}
			ans, _ := json.Marshal(protocol.AnswerEnvelope{PC: protocol.PCAudio, SessionDescription: answer})
			_ = c.write(protocol.Envelope{Event: "answer", Data: ans})
		case "candidate":
			var ic webrtc.ICECandidateInit
			if err := json.Unmarshal(env.Data, &ic); err != nil {
				continue
			}
			_ = c.pc.AddICECandidate(ic)
		}
	}
}

func (c *loadClient) publishLoop(stop <-chan struct{}) {
	if c.audioTrack == nil {
		return
	}
	payload := make([]byte, 80)
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-stop:
			return
		case <-c.ctx.Done():
			return
		case <-ticker.C:
			_ = c.audioTrack.WriteSample(media.Sample{Data: payload, Duration: 20 * time.Millisecond})
		}
	}
}

func (c *loadClient) close() {
	c.cancel()
	_ = c.ws.Close(websocket.StatusNormalClosure, "bye")
	_ = c.pc.Close()
}
