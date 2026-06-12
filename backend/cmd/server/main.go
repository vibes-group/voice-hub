package main

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"log"
	"net/http"
	_ "net/http/pprof"
	"os"
	"os/signal"
	"strconv"
	"syscall"
	"time"

	"voice-hub/backend/internal/auth"
	"voice-hub/backend/internal/config"
	"voice-hub/backend/internal/filestore"
	"voice-hub/backend/internal/handler"
	"voice-hub/backend/internal/middleware"
	"voice-hub/backend/internal/presence"
	"voice-hub/backend/internal/sfu"
	"voice-hub/backend/internal/sfu/protocol"
	turnsrv "voice-hub/backend/internal/turn"

	"github.com/pion/webrtc/v4"
)

func main() {
	cfg, err := config.Load()
	if err != nil {
		log.Fatalf("config: %v", err)
	}

	allowInsecure, _ := strconv.ParseBool(os.Getenv("APP_ALLOW_INSECURE"))
	if err := config.ValidateInsecureConfig(&cfg, allowInsecure); err != nil {
		log.Fatalf("config: %v", err)
	}
	if allowInsecure {
		log.Printf("WARNING: APP_ALLOW_INSECURE=1 — insecure dev mode active, do not expose publicly")
	}

	if cfg.AdminPassword == "" {
		log.Fatal("APP_ADMIN_PASSWORD must be set")
	}
	if cfg.PublicIP == "" {
		log.Fatal("PUBLIC_IP must be set (used by SFU NAT mapping and TURN relay address)")
	}

	// Default is "./data", which under Docker (WORKDIR /app) resolves to
	// /app/data — the path the Dockerfile creates and compose mounts as
	// a volume. Locally it just falls in the working directory.
	dataDir := "./data"

	sessionSecret, turnSecret, err := loadSecrets(dataDir)
	if err != nil {
		log.Fatalf("%v", err)
	}
	cfg.SessionSecret = sessionSecret
	cfg.TurnSharedSecret = hex.EncodeToString(turnSecret)

	connPass, err := auth.LoadConnPassStore(dataDir)
	if err != nil {
		log.Fatalf("connpass store: %v", err)
	}

	fileStore, err := filestore.New(filestore.Config{
		TempDir:         cfg.UploadTempDir,
		TTL:             cfg.UploadTTL,
		TTLHardCap:      cfg.UploadTTLHardCap,
		MaxUploadBytes:  cfg.UploadMaxBytes,
		MaxTotalBytes:   cfg.UploadTotalBytes,
		JanitorInterval: 30 * time.Second,
	})
	if err != nil {
		log.Fatalf("file store: %v", err)
	}
	// Runs at process exit, after the explicit server.Shutdown + room close
	// below have stopped all traffic into the store.
	defer fileStore.Close()

	// Sealed into admin cookies; rotating APP_ADMIN_PASSWORD requires restart,
	// so any old admin cookie's AdminVersion will mismatch this and be rejected.
	adminVer := auth.AdminPasswordVersion(sessionSecret, cfg.AdminPassword)
	wsRegistry := auth.NewWSRegistry()

	limiter := auth.NewAuthLimiter(10, 15*time.Minute)

	// Literal IP, not AppHostname: hostname may proxy through CDN that
	// drops UDP. IP only reaches clients via auth-gated /api/config.
	stunURL := "stun:" + cfg.PublicIP + ":3478"
	turnURL := "turn:" + cfg.PublicIP + ":3478?transport=udp"

	rooms, presenceHub, err := buildRooms([]string{"room1", "room2", "room3"}, cfg, stunURL, fileStore)
	if err != nil {
		log.Fatalf("%v", err)
	}
	presenceCtx, presenceCancel := context.WithCancel(context.Background())
	defer presenceCancel()
	go presenceHub.Run(presenceCtx)

	turnServer, err := turnsrv.Start(turnsrv.Config{
		Realm:        cfg.TurnRealm,
		SharedSecret: cfg.TurnSharedSecret,
		PublicIP:     cfg.PublicIP,
		ListenAddr:   "0.0.0.0:3478",
		MinRelayPort: cfg.TurnRelayMin,
		MaxRelayPort: cfg.TurnRelayMax,
	})
	if err != nil {
		log.Fatalf("turn init: %v", err)
	}

	version := handler.FrontendVersion(cfg.WebDir)
	log.Printf("frontend version: %s", version)

	mux := wireRoutes(cfg, adminVer, version, connPass, wsRegistry, limiter, rooms, presenceHub, fileStore, stunURL, turnURL)

	server := &http.Server{
		Addr:              cfg.Addr,
		Handler:           middleware.AccessLog(cfg.TrustedProxies, mux),
		ReadHeaderTimeout: 10 * time.Second,
		// ReadTimeout/WriteTimeout intentionally unset: /ws is a long-lived
		// WebSocket and per-request timeouts would terminate it. Auth gates
		// every other endpoint.
		IdleTimeout: 120 * time.Second,
	}

	log.Printf("auth enabled (cookie_secure=%v, connpass_entries=%d)", cfg.CookieSecure, len(connPass.Status().Entries))
	log.Printf("listening on %s, serving web from %s", cfg.Addr, cfg.WebDir)

	srvErr := make(chan error, 1)
	go func() {
		if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			srvErr <- err
		}
	}()

	// pprof off by default: heap dumps expose in-memory secrets (session secret, TURN secret, admin password).
	if v, _ := strconv.ParseBool(os.Getenv("APP_PPROF")); v {
		go func() {
			log.Printf("pprof: listening on 127.0.0.1:6060")
			srv := &http.Server{
				Addr:              "127.0.0.1:6060",
				Handler:           http.DefaultServeMux,
				ReadHeaderTimeout: 5 * time.Second,
			}
			if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
				log.Printf("pprof: %v", err)
			}
		}()
	}

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)

	select {
	case err := <-srvErr:
		log.Fatalf("http server: %v", err)
	case sig := <-stop:
		log.Printf("shutdown: received %s, draining...", sig)
	}

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := server.Shutdown(shutdownCtx); err != nil {
		log.Printf("shutdown: http: %v", err)
	}
	for _, rm := range rooms {
		rm.Close()
	}
	if err := turnServer.Close(); err != nil {
		log.Printf("shutdown: turn: %v", err)
	}
	log.Printf("shutdown: done")
}

// loadSecrets auto-bootstraps and persists the session and TURN secrets.
// Wiping the data dir invalidates all sessions, which is acceptable.
func loadSecrets(dataDir string) (sessionSecret, turnSecret []byte, err error) {
	sessionSecret, err = auth.LoadOrCreateSecret(dataDir, "session.secret", 32)
	if err != nil {
		return nil, nil, fmt.Errorf("session secret: %w", err)
	}
	turnSecret, err = auth.LoadOrCreateSecret(dataDir, "turn.secret", 32)
	if err != nil {
		return nil, nil, fmt.Errorf("turn secret: %w", err)
	}
	return sessionSecret, turnSecret, nil
}

// buildRooms creates the SFU rooms and the presence hub that fans out peer events.
func buildRooms(slugs []string, cfg config.Config, stunURL string, fileStore *filestore.Store) (map[string]*sfu.Room, *presence.Hub, error) {
	rooms := make(map[string]*sfu.Room, len(slugs))
	hub := presence.New(func() map[string]presence.RoomLister {
		out := make(map[string]presence.RoomLister, len(rooms))
		for slug, room := range rooms {
			out[slug] = room
		}
		return out
	})
	for _, slug := range slugs {
		rm, err := sfu.NewRoom(sfu.Config{
			ICEServers:    []webrtc.ICEServer{{URLs: []string{stunURL}}},
			NAT1To1IPs:    []string{cfg.PublicIP},
			UDPPortMin:    cfg.UDPPortMin,
			UDPPortMax:    cfg.UDPPortMax,
			AppHostname:   cfg.AppHostname,
			OnPeerJoined:  func(p protocol.PeerInfo) { hub.PeerJoined(slug, p) },
			OnPeerLeft:    func(id string) { hub.PeerLeft(slug, id) },
			OnPeerUpdated: func(p protocol.PeerInfo) { hub.PeerUpdated(slug, p) },
			RoomID:        slug,
			FileStore:     fileStore,
		})
		if err != nil {
			return nil, nil, fmt.Errorf("sfu init %q: %w", slug, err)
		}
		rooms[slug] = rm
	}
	return rooms, hub, nil
}

// wireRoutes registers all HTTP routes on a new ServeMux and returns it.
func wireRoutes(
	cfg config.Config,
	adminVer, version string,
	connPass *auth.ConnPassStore,
	wsRegistry *auth.WSRegistry,
	limiter *auth.AuthLimiter,
	rooms map[string]*sfu.Room,
	presenceHub *presence.Hub,
	fileStore *filestore.Store,
	stunURL, turnURL string,
) *http.ServeMux {
	resolveRoom := func(w http.ResponseWriter, req *http.Request) (*sfu.Room, bool) {
		rm, ok := rooms[req.PathValue("roomID")]
		if !ok {
			http.NotFound(w, req)
			return nil, false
		}
		return rm, true
	}
	roomExists := func(roomID string) bool {
		_, ok := rooms[roomID]
		return ok
	}

	mux := http.NewServeMux()
	htmlRoutes := middleware.RequireAuthHTML(cfg.SessionSecret, connPass, adminVer, http.FileServer(http.Dir(cfg.WebDir)))
	mux.Handle("/", http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		w.Header().Set("Permissions-Policy", "geolocation=(), payment=()")
		htmlRoutes.ServeHTTP(w, req)
	}))
	mux.HandleFunc("GET /healthz", handler.Health())
	mux.HandleFunc("GET /api/version", handler.Version(version))
	mux.HandleFunc("POST /api/login", handler.Login(handler.LoginConfig{
		AdminPassword: cfg.AdminPassword,
		AdminVer:      adminVer,
		CookieSecure:  cfg.CookieSecure,
		SessionSecret: cfg.SessionSecret,
		ConnPass:      connPass,
		Limiter:       limiter,
		Trusted:       cfg.TrustedProxies,
	}))
	mux.HandleFunc("POST /api/logout", handler.Logout(cfg.CookieSecure))
	mux.Handle("GET /api/presence", middleware.RequireAuthAPI(cfg.SessionSecret, connPass, adminVer,
		middleware.TrackWS(cfg.SessionSecret, wsRegistry, http.HandlerFunc(presenceHub.ServeSSE))))
	mux.Handle("GET /ws/{roomID}", middleware.RequireAuthAPI(cfg.SessionSecret, connPass, adminVer,
		middleware.TrackWS(cfg.SessionSecret, wsRegistry, http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			rm, ok := resolveRoom(w, req)
			if !ok {
				return
			}
			rm.ServeWS(w, req)
		}))))
	mux.Handle("GET /api/config", middleware.RequireAuthAPI(cfg.SessionSecret, connPass, adminVer, handler.Config(handler.ConfigOptions{
		SessionSecret:    cfg.SessionSecret,
		TurnSharedSecret: cfg.TurnSharedSecret,
		StunURL:          stunURL,
		TurnURL:          turnURL,
	})))
	mux.Handle("POST /api/upload", middleware.RequireAuthAPI(cfg.SessionSecret, connPass, adminVer, handler.UploadFile(fileStore, roomExists)))
	mux.Handle("GET /api/file/{uploadID}", middleware.RequireAuthAPI(cfg.SessionSecret, connPass, adminVer, handler.DownloadFile(fileStore)))
	mux.Handle("GET /api/admin/connection-passwords", middleware.RequireAdmin(cfg.SessionSecret, adminVer, handler.ConnPassStatus(connPass)))
	mux.Handle("POST /api/admin/connection-passwords", middleware.RequireAdmin(cfg.SessionSecret, adminVer, handler.ConnPassCreate(cfg.AppHostname, connPass)))
	mux.Handle("POST /api/admin/connection-passwords/{id}/rotate", middleware.RequireAdmin(cfg.SessionSecret, adminVer, handler.ConnPassRotate(cfg.AppHostname, connPass)))
	mux.Handle("POST /api/admin/connection-passwords/{id}/rename", middleware.RequireAdmin(cfg.SessionSecret, adminVer, handler.ConnPassRename(connPass)))
	mux.Handle("POST /api/admin/connection-passwords/{id}/ttl", middleware.RequireAdmin(cfg.SessionSecret, adminVer, handler.ConnPassSetTTL(connPass)))
	mux.Handle("DELETE /api/admin/connection-passwords/{id}", middleware.RequireAdmin(cfg.SessionSecret, adminVer, handler.ConnPassRevoke(connPass)))
	mux.Handle("POST /api/admin/connection-passwords/disconnect-users", middleware.RequireAdmin(cfg.SessionSecret, adminVer, handler.DisconnectUsers(wsRegistry)))
	return mux
}
