package server

import (
	"context"
	"encoding/json"
	"log"
	"net/http"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"spotseek-app/internal/spotify"
)

type trackDTO struct {
	ID       string `json:"id,omitempty"`
	Artist   string `json:"artist,omitempty"`
	Title    string `json:"title"`
	Album    string `json:"album,omitempty"`
	Duration int    `json:"duration,omitempty"`
}

func toTrackDTO(t spotify.Track) trackDTO {
	return trackDTO{
		ID:       t.ID,
		Artist:   t.Artist,
		Title:    t.Title,
		Album:    t.Album,
		Duration: t.DurationSec,
	}
}

func (d trackDTO) toTrack() spotify.Track {
	return spotify.Track{
		ID:          d.ID,
		Artist:      d.Artist,
		Title:       d.Title,
		Album:       d.Album,
		DurationSec: d.Duration,
	}
}

func toTrackDTOs(tracks []spotify.Track) []trackDTO {
	out := make([]trackDTO, 0, len(tracks))
	for _, t := range tracks {
		out = append(out, toTrackDTO(t))
	}
	return out
}

type libraryItemDTO struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	Subtitle string `json:"subtitle,omitempty"`
	Count    int    `json:"count,omitempty"`
	Kind     string `json:"kind"`
}

func toLibraryItemDTOs(items []spotify.LibraryItem) []libraryItemDTO {
	out := make([]libraryItemDTO, 0, len(items))
	for _, it := range items {
		out = append(out, libraryItemDTO{
			ID:       it.ID,
			Name:     it.Name,
			Subtitle: it.Subtitle,
			Count:    it.Count,
			Kind:     it.Kind,
		})
	}
	return out
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	if err := json.NewEncoder(w).Encode(v); err != nil {
		log.Printf("write response: %v", err)
	}
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

func readJSON(r *http.Request, v any) error {
	defer r.Body.Close()
	dec := json.NewDecoder(http.MaxBytesReader(nil, r.Body, 8<<20))
	return dec.Decode(v)
}

func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	settings := s.Settings()
	s.warmIdentity()
	identity := s.savedIdentity()
	writeJSON(w, http.StatusOK, map[string]any{
		"account_id":  identity.ID,
		"ok":          true,
		"version":     Version,
		"uptime_sec":  int(time.Since(s.started).Seconds()),
		"logged_in":   s.source.CacheKey() != "",
		"profile":     s.cachedProfile(r.Context()),
		"output_dir":  settings.OutputDir,
		"active_jobs": s.manager.ActiveCountForAccount(identity.ID),
	})
}

func (s *Server) cachedProfile(ctx context.Context) string {
	return s.savedIdentity().Name
}

func (s *Server) handleGetSettings(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.Settings())
}

func (s *Server) handlePutSettings(w http.ResponseWriter, r *http.Request) {
	current := *s.Settings()
	next := current
	if err := readJSON(r, &next); err != nil {
		writeError(w, http.StatusBadRequest, "bad JSON: "+err.Error())
		return
	}
	next.applyDefaults()
	if err := next.Save(); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.mu.Lock()
	s.settings = &next
	s.mu.Unlock()
	if next.ClientID != current.ClientID {
		s.auth.SetClientID(next.ClientID)
		s.clearIdentity()
	}
	if next.OutputDir != current.OutputDir {
		if err := s.metadata.setPath(filepath.Join(next.OutputDir, ".spotseek", "metadata.json")); err != nil {
			log.Printf("metadata cache after folder change: %v", err)
		}
	}
	s.index.SetRoot(next.OutputDir)
	s.manager.SetSettings(&next)
	s.startIndexScan()
	writeJSON(w, http.StatusOK, &next)
}

func (s *Server) handleAuthStatus(w http.ResponseWriter, r *http.Request) {
	settings := s.Settings()
	s.warmIdentity()
	writeJSON(w, http.StatusOK, map[string]any{
		"logged_in":     s.source.CacheKey() != "",
		"account_id":    s.savedIdentity().ID,
		"profile":       s.cachedProfile(r.Context()),
		"client_id_set": strings.TrimSpace(settings.ClientID) != "",
		"redirect_uri":  spotify.RedirectURL(),
	})
}

func (s *Server) handleClientID(w http.ResponseWriter, r *http.Request) {
	var body struct {
		ClientID string `json:"client_id"`
	}
	if err := readJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, "bad JSON: "+err.Error())
		return
	}
	body.ClientID = strings.TrimSpace(body.ClientID)
	if body.ClientID == "" {
		writeError(w, http.StatusBadRequest, "client_id is empty")
		return
	}
	s.mu.Lock()
	s.settings.ClientID = body.ClientID
	settings := *s.settings
	s.auth.SetClientID(body.ClientID)
	s.mu.Unlock()
	s.clearIdentity()
	if err := settings.Save(); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

func (s *Server) handleLogin(w http.ResponseWriter, r *http.Request) {
	s.mu.Lock()
	clientID := strings.TrimSpace(s.settings.ClientID)
	s.auth.SetClientID(clientID)
	s.mu.Unlock()
	if clientID == "" {
		writeError(w, http.StatusBadRequest, "no Spotify client id is set")
		return
	}
	if err := s.auth.Login(r.Context(), openBrowser); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.clearIdentity()
	identity, err := s.ensureIdentity(r.Context(), true)
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"ok": true, "profile": identity.Name, "account_id": identity.ID,
	})
}

func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	s.auth.Logout()
	s.clearIdentity()
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "account_id": ""})
}

// namePrefix marks an artist that has no Spotify id yet. The server resolves
// it with a search when the artist is opened.
const namePrefix = "name:"

// deriveArtists builds an artist list from saved albums. It is the fallback
// when the account token has no user-follow-read scope.
func deriveArtists(albums []spotify.LibraryItem) []spotify.LibraryItem {
	seen := make(map[string]bool)
	var out []spotify.LibraryItem
	for _, al := range albums {
		name := strings.TrimSpace(al.Subtitle)
		if name == "" {
			continue
		}
		key := strings.ToLower(name)
		if seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, spotify.LibraryItem{
			ID:   namePrefix + name,
			Name: name,
			Kind: "artist",
		})
	}
	return out
}

func (s *Server) resolveArtistID(ctx context.Context, id string) string {
	if !strings.HasPrefix(id, namePrefix) {
		return id
	}
	name := strings.TrimPrefix(id, namePrefix)
	resolved, err := s.auth.SearchArtist(ctx, name)
	if err != nil {
		log.Printf("artist search %q: %v", name, err)
		return ""
	}
	return resolved
}

// AI-assisted code: retain the HTTP shape while using the saved metadata source.
func (s *Server) handleLibrary(w http.ResponseWriter, r *http.Request) {
	s.writeMetadata(w, r, "library", "", s.loadLibrary)
}

func (s *Server) handleArtistAlbums(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	s.writeMetadata(w, r, "artist-albums", id, func(ctx context.Context) (any, error) {
		resolved, err := s.resolveMetadataArtist(ctx, id)
		if err != nil {
			return nil, err
		}
		albums, err := s.source.ArtistAlbums(ctx, resolved)
		if err != nil {
			return nil, err
		}
		return map[string]any{"albums": toLibraryItemDTOs(uniqueLibraryItems(albums))}, nil
	})
}

func (s *Server) handleCollectionTracks(w http.ResponseWriter, r *http.Request) {
	kind, id := r.PathValue("kind"), r.PathValue("id")
	s.writeMetadata(w, r, "collection-"+kind, id, func(ctx context.Context) (any, error) {
		return s.loadCollection(ctx, kind, id)
	})
}

// End AI-assisted code.

func (s *Server) handleExtract(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Input string `json:"input"`
	}
	if err := readJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, "bad JSON: "+err.Error())
		return
	}
	coll, err := spotify.Extract(strings.TrimSpace(body.Input))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"id":     coll.ID,
		"kind":   coll.Kind,
		"name":   coll.Name,
		"artist": coll.Artist,
		"tracks": toTrackDTOs(coll.Tracks),
	})
}

func (s *Server) handleLookup(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Tracks []trackDTO `json:"tracks"`
	}
	if err := readJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, "bad JSON: "+err.Error())
		return
	}
	results := make([]map[string]any, 0, len(body.Tracks))
	for i, d := range body.Tracks {
		res := s.index.Lookup(d.toTrack())
		results = append(results, map[string]any{
			"index":  i,
			"id":     d.ID,
			"found":  res.Found,
			"path":   res.Path,
			"source": res.Source,
			"match":  res.Match,
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{"results": results})
}

func (s *Server) handleRescan(w http.ResponseWriter, r *http.Request) {
	if err := r.Context().Err(); err != nil {
		writeJSON(w, http.StatusRequestTimeout, map[string]any{"error": err.Error(), "complete": false})
		return
	}
	flight := s.startIndexScan()
	select {
	case <-flight.done:
		if flight.err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]any{"error": flight.err.Error(), "complete": false})
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"ok": true, "complete": true, "count": flight.count})
	case <-r.Context().Done():
		writeJSON(w, http.StatusRequestTimeout, map[string]any{"error": r.Context().Err().Error(), "complete": false})
	}
}

func (s *Server) handleStartDownloads(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Kind   string     `json:"kind"`
		ID     string     `json:"id"`
		Name   string     `json:"name"`
		Tracks []trackDTO `json:"tracks"`
	}
	if err := readJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, "bad JSON: "+err.Error())
		return
	}
	if len(body.Tracks) == 0 {
		writeError(w, http.StatusBadRequest, "no tracks")
		return
	}
	if body.Kind == "" {
		body.Kind = "playlist"
	}
	if body.Name == "" {
		body.Name = "Spotify"
	}
	tracks := make([]spotify.Track, 0, len(body.Tracks))
	for _, d := range body.Tracks {
		if strings.TrimSpace(d.Title) == "" {
			continue
		}
		tracks = append(tracks, d.toTrack())
	}
	identity, err := s.ensureIdentity(r.Context(), false)
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	jobs := s.manager.EnqueueForAccount(body.Kind, body.Name, tracks, identity.ID)
	batchID := ""
	if len(jobs) > 0 {
		batchID = jobs[0].BatchID
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"batch_id":   batchID,
		"account_id": identity.ID,
		"jobs":       jobs,
	})
}

func (s *Server) handleGetDownloads(w http.ResponseWriter, r *http.Request) {
	afterSeq, _ := strconv.ParseUint(r.URL.Query().Get("seq"), 10, 64)
	batchID := r.URL.Query().Get("batch")
	epoch := s.manager.Epoch()
	if requested := r.URL.Query().Get("epoch"); requested != "" && requested != epoch {
		afterSeq = 0
	}
	identity, err := s.ensureIdentity(r.Context(), false)
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	seq, jobs := s.manager.SnapshotForAccount(afterSeq, batchID, identity.ID)
	writeJSON(w, http.StatusOK, map[string]any{
		"seq":        seq,
		"epoch":      epoch,
		"account_id": identity.ID,
		"active":     s.manager.ActiveCountForAccount(identity.ID),
		"jobs":       jobs,
	})
}

func (s *Server) handleCancelDownloads(w http.ResponseWriter, r *http.Request) {
	var body struct {
		IDs     []string `json:"ids"`
		BatchID string   `json:"batch_id"`
	}
	if err := readJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, "bad JSON: "+err.Error())
		return
	}
	identity, err := s.ensureIdentity(r.Context(), false)
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	n := s.manager.CancelForAccount(body.IDs, body.BatchID, identity.ID)
	writeJSON(w, http.StatusOK, map[string]any{"canceled": n, "account_id": identity.ID})
}
