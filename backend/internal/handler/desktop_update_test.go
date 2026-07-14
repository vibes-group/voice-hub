package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

func writeDesktopFixture(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	manifest := "{\"version\":\"1.2.3\",\"platforms\":{\"windows-x86_64\":{\"signature\":\"signed\",\"url\":\"https://github.com/example/app/releases/download/v1.2.3/Voice.Hub_1.2.3_x64-setup.exe\"}}}"
	if err := os.WriteFile(filepath.Join(dir, "latest.json"), []byte(manifest), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "Voice.Hub_1.2.3_x64-setup.exe"), []byte("installer"), 0o600); err != nil {
		t.Fatal(err)
	}
	return dir
}

func TestDesktopUpdateManifestUsesRequestHost(t *testing.T) {
	dir := writeDesktopFixture(t)
	req := httptest.NewRequest(http.MethodGet, "http://voice.example.com/desktop/latest.json", nil)
	req.Header.Set("X-Forwarded-Proto", "https")
	rec := httptest.NewRecorder()

	DesktopUpdateManifest(dir).ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200", rec.Code)
	}
	var got desktopManifest
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	want := "https://voice.example.com/desktop/files/Voice.Hub_1.2.3_x64-setup.exe"
	if got.Platforms["windows-x86_64"].URL != want {
		t.Fatalf("url = %q, want %q", got.Platforms["windows-x86_64"].URL, want)
	}
}

func TestDesktopDownloadWindowsRedirectsToCurrentInstaller(t *testing.T) {
	dir := writeDesktopFixture(t)
	rec := httptest.NewRecorder()
	DesktopDownloadWindows(dir).ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/desktop/download/windows", nil))

	if rec.Code != http.StatusFound {
		t.Fatalf("status = %d, want 302", rec.Code)
	}
	if got, want := rec.Header().Get("Location"), "/desktop/files/Voice.Hub_1.2.3_x64-setup.exe"; got != want {
		t.Fatalf("location = %q, want %q", got, want)
	}
}

func TestDesktopUpdateFileRejectsTraversal(t *testing.T) {
	dir := writeDesktopFixture(t)
	req := httptest.NewRequest(http.MethodGet, "/desktop/files/anything", nil)
	req.SetPathValue("name", "../latest.json")
	rec := httptest.NewRecorder()

	DesktopUpdateFile(dir).ServeHTTP(rec, req)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404", rec.Code)
	}
}
