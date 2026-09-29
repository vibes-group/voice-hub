package main

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"voice-hub/backend/internal/auth"
	"voice-hub/backend/internal/config"
	"voice-hub/backend/internal/handler"
	"voice-hub/backend/internal/middleware"
)

// Tests in this file enforce the privilege boundary:
// a guest (user-role session) MUST NOT reach admin endpoints, and admin
// endpoints MUST NOT be reachable without a valid admin cookie.

func newTestSecret() []byte {
	return []byte("0123456789abcdef0123456789abcdef")
}

func mustHash(t *testing.T, plain string) string {
	t.Helper()
	h, err := auth.HashPassword(plain)
	if err != nil {
		t.Fatal(err)
	}
	return h
}

func adminHandler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("admin-only"))
	})
}

func cookieFor(secret []byte, role auth.Role, gen uint64) *http.Cookie {
	return cookieForVer(secret, role, gen, "")
}

func cookieForVer(secret []byte, role auth.Role, gen uint64, adminVer string) *http.Cookie {
	return cookieForEntry(secret, role, gen, adminVer, "")
}

func cookieForEntry(secret []byte, role auth.Role, gen uint64, adminVer, entryID string) *http.Cookie {
	return &http.Cookie{
		Name:  auth.CookieName,
		Value: auth.Encode(secret, role, gen, adminVer, entryID, time.Hour),
	}
}

func TestRequireAdmin_RejectsAnonymous(t *testing.T) {
	secret := newTestSecret()
	srv := httptest.NewServer(middleware.RequireAdmin(secret, "av-test", adminHandler()))
	defer srv.Close()

	resp, err := http.Get(srv.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("anonymous: got %d, want 403", resp.StatusCode)
	}
}

func TestRequireAdmin_RejectsUserRole(t *testing.T) {
	secret := newTestSecret()
	srv := httptest.NewServer(middleware.RequireAdmin(secret, "av-test", adminHandler()))
	defer srv.Close()

	req, _ := http.NewRequest(http.MethodGet, srv.URL, nil)
	req.AddCookie(cookieFor(secret, auth.RoleUser, 0))
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("user role: got %d, want 403 — privilege escalation possible", resp.StatusCode)
	}
}

func TestRequireAdmin_AcceptsAdminRole(t *testing.T) {
	secret := newTestSecret()
	srv := httptest.NewServer(middleware.RequireAdmin(secret, "av-test", adminHandler()))
	defer srv.Close()

	req, _ := http.NewRequest(http.MethodGet, srv.URL, nil)
	req.AddCookie(cookieForVer(secret, auth.RoleAdmin, 0, "av-test"))
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("admin role: got %d, want 200", resp.StatusCode)
	}
}

// An admin cookie minted under a previous APP_ADMIN_PASSWORD_HASH carries a stale
// AdminVersion fingerprint and must be rejected after restart, so rotating
// the admin password via redeploy actually invalidates old admin sessions.
func TestRequireAdmin_RejectsStaleAdminVersion(t *testing.T) {
	secret := newTestSecret()
	srv := httptest.NewServer(middleware.RequireAdmin(secret, "av-current", adminHandler()))
	defer srv.Close()

	req, _ := http.NewRequest(http.MethodGet, srv.URL, nil)
	req.AddCookie(cookieForVer(secret, auth.RoleAdmin, 0, "av-old"))
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("stale admin cookie: got %d, want 403", resp.StatusCode)
	}
}

func TestRequireAdmin_RejectsForgedCookie(t *testing.T) {
	secret := newTestSecret()
	srv := httptest.NewServer(middleware.RequireAdmin(secret, "av-test", adminHandler()))
	defer srv.Close()

	// Cookie minted with the wrong secret — simulates an attacker without server access.
	attackerSecret := []byte("ffffffffffffffffffffffffffffffff")
	forged := cookieFor(attackerSecret, auth.RoleAdmin, 0)

	req, _ := http.NewRequest(http.MethodGet, srv.URL, nil)
	req.AddCookie(forged)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("forged cookie: got %d, want 403", resp.StatusCode)
	}
}

func TestLogin_AdminPasswordYieldsAdminRole(t *testing.T) {
	secret := newTestSecret()
	connPass := mustEmptyStore(t)
	limiter := auth.NewAuthLimiter(100, time.Minute)

	srv := httptest.NewServer(handler.Login(handler.LoginConfig{
		AdminPasswordHash: mustHash(t, "correct-admin-pass"),
		AdminVer:          "av-test",
		CookieSecure:      false,
		SessionSecret:     secret,
		ConnPass:          connPass,
		Limiter:           limiter,
		Trusted:           config.DefaultTrustedProxies(),
	}))
	defer srv.Close()

	resp := postLogin(t, srv.URL, "correct-admin-pass")
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusNoContent {
		t.Fatalf("admin login: got %d, want 204", resp.StatusCode)
	}

	role := roleFromCookie(t, secret, resp)
	if role != auth.RoleAdmin {
		t.Fatalf("admin login: cookie role=%q, want admin", role)
	}
}

func TestLogin_ConnPassYieldsUserRole(t *testing.T) {
	secret := newTestSecret()
	connPass := mustEmptyStore(t)
	_, plain, err := connPass.Create("test", 0)
	if err != nil {
		t.Fatalf("create: %v", err)
	}

	limiter := auth.NewAuthLimiter(100, time.Minute)
	srv := httptest.NewServer(handler.Login(handler.LoginConfig{
		AdminPasswordHash: mustHash(t, "correct-admin-pass"),
		AdminVer:          "av-test",
		CookieSecure:      false,
		SessionSecret:     secret,
		ConnPass:          connPass,
		Limiter:           limiter,
		Trusted:           config.DefaultTrustedProxies(),
	}))
	defer srv.Close()

	resp := postLogin(t, srv.URL, plain)
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusNoContent {
		t.Fatalf("user login: got %d, want 204", resp.StatusCode)
	}
	role := roleFromCookie(t, secret, resp)
	if role != auth.RoleUser {
		t.Fatalf("user login: cookie role=%q, want user", role)
	}
}

func TestLogin_GuestCannotEscalateByGuessingAdmin(t *testing.T) {
	secret := newTestSecret()
	connPass := mustEmptyStore(t)
	if _, _, err := connPass.Create("test", 0); err != nil {
		t.Fatal(err)
	}

	limiter := auth.NewAuthLimiter(100, time.Minute)
	srv := httptest.NewServer(handler.Login(handler.LoginConfig{
		AdminPasswordHash: mustHash(t, "correct-admin-pass"),
		AdminVer:          "av-test",
		CookieSecure:      false,
		SessionSecret:     secret,
		ConnPass:          connPass,
		Limiter:           limiter,
		Trusted:           config.DefaultTrustedProxies(),
	}))
	defer srv.Close()

	// Wrong password — neither admin nor SP — must not yield any session.
	resp := postLogin(t, srv.URL, "definitely-not-the-admin-password")
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("wrong password: got %d, want 401", resp.StatusCode)
	}
	for _, c := range resp.Cookies() {
		if c.Name == auth.CookieName && c.Value != "" {
			t.Fatalf("wrong password issued a session cookie")
		}
	}
}

func TestAuthenticated_StaleUserGenerationRejected(t *testing.T) {
	secret := newTestSecret()
	connPass := mustEmptyStore(t)
	entry, _, err := connPass.Create("test", 0)
	if err != nil {
		t.Fatal(err)
	}

	// Issue a user session at the current entry generation, then rotate the entry.
	cookie := cookieForEntry(secret, auth.RoleUser, entry.Generation, "", entry.ID)
	if _, _, err := connPass.Rotate(entry.ID); err != nil {
		t.Fatal(err)
	}

	req, _ := http.NewRequest(http.MethodGet, "/", nil)
	req.AddCookie(cookie)
	if auth.Authenticated(secret, connPass, "av-test", req) {
		t.Fatal("user session with stale generation accepted after rotate")
	}
}

// A user session for an entry that's been deleted must be rejected — otherwise
// "Удалить пароль" wouldn't actually disconnect users tied to that entry.
func TestAuthenticated_RevokedEntryRejectsUser(t *testing.T) {
	secret := newTestSecret()
	connPass := mustEmptyStore(t)
	entry, _, err := connPass.Create("test", 0)
	if err != nil {
		t.Fatal(err)
	}
	cookie := cookieForEntry(secret, auth.RoleUser, entry.Generation, "", entry.ID)
	if err := connPass.Revoke(entry.ID); err != nil {
		t.Fatal(err)
	}
	req, _ := http.NewRequest(http.MethodGet, "/", nil)
	req.AddCookie(cookie)
	if auth.Authenticated(secret, connPass, "av-test", req) {
		t.Fatal("user session for revoked entry accepted")
	}
}

func TestAuthenticated_AdminUnaffectedByRotate(t *testing.T) {
	secret := newTestSecret()
	connPass := mustEmptyStore(t)
	cookie := cookieForVer(secret, auth.RoleAdmin, 0, "av-test")
	entry, _, err := connPass.Create("test", 0)
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := connPass.Rotate(entry.ID); err != nil {
		t.Fatal(err)
	}

	req, _ := http.NewRequest(http.MethodGet, "/", nil)
	req.AddCookie(cookie)
	if !auth.Authenticated(secret, connPass, "av-test", req) {
		t.Fatal("admin session rejected after connpass rotate")
	}
}

// A stale admin cookie (issued under a previous APP_ADMIN_PASSWORD_HASH) must be
// rejected by the broad Authenticated check, not only by RequireAdmin —
// otherwise it could still reach /api/config and /ws endpoints.
func TestAuthenticated_StaleAdminVersionRejected(t *testing.T) {
	secret := newTestSecret()
	connPass := mustEmptyStore(t)
	cookie := cookieForVer(secret, auth.RoleAdmin, 0, "av-old")

	req, _ := http.NewRequest(http.MethodGet, "/", nil)
	req.AddCookie(cookie)
	if auth.Authenticated(secret, connPass, "av-current", req) {
		t.Fatal("stale admin cookie accepted by Authenticated")
	}
}

// ---- helpers ----

func mustEmptyStore(t *testing.T) *auth.ConnPassStore {
	t.Helper()
	store, err := auth.LoadConnPassStore(t.TempDir())
	if err != nil {
		t.Fatalf("load store: %v", err)
	}
	return store
}

func postLogin(t *testing.T, serverURL, password string) *http.Response {
	t.Helper()
	body := strings.NewReader("password=" + url.QueryEscape(password))
	req, _ := http.NewRequest(http.MethodPost, serverURL, body)
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func roleFromCookie(t *testing.T, secret []byte, resp *http.Response) auth.Role {
	t.Helper()
	for _, c := range resp.Cookies() {
		if c.Name != auth.CookieName {
			continue
		}
		sess, err := auth.Decode(secret, c.Value)
		if err != nil {
			t.Fatalf("decode cookie: %v", err)
		}
		return sess.Role
	}
	t.Fatal("no session cookie in response")
	return ""
}
