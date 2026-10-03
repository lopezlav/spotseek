// AI-assisted code: serve saved Spotify metadata and refresh it outside the view.
package server

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"strings"
	"time"

	"spotseek-app/internal/spotify"
)

type metadataSource interface {
	CacheKey() string
	SessionVersion() uint64
	Identity(context.Context) (string, string, error)
	Playlists(context.Context) ([]spotify.LibraryItem, error)
	Albums(context.Context) ([]spotify.LibraryItem, error)
	Artists(context.Context) ([]spotify.LibraryItem, error)
	ArtistAlbums(context.Context, string) ([]spotify.LibraryItem, error)
	SearchArtist(context.Context, string) (string, error)
	PlaylistTracks(context.Context, string) ([]spotify.Track, error)
	AlbumTracks(context.Context, string) ([]spotify.Track, error)
	ArtistTopTracks(context.Context, string) ([]spotify.Track, error)
	ArtistAllTracks(context.Context, string, int) ([]spotify.Track, error)
	LikedTracks(context.Context) ([]spotify.Track, error)
	Track(context.Context, string) (spotify.Track, error)
}

func (s *Server) activateIdentity(session string, id accountIdentity) {
	s.mu.Lock()
	s.accountID, s.accountSession, s.profile = id.ID, session, id.Name
	s.profileAt = time.Now()
	s.mu.Unlock()
	s.manager.SetAccount(id.ID)
}

func (s *Server) savedIdentity() accountIdentity {
	session := s.source.CacheKey()
	if session == "" {
		return accountIdentity{}
	}
	s.mu.Lock()
	current := accountIdentity{ID: s.accountID, Name: s.profile}
	currentSession := s.accountSession
	s.mu.Unlock()
	if currentSession == session && current.ID != "" {
		return current
	}
	saved := s.metadata.identity(session)
	if saved.ID != "" {
		s.activateIdentity(session, saved)
	}
	return saved
}

func (s *Server) ensureIdentity(ctx context.Context, force bool) (accountIdentity, error) {
	if !force {
		if saved := s.savedIdentity(); saved.ID != "" {
			return saved, nil
		}
	}
	session := s.source.CacheKey()
	if session == "" {
		return accountIdentity{}, fmt.Errorf("not connected to Spotify")
	}
	version := s.source.SessionVersion()
	flight := s.metadata.fetch(s.ctx, metadataKey(session, "identity", ""), func(loadCtx context.Context) (any, error) {
		identityCtx, cancel := context.WithTimeout(loadCtx, 10*time.Second)
		defer cancel()
		id, name, err := s.source.Identity(identityCtx)
		if err != nil {
			return nil, err
		}
		if id == "" {
			return nil, fmt.Errorf("Spotify did not return an account ID")
		}
		identity := accountIdentity{ID: id, Name: name}
		if version != s.source.SessionVersion() {
			return nil, fmt.Errorf("Spotify login changed during the request")
		}
		currentSession := s.source.CacheKey()
		if currentSession == "" {
			return nil, fmt.Errorf("Spotify login changed during the request")
		}
		if err := s.metadata.saveIdentity(currentSession, identity); err != nil {
			return nil, err
		}
		return identity, nil
	})
	select {
	case <-flight.done:
		if flight.err != nil {
			return accountIdentity{}, flight.err
		}
		if version != s.source.SessionVersion() {
			return accountIdentity{}, fmt.Errorf("Spotify login changed during the request")
		}
		var identity accountIdentity
		if err := json.Unmarshal(flight.entry.Data, &identity); err != nil {
			return accountIdentity{}, err
		}
		s.activateIdentity(s.source.CacheKey(), identity)
		return identity, nil
	case <-ctx.Done():
		return accountIdentity{}, ctx.Err()
	}
}

func (s *Server) clearIdentity() {
	s.mu.Lock()
	s.accountID, s.accountSession, s.profile = "", "", ""
	s.profileAt = time.Time{}
	s.identityAttempt = time.Time{}
	s.mu.Unlock()
	s.manager.SetAccount("")
}

func (s *Server) writeMetadata(w http.ResponseWriter, r *http.Request, kind, id string, loader func(context.Context) (any, error)) {
	version := s.source.SessionVersion()
	identity, err := s.ensureIdentity(r.Context(), false)
	if err == nil && version != s.source.SessionVersion() {
		err = fmt.Errorf("Spotify login changed during the request")
	}
	if err != nil {
		writeCatalogError(w, err, "")
		return
	}
	force := r.URL.Query().Get("refresh") == "1"
	entry, cached, err := s.metadata.get(s.ctx, metadataKey(identity.ID, kind, id), force, func(ctx context.Context) (any, error) {
		if version != s.source.SessionVersion() {
			return nil, fmt.Errorf("Spotify login changed during the request")
		}
		data, err := loader(ctx)
		if err == nil && version != s.source.SessionVersion() {
			return nil, fmt.Errorf("Spotify login changed during the request")
		}
		return data, err
	}, r.Context())
	if err != nil {
		writeCatalogError(w, err, identity.ID)
		return
	}
	var response map[string]any
	if err := json.Unmarshal(entry.Data, &response); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]any{"error": "invalid saved metadata", "complete": false, "account_id": identity.ID})
		return
	}
	response["account_id"] = identity.ID
	response["profile"] = identity.Name
	response["revision"] = entry.Revision
	response["complete"] = true
	response["last_sync"] = entry.LastSync.Format(time.RFC3339Nano)
	response["cached"] = cached
	if kind == "library" {
		response["logged_in"] = s.source.CacheKey() != ""
	}
	writeJSON(w, http.StatusOK, response)
}

func uniqueLibraryItems(items []spotify.LibraryItem) []spotify.LibraryItem {
	out := make([]spotify.LibraryItem, 0, len(items))
	seen := map[string]bool{}
	for _, item := range items {
		key := item.Kind + "\x00" + item.ID
		if item.ID == "" || seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, item)
	}
	return out
}

func (s *Server) loadLibrary(ctx context.Context) (any, error) {
	playlists, err := s.source.Playlists(ctx)
	if err != nil {
		return nil, fmt.Errorf("playlists: %w", err)
	}
	albums, err := s.source.Albums(ctx)
	if err != nil {
		return nil, fmt.Errorf("albums: %w", err)
	}
	artists, err := s.source.Artists(ctx)
	if err != nil {
		return nil, fmt.Errorf("followed artists: %w", err)
	}
	return map[string]any{
		"playlists": toLibraryItemDTOs(uniqueLibraryItems(playlists)),
		"albums":    toLibraryItemDTOs(uniqueLibraryItems(albums)),
		"artists":   toLibraryItemDTOs(uniqueLibraryItems(artists)),
		"warnings":  []string{},
	}, nil
}

func (s *Server) resolveMetadataArtist(ctx context.Context, id string) (string, error) {
	if !strings.HasPrefix(id, namePrefix) {
		return id, nil
	}
	resolved, err := s.source.SearchArtist(ctx, strings.TrimPrefix(id, namePrefix))
	if err != nil {
		return "", err
	}
	if resolved == "" {
		return "", fmt.Errorf("artist not found")
	}
	return resolved, nil
}

func (s *Server) loadCollection(ctx context.Context, kind, id string) (any, error) {
	var tracks []spotify.Track
	var err error
	switch kind {
	case "playlist":
		tracks, err = s.source.PlaylistTracks(ctx, id)
	case "album":
		tracks, err = s.source.AlbumTracks(ctx, id)
	case "artist":
		var resolved string
		resolved, err = s.resolveMetadataArtist(ctx, id)
		if err == nil {
			tracks, err = s.source.ArtistTopTracks(ctx, resolved)
			if err != nil {
				tracks, err = s.source.ArtistAllTracks(ctx, resolved, 0)
			}
		}
	case "liked":
		tracks, err = s.source.LikedTracks(ctx)
	case "track":
		var track spotify.Track
		track, err = s.source.Track(ctx, id)
		if err == nil {
			tracks = []spotify.Track{track}
		}
	default:
		return nil, fmt.Errorf("unknown collection kind %q", kind)
	}
	if err != nil {
		return nil, err
	}
	return map[string]any{"kind": kind, "id": id, "tracks": toTrackDTOs(tracks)}, nil
}

func (s *Server) warmIdentity() {
	if s.savedIdentity().ID != "" || s.source.CacheKey() == "" {
		return
	}
	s.mu.Lock()
	if time.Since(s.identityAttempt) < 30*time.Second {
		s.mu.Unlock()
		return
	}
	s.identityAttempt = time.Now()
	s.mu.Unlock()
	s.backgroundWG.Add(1)
	go func() {
		defer s.backgroundWG.Done()
		if _, err := s.ensureIdentity(s.ctx, false); err != nil {
			log.Printf("Spotify account identity: %v", err)
		}
	}()
}

// End AI-assisted code.
