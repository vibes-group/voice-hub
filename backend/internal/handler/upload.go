package handler

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"mime"
	"net/http"
	"net/url"
	"os"
	"strings"

	"voice-hub/backend/internal/filestore"
)

type uploadResponse struct {
	UploadID string `json:"uploadId"`
}

// UploadFile handles POST /api/upload?room={roomID}, streaming the body to a
// transient temp file and registering it. roomExists rejects unknown rooms so a
// stray upload can't occupy the budget.
func UploadFile(store *filestore.Store, roomExists func(string) bool) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		room := r.URL.Query().Get("room")
		if room == "" || !roomExists(room) {
			http.NotFound(w, r)
			return
		}

		r.Body = http.MaxBytesReader(w, r.Body, store.MaxUploadBytes()+1)

		name, src, err := uploadSource(r)
		if err != nil {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}

		f, err := store.CreateTemp()
		if err != nil {
			log.Printf("upload: create temp: %v", err)
			http.Error(w, "upload failed", http.StatusInternalServerError)
			return
		}
		tempPath := f.Name()

		sniffer := &headSniffer{}
		size, copyErr := io.Copy(f, io.TeeReader(src, sniffer))
		closeErr := f.Close()
		if copyErr != nil {
			_ = os.Remove(tempPath)
			var maxErr *http.MaxBytesError
			if errors.As(copyErr, &maxErr) {
				http.Error(w, "file too large", http.StatusRequestEntityTooLarge)
				return
			}
			log.Printf("upload: copy: %v", copyErr)
			http.Error(w, "upload failed", http.StatusInternalServerError)
			return
		}
		if closeErr != nil {
			_ = os.Remove(tempPath)
			log.Printf("upload: close temp: %v", closeErr)
			http.Error(w, "upload failed", http.StatusInternalServerError)
			return
		}

		mimeType := http.DetectContentType(sniffer.head)
		entry, err := store.Store(room, sanitizeName(name), mimeType, size, tempPath)
		if errors.Is(err, filestore.ErrBudgetExceeded) {
			_ = os.Remove(tempPath)
			http.Error(w, "server storage full", http.StatusInsufficientStorage)
			return
		}
		if err != nil {
			_ = os.Remove(tempPath)
			log.Printf("upload: store: %v", err)
			http.Error(w, "upload failed", http.StatusInternalServerError)
			return
		}

		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusCreated)
		if err := json.NewEncoder(w).Encode(uploadResponse{UploadID: entry.UploadID}); err != nil {
			log.Printf("upload: encode: %v", err)
		}
	}
}

// DownloadFile handles GET /api/file/{uploadID}?room={roomID}, serving the
// transient file (images inline, else attachment) and extending its TTL.
func DownloadFile(store *filestore.Store) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := r.PathValue("uploadID")
		room := r.URL.Query().Get("room")

		entry, ok := store.Get(id)
		// Room scoping is cheap hardening — the id alone shouldn't grant
		// cross-room access.
		if !ok || entry.RoomID != room {
			http.NotFound(w, r)
			return
		}

		f, err := os.Open(entry.TempPath)
		if err != nil {
			http.NotFound(w, r) // raced with eviction
			return
		}
		defer f.Close()

		disposition := "attachment"
		if strings.HasPrefix(entry.MIME, "image/") {
			disposition = "inline"
		}
		w.Header().Set("Content-Type", entry.MIME)
		w.Header().Set("Content-Disposition", contentDisposition(disposition, entry.Name))
		http.ServeContent(w, r, entry.Name, entry.CreatedAt, f)

		store.Touch(id)
	}
}

// uploadSource returns the filename and reader for the upload body, handling
// both a raw body (name from Content-Disposition) and a single multipart file
// part. r.Body is assumed already wrapped in a MaxBytesReader.
func uploadSource(r *http.Request) (name string, src io.Reader, err error) {
	mediaType, _, _ := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if strings.HasPrefix(mediaType, "multipart/") {
		mr, err := r.MultipartReader()
		if err != nil {
			return "", nil, err
		}
		for {
			part, err := mr.NextPart()
			if err != nil {
				return "", nil, err
			}
			if part.FileName() != "" {
				return part.FileName(), part, nil
			}
		}
	}
	return filenameFromContentDisposition(r.Header.Get("Content-Disposition")), r.Body, nil
}

// filenameFromContentDisposition extracts the filename param, decoding the
// RFC 5987 extended form (filename*=) when present.
func filenameFromContentDisposition(header string) string {
	if header == "" {
		return ""
	}
	_, params, err := mime.ParseMediaType(header)
	if err != nil {
		return ""
	}
	return params["filename"]
}

// sanitizeName strips any path component so an upload name can never escape the
// temp dir or mislead the download filename, falling back to "file".
func sanitizeName(name string) string {
	name = strings.TrimSpace(name)
	if i := strings.LastIndexAny(name, `/\`); i >= 0 {
		name = name[i+1:]
	}
	if name == "" {
		return "file"
	}
	return name
}

// contentDisposition builds a header value with an ASCII-safe filename and the
// RFC 5987 UTF-8 form for clients that support it.
func contentDisposition(disposition, filename string) string {
	if filename == "" {
		return disposition
	}
	ascii := strings.Map(func(r rune) rune {
		if r < 0x20 || r > 0x7e || r == '"' || r == '\\' {
			return '_'
		}
		return r
	}, filename)
	return fmt.Sprintf("%s; filename=%q; filename*=UTF-8''%s", disposition, ascii, url.PathEscape(filename))
}

// headSniffer captures the first 512 bytes written through it for MIME
// detection, discarding the rest. It never errors so io.Copy is unaffected.
type headSniffer struct{ head []byte }

func (h *headSniffer) Write(p []byte) (int, error) {
	if room := 512 - len(h.head); room > 0 {
		if room > len(p) {
			room = len(p)
		}
		h.head = append(h.head, p[:room]...)
	}
	return len(p), nil
}
