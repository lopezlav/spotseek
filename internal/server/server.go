package server

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"

	"spotseek-app/internal/libraryindex"
	"spotseek-app/internal/spotify"
)

// Version is the server version, reported by /health.
const Version = "1.5.0"

// logf adapts log.Print to the func(string) logger used by the index.
func logf(msg string) { log.Print(msg) }

// Server is the headless spotseek HTTP service.
type Server struct {
	mu              sync.Mutex
	settings        *Settings
	auth            *spotify.Auth
	index           *libraryindex.Index
	manager         *Manager
	started         time.Time
	profile         string
	profileAt       time.Time
	ctx             context.Context
	metadata        *metadataCache
	source          metadataSource
	catalog         *catalogService
	accountID       string
	accountSession  string
	identityAttempt time.Time
	scanMu          sync.Mutex
	scanFlight      *indexScanFlight
	scanWG          sync.WaitGroup
	backgroundWG    sync.WaitGroup
}

// New prepares the server, loads the index and starts a background scan.
func New(ctx context.Context, settings *Settings, auth *spotify.Auth) (*Server, error) {
	if err := os.MkdirAll(settings.OutputDir, 0o755); err != nil {
		return nil, err
	}
	indexPath := filepath.Join(settings.OutputDir, ".spotseek", "library-index.json")
	index := libraryindex.New(indexPath, settings.OutputDir)
	if err := index.Load(); err != nil {
		log.Printf("index: %v", err)
	}
	cache, err := newMetadataCache(filepath.Join(settings.OutputDir, ".spotseek", "metadata.json"))
	if err != nil {
		return nil, fmt.Errorf("metadata cache: %w", err)
	}
	s := &Server{
		settings: settings,
		auth:     auth,
		index:    index,
		manager:  NewManager(ctx, settings, index),
		started:  time.Now(),
		ctx:      ctx,
		metadata: cache,
		source:   auth,
		catalog:  newCatalogService(auth),
	}
	s.startIndexScan()
	return s, nil
}

// Settings returns the current settings.
func (s *Server) Settings() *Settings {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.settings
}

// Handler returns the HTTP routes.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/v1/health", s.handleHealth)
	mux.HandleFunc("GET /api/v1/settings", s.handleGetSettings)
	mux.HandleFunc("PUT /api/v1/settings", s.handlePutSettings)
	mux.HandleFunc("GET /api/v1/auth/status", s.handleAuthStatus)
	mux.HandleFunc("POST /api/v1/auth/client-id", s.handleClientID)
	mux.HandleFunc("POST /api/v1/auth/login", s.handleLogin)
	mux.HandleFunc("POST /api/v1/auth/logout", s.handleLogout)
	mux.HandleFunc("GET /api/v1/library", s.handleLibrary)
	mux.HandleFunc("GET /api/v1/catalog/search", s.handleSearch)
	mux.HandleFunc("GET /api/v1/catalog/downloaded", s.handleDownloaded)
	mux.HandleFunc("GET /api/v1/artists/{id}/albums", s.handleArtistAlbums)
	mux.HandleFunc("GET /api/v1/collections/{kind}/{id}/tracks", s.handleCollectionTracks)
	mux.HandleFunc("POST /api/v1/extract", s.handleExtract)
	mux.HandleFunc("POST /api/v1/index/lookup", s.handleLookup)
	mux.HandleFunc("POST /api/v1/index/rescan", s.handleRescan)
	mux.HandleFunc("POST /api/v1/downloads", s.handleStartDownloads)
	mux.HandleFunc("GET /api/v1/downloads", s.handleGetDownloads)
	mux.HandleFunc("POST /api/v1/downloads/cancel", s.handleCancelDownloads)
	return s.withLogging(mux)
}

func (s *Server) withLogging(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		next.ServeHTTP(w, r)
		if d := time.Since(start); d > 2*time.Second {
			log.Printf("%s %s took %s", r.Method, r.URL.Path, d.Round(time.Millisecond))
		}
	})
}

// Run serves HTTP until the context is canceled.
func (s *Server) Run(ctx context.Context) error {
	srv := &http.Server{
		Addr:              s.Settings().Addr,
		Handler:           s.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}
	go func() {
		<-ctx.Done()
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutdownCtx)
	}()
	err := srv.ListenAndServe()
	if errors.Is(err, http.ErrServerClosed) {
		s.Wait()
		return nil
	}
	return err
}
