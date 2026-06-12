package handler

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"voice-hub/backend/internal/filestore"
)

// putFile registers content directly in the store, bypassing the upload
// handler, so download behaviour can be tested in isolation.
func putFile(t *testing.T, s *filestore.Store, room, name, mimeType, content string) *filestore.Entry {
	t.Helper()
	f, err := s.CreateTemp()
	if err != nil {
		t.Fatalf("CreateTemp: %v", err)
	}
	if _, err := f.WriteString(content); err != nil {
		t.Fatalf("write: %v", err)
	}
	if err := f.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	entry, err := s.Store(room, name, mimeType, int64(len(content)), f.Name())
	if err != nil {
		t.Fatalf("Store: %v", err)
	}
	return entry
}

func TestDownload_Unauthorized(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	mux := fileMux(env, newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20}))

	req := httptest.NewRequest(http.MethodGet, "/api/file/whatever?room=room1", nil)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status=%d, want 401", rec.Code)
	}
}

func TestDownload_UnknownID(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	mux := fileMux(env, newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20}))

	req := httptest.NewRequest(http.MethodGet, "/api/file/nope?room=room1", nil)
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusNotFound {
		t.Fatalf("status=%d, want 404", rec.Code)
	}
}

func TestDownload_AfterEviction(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	// Negative TTL means an entry is born already expired, so Get reports it
	// gone without any clock manipulation.
	store := newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20, TTL: -time.Second, TTLHardCap: time.Hour})
	mux := fileMux(env, store)

	entry := putFile(t, store, "room1", "a.txt", "text/plain", "gone soon")
	req := httptest.NewRequest(http.MethodGet, "/api/file/"+entry.UploadID+"?room=room1", nil)
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusNotFound {
		t.Fatalf("status=%d, want 404 after eviction", rec.Code)
	}
}

func TestDownload_CrossRoom(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	store := newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20})
	mux := fileMux(env, store)

	entry := putFile(t, store, "room2", "a.txt", "text/plain", "secret")
	req := httptest.NewRequest(http.MethodGet, "/api/file/"+entry.UploadID+"?room=room1", nil)
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusNotFound {
		t.Fatalf("status=%d, want 404 for cross-room access", rec.Code)
	}
}

func TestDownload_RangeRequest(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	store := newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20})
	mux := fileMux(env, store)

	entry := putFile(t, store, "room1", "a.txt", "text/plain", "0123456789")
	req := httptest.NewRequest(http.MethodGet, "/api/file/"+entry.UploadID+"?room=room1", nil)
	req.Header.Set("Range", "bytes=0-3")
	req.AddCookie(env.cookie)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusPartialContent {
		t.Fatalf("status=%d, want 206", rec.Code)
	}
	if got := rec.Body.String(); got != "0123" {
		t.Errorf("body=%q, want %q", got, "0123")
	}
}

func TestDownload_ContentDisposition(t *testing.T) {
	t.Parallel()
	env := newAuthEnv(t)
	store := newFileStore(t, filestore.Config{MaxUploadBytes: 1 << 20, MaxTotalBytes: 1 << 20})
	mux := fileMux(env, store)

	tests := []struct {
		name     string
		mimeType string
		want     string // expected Content-Disposition prefix
	}{
		{name: "image inline", mimeType: "image/png", want: "inline"},
		{name: "file attachment", mimeType: "application/pdf", want: "attachment"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			entry := putFile(t, store, "room1", "f.bin", tc.mimeType, "body")
			req := httptest.NewRequest(http.MethodGet, "/api/file/"+entry.UploadID+"?room=room1", nil)
			req.AddCookie(env.cookie)
			rec := httptest.NewRecorder()
			mux.ServeHTTP(rec, req)

			if rec.Code != http.StatusOK {
				t.Fatalf("status=%d, want 200", rec.Code)
			}
			disp := rec.Header().Get("Content-Disposition")
			if !hasDispositionType(disp, tc.want) {
				t.Errorf("Content-Disposition=%q, want %s ...", disp, tc.want)
			}
			if ct := rec.Header().Get("Content-Type"); ct != tc.mimeType {
				t.Errorf("Content-Type=%q, want %q", ct, tc.mimeType)
			}
		})
	}
}

func hasDispositionType(header, want string) bool {
	return len(header) >= len(want) && header[:len(want)] == want
}
