// Package middleware provides HTTP middleware used by the voice-hub server.
package middleware

import (
	"context"
	"crypto/hmac"
	"log"
	"net/http"
	"net/netip"
	"strings"
	"time"

	"voice-hub/backend/internal/auth"
)

// statusRecorder captures the response status code written by a handler so
// AccessLog can include it. The default is 200 if WriteHeader is never called.
type statusRecorder struct {
	http.ResponseWriter
	status int
}

func (s *statusRecorder) WriteHeader(code int) {
	s.status = code
	s.ResponseWriter.WriteHeader(code)
}

// AccessLog wraps next with one log line per request.
// /healthz (Docker health-check spam) and /ws (WebSocket — hijacks the
// connection, wrapping would break Hijacker) are passed through silently.
// trusted is the proxy CIDR list; passed through to ClientIP so the logged IP
// is the same identity the rate limiter keys on.
func AccessLog(trusted []netip.Prefix, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/healthz" || strings.HasPrefix(r.URL.Path, "/ws/") {
			next.ServeHTTP(w, r)
			return
		}
		start := time.Now()
		rec := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
		next.ServeHTTP(rec, r)
		log.Printf("http %s %q %d %s ip=%q",
			r.Method, r.URL.Path, rec.status, time.Since(start).Round(time.Millisecond), ClientIP(r, trusted))
	})
}

// RequireAuthHTML gates HTML routes behind a valid session. Public assets
// (login page, favicons, Vite bundles) pass through without a cookie.
// Unauthenticated requests to gated paths are redirected to /login.html.
// User sessions whose ConnPass generation has drifted are also rejected.
func RequireAuthHTML(secret []byte, connPass *auth.ConnPassStore, adminVer string, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// connect.html is Tauri-only; hide it from browser consumers.
		if r.URL.Path == "/connect.html" {
			http.NotFound(w, r)
			return
		}
		switch r.URL.Path {
		case "/login.html", "/login.js",
			"/favicon.ico", "/favicon.svg", "/favicon.png", "/apple-touch-icon.png":
			next.ServeHTTP(w, r)
			return
		}
		if strings.HasPrefix(r.URL.Path, "/assets/") || strings.HasPrefix(r.URL.Path, "/vendor/") {
			next.ServeHTTP(w, r)
			return
		}
		if auth.Authenticated(secret, connPass, adminVer, r) {
			next.ServeHTTP(w, r)
			return
		}
		http.Redirect(w, r, "/login.html", http.StatusSeeOther)
	})
}

// RequireAuthAPI gates API routes behind a valid session.
// Returns 401 for unauthenticated requests (no redirect).
func RequireAuthAPI(secret []byte, connPass *auth.ConnPassStore, adminVer string, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if auth.Authenticated(secret, connPass, adminVer, r) {
			next.ServeHTTP(w, r)
			return
		}
		http.Error(w, "unauthorized", http.StatusUnauthorized)
	})
}

// RequireAdmin gates a handler behind a valid admin-role session whose
// embedded AdminVersion matches adminVer. Mismatch -> 403, so rotating
// APP_ADMIN_PASSWORD via redeploy invalidates every old admin cookie.
func RequireAdmin(secret []byte, adminVer string, next http.Handler) http.Handler {
	want := []byte(adminVer)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		sess, ok := auth.SessionFromRequest(secret, r)
		if !ok || sess.Role != auth.RoleAdmin {
			http.Error(w, "forbidden", http.StatusForbidden)
			return
		}
		if !hmac.Equal([]byte(sess.AdminVersion), want) {
			http.Error(w, "forbidden", http.StatusForbidden)
			return
		}
		next.ServeHTTP(w, r)
	})
}

// TrackWS registers each /ws connection in registry under the session role
// and wraps the request with a cancellable child context. The admin
// "Disconnect users" endpoint cancels role==user contexts via the registry.
// Must run inside RequireAuthAPI so a valid session is guaranteed.
func TrackWS(secret []byte, registry *auth.WSRegistry, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		sess, ok := auth.SessionFromRequest(secret, r)
		if !ok {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		ctx, cancel := context.WithCancel(r.Context())
		defer cancel()
		id := registry.Add(sess.Role, cancel)
		defer registry.Remove(id)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// ClientIP returns the request's source IP, treating X-Forwarded-For as
// authoritative only when RemoteAddr is itself a trusted proxy. The XFF chain
// is walked right-to-left (last hop = most trusted): trusted entries are
// stripped, the first untrusted entry is the real source. Any malformed token
// fails safe to RemoteAddr, since a partially-parsed chain cannot be trusted
// to identify the real client.
//
// trusted typically comes from config.Config.TrustedProxies (loopback by
// default, the docker network range in prod compose).
func ClientIP(r *http.Request, trusted []netip.Prefix) string {
	addr, ok := remoteAddrIP(r)
	if !ok {
		return r.RemoteAddr
	}
	if !inAny(addr, trusted) {
		return addr.String()
	}
	xff := r.Header.Get("X-Forwarded-For")
	if xff == "" {
		return addr.String()
	}
	parts := strings.Split(xff, ",")
	for i := len(parts) - 1; i >= 0; i-- {
		cand, err := netip.ParseAddr(strings.TrimSpace(parts[i]))
		if err != nil {
			return addr.String()
		}
		if !inAny(cand.Unmap(), trusted) {
			return cand.Unmap().String()
		}
	}
	return addr.String()
}

func remoteAddrIP(r *http.Request) (netip.Addr, bool) {
	if ap, err := netip.ParseAddrPort(r.RemoteAddr); err == nil {
		return ap.Addr().Unmap(), true
	}
	if a, err := netip.ParseAddr(r.RemoteAddr); err == nil {
		return a.Unmap(), true
	}
	return netip.Addr{}, false
}

func inAny(a netip.Addr, prefixes []netip.Prefix) bool {
	for _, p := range prefixes {
		if p.Contains(a) {
			return true
		}
	}
	return false
}
