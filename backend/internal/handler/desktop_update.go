package handler

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"strings"
)

type desktopManifest struct {
	Version   string                     `json:"version"`
	Notes     string                     `json:"notes,omitempty"`
	PubDate   string                     `json:"pub_date,omitempty"`
	Platforms map[string]desktopPlatform `json:"platforms"`
}

type desktopPlatform struct {
	Signature string `json:"signature"`
	URL       string `json:"url"`
}

func readDesktopManifest(updatesDir string) (desktopManifest, error) {
	data, err := os.ReadFile(filepath.Join(updatesDir, "latest.json"))
	if err != nil {
		return desktopManifest{}, err
	}
	var manifest desktopManifest
	if err := json.Unmarshal(data, &manifest); err != nil {
		return desktopManifest{}, fmt.Errorf("decode latest.json: %w", err)
	}
	if manifest.Version == "" || len(manifest.Platforms) == 0 {
		return desktopManifest{}, fmt.Errorf("latest.json: missing version or platforms")
	}
	return manifest, nil
}

func desktopFilename(rawURL string) (string, error) {
	parsed, err := url.Parse(rawURL)
	if err != nil {
		return "", fmt.Errorf("parse artifact url: %w", err)
	}
	name := path.Base(parsed.Path)
	if name == "" || name == "." || name == "/" || filepath.Base(name) != name {
		return "", fmt.Errorf("invalid artifact filename")
	}
	return name, nil
}

func requestOrigin(req *http.Request) string {
	scheme := "https"
	if req.TLS == nil {
		if forwarded := strings.TrimSpace(strings.Split(req.Header.Get("X-Forwarded-Proto"), ",")[0]); forwarded == "http" || forwarded == "https" {
			scheme = forwarded
		} else if strings.HasPrefix(req.Host, "localhost:") || req.Host == "localhost" || strings.HasPrefix(req.Host, "127.0.0.1:") {
			scheme = "http"
		}
	}
	return (&url.URL{Scheme: scheme, Host: req.Host}).String()
}

// DesktopUpdateManifest serves the mirrored Tauri manifest. Artifact URLs are
// rewritten to the hostname used for this request, so a desktop client always
// downloads from the server selected in the app.
func DesktopUpdateManifest(updatesDir string) http.HandlerFunc {
	return func(w http.ResponseWriter, req *http.Request) {
		manifest, err := readDesktopManifest(updatesDir)
		if err != nil {
			http.NotFound(w, req)
			return
		}
		origin := requestOrigin(req)
		for target, platform := range manifest.Platforms {
			name, err := desktopFilename(platform.URL)
			if err != nil {
				http.Error(w, "invalid update manifest", http.StatusInternalServerError)
				return
			}
			platform.URL = origin + "/desktop/files/" + url.PathEscape(name)
			manifest.Platforms[target] = platform
		}

		w.Header().Set("Cache-Control", "no-cache")
		w.Header().Set("Content-Type", "application/json")
		if err := json.NewEncoder(w).Encode(manifest); err != nil {
			return
		}
	}
}

// DesktopDownloadWindows redirects the website button to the current Windows
// installer without exposing a versioned filename in the frontend bundle.
func DesktopDownloadWindows(updatesDir string) http.HandlerFunc {
	return func(w http.ResponseWriter, req *http.Request) {
		manifest, err := readDesktopManifest(updatesDir)
		if err != nil {
			http.NotFound(w, req)
			return
		}
		platform, ok := manifest.Platforms["windows-x86_64"]
		if !ok {
			http.NotFound(w, req)
			return
		}
		name, err := desktopFilename(platform.URL)
		if err != nil {
			http.Error(w, "invalid update manifest", http.StatusInternalServerError)
			return
		}
		w.Header().Set("Cache-Control", "no-cache")
		http.Redirect(w, req, "/desktop/files/"+url.PathEscape(name), http.StatusFound)
	}
}

func DesktopUpdateFile(updatesDir string) http.HandlerFunc {
	return func(w http.ResponseWriter, req *http.Request) {
		name := req.PathValue("name")
		if name == "" || filepath.Base(name) != name {
			http.NotFound(w, req)
			return
		}
		filePath := filepath.Join(updatesDir, name)
		info, err := os.Stat(filePath)
		if err != nil || !info.Mode().IsRegular() {
			http.NotFound(w, req)
			return
		}
		w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
		w.Header().Set("Content-Disposition", fmt.Sprintf("attachment; filename=%q", name))
		http.ServeFile(w, req, filePath)
	}
}
