package handler

import (
	"bytes"
	"encoding/json"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"voice-hub/backend/internal/auth"
	"voice-hub/backend/internal/filestore"
	"voice-hub/backend/internal/middleware"
)

// authEnv holds the secret + cookie + connpass needed to drive the upload and
// download handlers through their RequireAuthAPI gate.
type authEnv struct {
	secret   []byte
	connPass *auth.ConnPassStore
	adminVer string
	cookie   *http.Cookie
}

func newAuthEnv(t *testing.T) authEnv {
	t.Helper()
	secret := []byte("0123456789abcdef0123456789abcdef")
	connPass, err := auth.LoadConnPassStore(t.TempDir())
	if err != nil {
		t.Fatalf("connpass store: %v", err)
	}
	entry, _, err := connPass.Create("test", 0)
	if err != nil {
		t.Fatalf("create connpass entry: %v", err)
	}
	const adminVer = "av-test"
	cookie := &http.Cookie{Name: auth.CookieName, Value: auth.Encode(secret, auth.RoleUser, entry.Generation, "", entry.ID, time.Hour)}
	return authEnv{secret: secret, connPass: connPass, adminVer: adminVer, cookie: cookie}
}

func newFileStore(t *testing.T, cfg filestore.Config) *filestore.Store {
	t.Helper()
	cfg.TempDir = t.TempDir()
	if cfg.JanitorInterval == 0 {
		cfg.JanitorInterval = time.Hour
	}
	if cfg.TTL == 0 {
		cfg.TTL = time.Minute
	}
	if cfg.TTLHardCap == 0 {
		cfg.TTLHardCap = time.Hour
	}
	s, err := filestore.New(cfg)
	if err != nil {
		t.Fatalf("filestore.New: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func onlyRoom1(roomID string) bool { return roomID == "room1" }

// fileMux wires both handlers behind RequireAuthAPI exactly as production does,
// so PathValue and the auth gate behave identically in tests.
func fileMux(env authEnv, store *filestore.Store) *http.ServeMux {
	mux := http.NewServeMux()
	mux.Handle("POST /api/upload", middleware.RequireAuthAPI(env.secret, env.connPass, env.adminVer, UploadFile(store, onlyRoom1)))
	mux.Handle("GET /api/file/{uploadID}", middleware.RequireAuthAPI(env.secret, env.connPass, env.adminVer, DownloadFile(store)))
	return mux
}

func TestUpload_Unauthorized(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	mux := fileMux(env, newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20}))

	req := httptest.NewRequest(http.MethodPost, "/api/upload?room=room1", strings.NewReader("hi"))
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status=%d, want 401", rec.Code)
	}
}

func TestUpload_UnknownRoom(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	mux := fileMux(env, newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20}))

	req := httptest.NewRequest(http.MethodPost, "/api/upload?room=ghost", strings.NewReader("hi"))
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusNotFound {
		t.Fatalf("status=%d, want 404", rec.Code)
	}
}

func TestUpload_TooLarge(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	mux := fileMux(env, newFileStore(t, filestore.Config{MaxUploadBytes: 10, MaxTotalBytes: 1 << 20}))

	req := httptest.NewRequest(http.MethodPost, "/api/upload?room=room1", strings.NewReader(strings.Repeat("a", 100)))
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("status=%d, want 413", rec.Code)
	}
}

func TestUpload_BudgetExceeded(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	mux := fileMux(env, newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 3}))

	req := httptest.NewRequest(http.MethodPost, "/api/upload?room=room1", strings.NewReader("12345"))
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusInsufficientStorage {
		t.Fatalf("status=%d, want 507", rec.Code)
	}
}

func TestUpload_RawWithContentDisposition(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	store := newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20})
	mux := fileMux(env, store)

	req := httptest.NewRequest(http.MethodPost, "/api/upload?room=room1", strings.NewReader("plain text body"))
	req.Header.Set("Content-Disposition", `attachment; filename="notes.txt"`)
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusCreated {
		t.Fatalf("status=%d, want 201 (body=%q)", rec.Code, rec.Body.String())
	}
	var resp struct {
		UploadID string `json:"uploadId"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if resp.UploadID == "" {
		t.Fatal("empty uploadId in response")
	}
	entry, ok := store.Get(resp.UploadID)
	if !ok {
		t.Fatal("stored entry not retrievable")
	}
	if entry.Name != "notes.txt" {
		t.Errorf("Name=%q, want notes.txt", entry.Name)
	}
	if !strings.HasPrefix(entry.MIME, "text/plain") {
		t.Errorf("MIME=%q, want text/plain*", entry.MIME)
	}
}

func TestUpload_Multipart(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	store := newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20})
	mux := fileMux(env, store)

	var body bytes.Buffer
	mw := multipart.NewWriter(&body)
	part, err := mw.CreateFormFile("file", "pic.png")
	if err != nil {
		t.Fatalf("CreateFormFile: %v", err)
	}
	// PNG magic so DetectContentType reports image/png.
	pngMagic := []byte{0x89, 'P', 'N', 'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0}
	if _, err := part.Write(pngMagic); err != nil {
		t.Fatalf("write part: %v", err)
	}
	if err := mw.Close(); err != nil {
		t.Fatalf("close multipart: %v", err)
	}

	req := httptest.NewRequest(http.MethodPost, "/api/upload?room=room1", &body)
	req.Header.Set("Content-Type", mw.FormDataContentType())
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusCreated {
		t.Fatalf("status=%d, want 201 (body=%q)", rec.Code, rec.Body.String())
	}
	var resp struct {
		UploadID string `json:"uploadId"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	entry, ok := store.Get(resp.UploadID)
	if !ok {
		t.Fatal("stored entry not retrievable")
	}
	if entry.Name != "pic.png" {
		t.Errorf("Name=%q, want pic.png", entry.Name)
	}
	if entry.MIME != "image/png" {
		t.Errorf("MIME=%q, want image/png", entry.MIME)
	}
}
