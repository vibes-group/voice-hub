package middleware

import (
	"bufio"
	"bytes"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"net/url"
	"strings"
	"testing"
	"time"

	"voice-hub/backend/internal/auth"
	"voice-hub/backend/internal/ratelimit"

	"golang.org/x/time/rate"
)

// loopbackTrusted matches what config.DefaultTrustedProxies returns; mirrored
// here so this package's tests stay free of the config import cycle.
var loopbackTrusted = []netip.Prefix{
	netip.MustParsePrefix("127.0.0.0/8"),
	netip.MustParsePrefix("::1/128"),
}

func TestRateLimitAPI(t *testing.T) {
	l := ratelimit.New(ratelimit.Config{Rate: rate.Every(time.Hour), Burst: 2})
	ok := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) })
	h := RateLimitAPI(l, loopbackTrusted, ok)

	// RemoteAddr is loopback (trusted), so XFF identifies the client. Burst of 2
	// passes, the 3rd is throttled with a Retry-After hint.
	do := func(path, xff string) *httptest.ResponseRecorder {
		r := httptest.NewRequest(http.MethodGet, path, nil)
		r.RemoteAddr = "127.0.0.1:5000"
		r.Header.Set("X-Forwarded-For", xff)
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, r)
		return rec
	}

	for i := 0; i < 2; i++ {
		if got := do("/api/x", "1.2.3.4").Code; got != http.StatusOK {
			t.Fatalf("/api req %d: got %d, want 200", i, got)
		}
	}
	rec := do("/api/x", "1.2.3.4")
	if rec.Code != http.StatusTooManyRequests {
		t.Fatalf("over-budget /api: got %d, want 429", rec.Code)
	}
	if rec.Header().Get("Retry-After") == "" {
		t.Fatal("expected Retry-After on 429")
	}

	// Non-/api paths are exempt even when that client is over budget.
	if got := do("/", "1.2.3.4").Code; got != http.StatusOK {
		t.Fatalf("non-/api path should be exempt, got %d", got)
	}

	// A different real client gets a fresh bucket.
	if got := do("/api/x", "5.6.7.8").Code; got != http.StatusOK {
		t.Fatalf("fresh client should pass, got %d", got)
	}
}

func TestClientIP(t *testing.T) {
	trusted := []netip.Prefix{
		netip.MustParsePrefix("10.0.0.0/8"),
		netip.MustParsePrefix("::1/128"),
	}
	tests := []struct {
		name       string
		remoteAddr string
		xff        string
		want       string
	}{
		{
			name:       "untrusted RemoteAddr ignores spoofed XFF",
			remoteAddr: "203.0.113.7:51000",
			xff:        "1.2.3.4, 5.6.7.8",
			want:       "203.0.113.7",
		},
		{
			name:       "trusted RemoteAddr + single XFF returns XFF",
			remoteAddr: "10.0.0.5:51000",
			xff:        "203.0.113.7",
			want:       "203.0.113.7",
		},
		{
			name:       "trusted RemoteAddr + chain (client, trusted1, trusted2)",
			remoteAddr: "10.0.0.5:51000",
			xff:        "203.0.113.7, 10.0.0.10, 10.0.0.20",
			want:       "203.0.113.7",
		},
		{
			name:       "trusted RemoteAddr + (attacker_spoofed_left, real_proxy)",
			remoteAddr: "10.0.0.5:51000",
			xff:        "9.9.9.9, 203.0.113.7",
			want:       "203.0.113.7",
		},
		{
			name:       "malformed XFF token fails safe to RemoteAddr",
			remoteAddr: "10.0.0.5:51000",
			xff:        "203.0.113.7, not-an-ip, 10.0.0.10",
			want:       "10.0.0.5",
		},
		{
			name:       "trusted RemoteAddr but no XFF returns RemoteAddr",
			remoteAddr: "10.0.0.5:51000",
			xff:        "",
			want:       "10.0.0.5",
		},
		{
			name:       "entire chain trusted returns RemoteAddr",
			remoteAddr: "10.0.0.5:51000",
			xff:        "10.0.0.10, 10.0.0.20",
			want:       "10.0.0.5",
		},
		{
			name:       "ipv6 loopback trusted",
			remoteAddr: "[::1]:51000",
			xff:        "203.0.113.7",
			want:       "203.0.113.7",
		},
		{
			name:       "ipv4-mapped ipv6 RemoteAddr unmaps to ipv4 trusted",
			remoteAddr: "[::ffff:10.0.0.5]:51000",
			xff:        "203.0.113.7",
			want:       "203.0.113.7",
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			r := httptest.NewRequest(http.MethodGet, "/", nil)
			r.RemoteAddr = tc.remoteAddr
			if tc.xff != "" {
				r.Header.Set("X-Forwarded-For", tc.xff)
			}
			if got := ClientIP(r, trusted); got != tc.want {
				t.Errorf("got %q, want %q", got, tc.want)
			}
		})
	}
}

// End-to-end check that the disconnect-users path actually cancels live
// /ws-style handlers: hit RequireAuthAPI -> TrackWS -> a blocking handler
// with both a user and an admin cookie, then call WSRegistry.DisconnectUsers
// and assert the user handler unblocks while the admin handler stays running.
// Catches regressions where TrackWS forgets to thread the child context, the
// registry is wired with the wrong role, or RequireAuthAPI rejects a freshly
// minted cookie.
func TestDisconnectUsersCancelsOnlyUserConnections(t *testing.T) {
	secret := []byte("0123456789abcdef0123456789abcdef")
	const adminVer = "av-current"
	connPass, err := auth.LoadConnPassStore(t.TempDir())
	if err != nil {
		t.Fatalf("connpass store: %v", err)
	}
	entry, _, err := connPass.Create("test", 0)
	if err != nil {
		t.Fatalf("create entry: %v", err)
	}
	registry := auth.NewWSRegistry()

	type result struct{ entered, exited chan struct{} }
	user := result{entered: make(chan struct{}, 1), exited: make(chan struct{}, 1)}
	admin := result{entered: make(chan struct{}, 1), exited: make(chan struct{}, 1)}

	pickResult := func(r *http.Request) result {
		sess, _ := auth.SessionFromRequest(secret, r)
		if sess.Role == auth.RoleAdmin {
			return admin
		}
		return user
	}

	blocking := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		res := pickResult(r)
		res.entered <- struct{}{}
		<-r.Context().Done()
		res.exited <- struct{}{}
	})
	chain := RequireAuthAPI(secret, connPass, adminVer, TrackWS(secret, registry, blocking))
	srv := httptest.NewServer(chain)
	t.Cleanup(srv.Close)

	userCookie := &http.Cookie{Name: auth.CookieName, Value: auth.Encode(secret, auth.RoleUser, entry.Generation, "", entry.ID, time.Hour)}
	adminCookie := &http.Cookie{Name: auth.CookieName, Value: auth.Encode(secret, auth.RoleAdmin, 0, adminVer, "", time.Hour)}

	mkReq := func(c *http.Cookie) *http.Request {
		req, _ := http.NewRequest(http.MethodGet, srv.URL, nil)
		req.AddCookie(c)
		return req
	}

	go func() { _, _ = http.DefaultClient.Do(mkReq(userCookie)) }()
	go func() { _, _ = http.DefaultClient.Do(mkReq(adminCookie)) }()

	waitFor := func(ch chan struct{}, label string) {
		select {
		case <-ch:
		case <-time.After(2 * time.Second):
			t.Fatalf("timed out waiting for %s", label)
		}
	}
	waitFor(user.entered, "user handler entry")
	waitFor(admin.entered, "admin handler entry")

	if got := registry.DisconnectUsers(); got != 1 {
		t.Fatalf("DisconnectUsers returned %d, want 1", got)
	}
	waitFor(user.exited, "user handler exit after disconnect")

	select {
	case <-admin.exited:
		t.Fatal("admin handler exited after DisconnectUsers — must stay connected")
	case <-time.After(100 * time.Millisecond):
	}

	// Drain the admin handler so the test server can shut down cleanly.
	srv.CloseClientConnections()
	waitFor(admin.exited, "admin handler exit on server close")
}

// A stale admin cookie (issued under a previous APP_ADMIN_PASSWORD) must be
// rejected by the broad RequireAuthAPI gate, not only by RequireAdmin —
// otherwise it could still reach /api/config and /ws endpoints.
func TestRequireAuthAPIRejectsStaleAdminCookie(t *testing.T) {
	secret := []byte("0123456789abcdef0123456789abcdef")
	connPass, err := auth.LoadConnPassStore(t.TempDir())
	if err != nil {
		t.Fatalf("connpass store: %v", err)
	}
	srv := httptest.NewServer(RequireAuthAPI(secret, connPass, "av-current",
		http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) })))
	t.Cleanup(srv.Close)

	stale := &http.Cookie{Name: auth.CookieName, Value: auth.Encode(secret, auth.RoleAdmin, 0, "av-old", "", time.Hour)}
	req, _ := http.NewRequest(http.MethodGet, srv.URL, nil)
	req.AddCookie(stale)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("stale admin cookie: got %d, want 401", resp.StatusCode)
	}
}

// Percent-encoded LF in path decodes to a literal \n on r.URL.Path. With %s
// formatting that newline forges a fake log line; %q escapes it.
func TestAccessLogQuotesPath(t *testing.T) {
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(nil) })

	srv := httptest.NewServer(AccessLog(loopbackTrusted, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})))
	t.Cleanup(srv.Close)

	u, err := url.Parse(srv.URL)
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	conn, err := net.Dial("tcp", u.Host)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer conn.Close()
	if _, err := conn.Write([]byte("GET /a%0Ainjected HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")); err != nil {
		t.Fatalf("write: %v", err)
	}
	resp, err := http.ReadResponse(bufio.NewReader(conn), nil)
	if err != nil {
		t.Fatalf("read response: %v", err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status: got %d want 200", resp.StatusCode)
	}

	out := buf.String()
	if got := strings.Count(out, "\n"); got != 1 {
		t.Fatalf("AccessLog must emit exactly one line; got %d: %q", got, out)
	}
	if !strings.Contains(out, `\n`) {
		t.Errorf("log line should contain escaped \\n marker proving %%q quoted the path: %q", out)
	}
}

// Go's HTTP parser folds obs-fold continuations to a single space and rejects
// raw CR/LF, so an attacker cannot smuggle a newline into r.Header values.
// %q on the ip field is defense-in-depth: even when ClientIP returns a garbage
// string (folded continuations, comma-stuffed XFF), %q escapes it so log
// structure stays parseable.
func TestAccessLogQuotesClientIP(t *testing.T) {
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(nil) })

	srv := httptest.NewServer(AccessLog(loopbackTrusted, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})))
	t.Cleanup(srv.Close)

	u, err := url.Parse(srv.URL)
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	conn, err := net.Dial("tcp", u.Host)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer conn.Close()
	// Header continuation (LWS) is the only way to smuggle a \n into a header
	// value that Go's server will accept. The continuation byte (space/tab) at
	// the start of the next line means Go folds it into the prior header value.
	raw := "GET / HTTP/1.1\r\nHost: x\r\nX-Forwarded-For: 1.2.3.4\r\n\thttp FORGED \"/admin\" 200 0s ip=\"evil\"\r\nConnection: close\r\n\r\n"
	if _, err := conn.Write([]byte(raw)); err != nil {
		t.Fatalf("write: %v", err)
	}
	resp, err := http.ReadResponse(bufio.NewReader(conn), nil)
	if err != nil {
		// Go 1.21+ rejects obs-fold by default; that's a valid defense too.
		t.Skipf("server rejected folded header (acceptable defense): %v", err)
	}
	resp.Body.Close()

	out := buf.String()
	if got := strings.Count(out, "\n"); got != 1 {
		t.Fatalf("AccessLog must emit exactly one line; got %d: %q", got, out)
	}
}
