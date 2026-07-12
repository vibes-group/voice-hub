// Package handler provides the HTTP handler factories for the voice-hub server.
// Each factory captures its dependencies by value or pointer at construction
// time; the returned HandlerFunc is safe for concurrent use.
package handler

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"net/netip"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"voice-hub/backend/internal/auth"
	"voice-hub/backend/internal/middleware"
	turnsrv "voice-hub/backend/internal/turn"
)

// turnCredsTTL balances security and usability: credentials are short-lived enough
// to limit exposure if leaked, but long enough to avoid frequent re-issuance during
// typical calls and brief reconnects.
const turnCredsTTL = 6 * time.Hour

type HealthResponse struct {
	Status string `json:"status"`
}

// ICEServer describes one ICE/TURN server entry in AppConfigResponse.
type ICEServer struct {
	URLs       []string `json:"urls"`
	Username   string   `json:"username,omitempty"`
	Credential string   `json:"credential,omitempty"`
}

type AppConfigResponse struct {
	ICEServers []ICEServer `json:"iceServers"`
	Role       string      `json:"role"`
}

type VersionResponse struct {
	Version string `json:"version"`
}

// ConnPassPlaintextResponse carries the one-time plaintext shown after mint or rotate.
type ConnPassPlaintextResponse struct {
	Host       string    `json:"host"`
	ID         string    `json:"id"`
	Label      string    `json:"label"`
	Password   string    `json:"password"`
	Generation uint64    `json:"generation"`
	CreatedAt  time.Time `json:"created_at"`
}

// FrontendVersion fingerprints the deployed frontend by hashing index.html.
// Vite injects content-hashed asset URLs into index.html, so any rebuild
// shifts the hash. Used by the version-poll banner to detect stale tabs.
func FrontendVersion(webDir string) string {
	indexPath := filepath.Join(webDir, "index.html")
	data, err := os.ReadFile(indexPath)
	if err != nil {
		log.Printf("version: cannot read %s: %v", indexPath, err)
		return "unknown"
	}
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])[:12]
}

func Health() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if err := json.NewEncoder(w).Encode(HealthResponse{Status: "ok"}); err != nil {
			log.Printf("health: write: %v", err)
		}
	}
}

func CallStatus(active func() bool) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		if active() {
			_, _ = w.Write([]byte("active\n"))
			return
		}
		_, _ = w.Write([]byte("idle\n"))
	}
}

func Version(version string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Cache-Control", "no-store")
		if err := json.NewEncoder(w).Encode(VersionResponse{Version: version}); err != nil {
			log.Printf("version: encode: %v", err)
		}
	}
}

// LoginConfig groups dependencies and options for the Login handler factory.
type LoginConfig struct {
	AdminPassword string
	AdminVer      string
	CookieSecure  bool
	SessionSecret []byte
	ConnPass      *auth.ConnPassStore
	Limiter       *auth.AuthLimiter
	Trusted       []netip.Prefix
}

// Login handles POST /api/login. It checks the admin password first (constant-time),
// then the connection password, and issues a signed session cookie on success.
// trusted is the proxy CIDR list — it controls which RemoteAddr values are
// allowed to set X-Forwarded-For for rate-limit keying.
func Login(cfg LoginConfig) http.HandlerFunc {
	wantAdmin := []byte(cfg.AdminPassword)
	return func(w http.ResponseWriter, r *http.Request) {
		ip := middleware.ClientIP(r, cfg.Trusted)
		if cfg.Limiter.Blocked(ip) {
			w.Header().Set("Retry-After", "900")
			http.Error(w, "too many failed attempts", http.StatusTooManyRequests)
			return
		}
		if err := r.ParseForm(); err != nil {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		pass := r.PostFormValue("password")
		if pass == "" {
			cfg.Limiter.Fail(ip)
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}

		// Admin first; constant-time compare to avoid timing leak on length.
		if subtle.ConstantTimeCompare([]byte(pass), wantAdmin) == 1 {
			cfg.Limiter.Success(ip)
			auth.SetSessionCookie(w, cfg.CookieSecure, cfg.SessionSecret, auth.RoleAdmin, 0, cfg.AdminVer, "")
			w.WriteHeader(http.StatusNoContent)
			return
		}

		// Then connection password — try every active entry.
		if entryID, gen, ok := cfg.ConnPass.Verify(pass); ok {
			cfg.Limiter.Success(ip)
			auth.SetSessionCookie(w, cfg.CookieSecure, cfg.SessionSecret, auth.RoleUser, gen, "", entryID)
			w.WriteHeader(http.StatusNoContent)
			return
		}

		cfg.Limiter.Fail(ip)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
	}
}

func Logout(cookieSecure bool) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		http.SetCookie(w, &http.Cookie{
			Name:     auth.CookieName,
			Value:    "",
			Path:     "/",
			MaxAge:   -1,
			HttpOnly: true,
			Secure:   cookieSecure,
			SameSite: http.SameSiteStrictMode,
		})
		w.WriteHeader(http.StatusNoContent)
	}
}

// turnUsernamePrefix is the stable prefix used when minting ephemeral TURN
// usernames for credentials returned by the /api/config endpoint.
const turnUsernamePrefix = "u"

// ConfigOptions contains dependencies and static values used by Config.
type ConfigOptions struct {
	SessionSecret    []byte
	TurnSharedSecret string
	StunURL          string
	TurnURL          string
}

// Config handles GET /api/config. It generates short-lived TURN credentials and
// returns ICE server config along with the caller's role.
func Config(opts ConfigOptions) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sess, _ := auth.SessionFromRequest(opts.SessionSecret, r)
		username, credential := turnsrv.GenerateCredentials(opts.TurnSharedSecret, turnUsernamePrefix, turnCredsTTL)
		response := AppConfigResponse{
			ICEServers: []ICEServer{
				{URLs: []string{opts.StunURL}},
				{
					URLs:       []string{opts.TurnURL},
					Username:   username,
					Credential: credential,
				},
			},
			Role: string(sess.Role),
		}
		w.Header().Set("Content-Type", "application/json")
		if err := json.NewEncoder(w).Encode(response); err != nil {
			log.Printf("config: encode: %v", err)
		}
	}
}

func ConnPassStatus(connPass *auth.ConnPassStore) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if err := json.NewEncoder(w).Encode(connPass.Status()); err != nil {
			log.Printf("connpass status: encode: %v", err)
		}
	}
}

// ConnPassCreate handles POST /api/admin/connection-passwords.
// Adds a new entry with an optional label and TTL and returns the plaintext once.
func ConnPassCreate(appHostname string, connPass *auth.ConnPassStore) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		label, ttl := parseLabelAndTTL(r)
		entry, plain, err := connPass.Create(label, ttl)
		if err != nil {
			if errors.Is(err, auth.ErrTooManyEntries) {
				http.Error(w, "too many entries", http.StatusConflict)
				return
			}
			log.Printf("connpass create: %v", err)
			http.Error(w, "create failed", http.StatusInternalServerError)
			return
		}
		writePlaintextResponse(w, appHostname, entry, plain, "connpass create")
	}
}

// ConnPassSetTTL handles POST /api/admin/connection-passwords/{id}/ttl.
// Body: {"ttl_seconds": N} where N=0 clears expiry, N>0 sets ExpiresAt=now+N.
func ConnPassSetTTL(connPass *auth.ConnPassStore) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := r.PathValue("id")
		ttl := parseTTL(r)
		entry, err := connPass.SetTTL(id, ttl)
		if err != nil {
			if errors.Is(err, auth.ErrEntryNotFound) {
				http.Error(w, "not found", http.StatusNotFound)
				return
			}
			log.Printf("connpass set ttl: %v", err)
			http.Error(w, "set ttl failed", http.StatusInternalServerError)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		if err := json.NewEncoder(w).Encode(entry); err != nil {
			log.Printf("connpass set ttl: encode: %v", err)
		}
	}
}

// ConnPassRotate handles POST /api/admin/connection-passwords/{id}/rotate.
// Replaces the plaintext of the named entry, invalidating sessions issued
// against the old one.
func ConnPassRotate(appHostname string, connPass *auth.ConnPassStore) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := r.PathValue("id")
		entry, plain, err := connPass.Rotate(id)
		if err != nil {
			if errors.Is(err, auth.ErrEntryNotFound) {
				http.Error(w, "not found", http.StatusNotFound)
				return
			}
			log.Printf("connpass rotate: %v", err)
			http.Error(w, "rotate failed", http.StatusInternalServerError)
			return
		}
		writePlaintextResponse(w, appHostname, entry, plain, "connpass rotate")
	}
}

// ConnPassRevoke handles DELETE /api/admin/connection-passwords/{id}.
func ConnPassRevoke(connPass *auth.ConnPassStore) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := r.PathValue("id")
		if err := connPass.Revoke(id); err != nil {
			if errors.Is(err, auth.ErrEntryNotFound) {
				http.Error(w, "not found", http.StatusNotFound)
				return
			}
			log.Printf("connpass revoke: %v", err)
			http.Error(w, "revoke failed", http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusNoContent)
	}
}

// ConnPassRename handles POST /api/admin/connection-passwords/{id}/rename.
func ConnPassRename(connPass *auth.ConnPassStore) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := r.PathValue("id")
		label := parseLabel(r)
		if err := connPass.Rename(id, label); err != nil {
			if errors.Is(err, auth.ErrEntryNotFound) {
				http.Error(w, "not found", http.StatusNotFound)
				return
			}
			log.Printf("connpass rename: %v", err)
			http.Error(w, "rename failed", http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusNoContent)
	}
}

// decodeJSONOrForm fills v from a JSON body when Content-Type is application/json,
// otherwise parses form values via fillForm.
func decodeJSONOrForm(r *http.Request, v any, fillForm func()) {
	if strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
		_ = json.NewDecoder(r.Body).Decode(v)
		return
	}
	_ = r.ParseForm()
	fillForm()
}

// parseLabel pulls a label out of either a JSON body or a form field.
// Empty body is allowed — the entry just gets no label.
func parseLabel(r *http.Request) string {
	var body struct {
		Label string `json:"label"`
	}
	decodeJSONOrForm(r, &body, func() { body.Label = r.PostFormValue("label") })
	return body.Label
}

// parseLabelAndTTL extracts {label, ttl_seconds} from the JSON body (or form),
// converting ttl_seconds to a Duration. Missing/zero/negative TTL = no expiry.
// TTL is capped at one year as a sanity bound — the admin can always extend.
func parseLabelAndTTL(r *http.Request) (string, time.Duration) {
	var body struct {
		Label      string `json:"label"`
		TTLSeconds int64  `json:"ttl_seconds"`
	}
	decodeJSONOrForm(r, &body, func() {
		body.Label = r.PostFormValue("label")
		body.TTLSeconds, _ = strconv.ParseInt(r.PostFormValue("ttl_seconds"), 10, 64)
	})
	return body.Label, ttlFromSeconds(body.TTLSeconds)
}

// parseTTL extracts {ttl_seconds} from the JSON body (or form).
func parseTTL(r *http.Request) time.Duration {
	var body struct {
		TTLSeconds int64 `json:"ttl_seconds"`
	}
	decodeJSONOrForm(r, &body, func() {
		body.TTLSeconds, _ = strconv.ParseInt(r.PostFormValue("ttl_seconds"), 10, 64)
	})
	return ttlFromSeconds(body.TTLSeconds)
}

const maxConnPassTTL = 366 * 24 * time.Hour

// ttlFromSeconds maps a JSON ttl_seconds value to a Duration with three
// distinct states understood by ConnPassStore.SetTTL / Create:
//   - s == 0 → 0          (never expires)
//   - s  > 0 → s*Second   (capped at maxConnPassTTL)
//   - s  < 0 → -1*Second  (disable now — entry kept on disk, login blocked)
func ttlFromSeconds(s int64) time.Duration {
	if s == 0 {
		return 0
	}
	if s < 0 {
		return -time.Second
	}
	d := time.Duration(s) * time.Second
	if d > maxConnPassTTL {
		return maxConnPassTTL
	}
	return d
}

func writePlaintextResponse(w http.ResponseWriter, host string, entry auth.ConnPassEntryStatus, plain, logTag string) {
	resp := ConnPassPlaintextResponse{
		Host:       host,
		ID:         entry.ID,
		Label:      entry.Label,
		Password:   plain,
		Generation: entry.Generation,
		CreatedAt:  entry.CreatedAt,
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	if err := json.NewEncoder(w).Encode(resp); err != nil {
		log.Printf("%s: encode: %v", logTag, err)
	}
}

// DisconnectUsers cancels every active /ws connection with role==user.
// Admin connections and the connection password are untouched, so a user
// whose cookie is still valid can reconnect — pair with ConnPassRevoke to
// lock new logins.
func DisconnectUsers(registry *auth.WSRegistry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		registry.DisconnectUsers()
		w.WriteHeader(http.StatusNoContent)
	}
}
