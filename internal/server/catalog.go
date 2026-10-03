// AI-assisted code: catalogue requests are separate from saved-library sync.
package server

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"spotseek-app/internal/spotify"
)

type catalogSource interface {
	Search(context.Context, spotify.SearchRequest) (spotify.SearchPage, error)
}
type cachedSearch struct {
	page spotify.SearchPage
	at   time.Time
	used time.Time
}
type catalogService struct {
	mu      sync.Mutex
	source  catalogSource
	pages   map[string]cachedSearch
	retryAt time.Time
}

func newCatalogService(source catalogSource) *catalogService {
	return &catalogService{source: source, pages: map[string]cachedSearch{}}
}
func copySearchPage(page spotify.SearchPage) spotify.SearchPage {
	page.Tracks = append([]spotify.Track{}, page.Tracks...)
	page.Items = append([]spotify.LibraryItem{}, page.Items...)
	return page
}
func (c *catalogService) search(ctx context.Context, account string, request spotify.SearchRequest, refresh bool) (spotify.SearchPage, bool, error) {
	request.Query = strings.TrimSpace(request.Query)
	if err := request.Validate(); err != nil {
		return spotify.SearchPage{}, false, err
	}
	key := metadataKey(account, "search", fmt.Sprintf("%s\x00%s\x00%d\x00%d", request.Kind, request.Query, request.Offset, request.Limit))
	now := time.Now()
	c.mu.Lock()
	entry, found := c.pages[key]
	if found && !refresh && now.Sub(entry.at) < 5*time.Minute {
		entry.used = now
		c.pages[key] = entry
		c.mu.Unlock()
		return copySearchPage(entry.page), true, nil
	}
	retry := c.retryAt
	c.mu.Unlock()
	if now.Before(retry) {
		return spotify.SearchPage{}, false, &spotify.APIError{Status: 429, RetryAfter: int(time.Until(retry).Seconds()) + 1}
	}
	page, err := c.source.Search(ctx, request)
	if err != nil {
		var api *spotify.APIError
		if errors.As(err, &api) && api.Status == 429 {
			c.mu.Lock()
			c.retryAt = time.Now().Add(time.Duration(api.RetryAfter) * time.Second)
			c.mu.Unlock()
		}
		return spotify.SearchPage{}, false, err
	}
	if err := ctx.Err(); err != nil {
		return spotify.SearchPage{}, false, err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.pages) >= 128 && !found {
		oldestKey := ""
		var oldest time.Time
		for key, entry := range c.pages {
			if oldestKey == "" || entry.used.Before(oldest) {
				oldestKey = key
				oldest = entry.used
			}
		}
		delete(c.pages, oldestKey)
	}
	c.pages[key] = cachedSearch{page: copySearchPage(page), at: now, used: now}
	return page, false, nil
}

func writeCatalogError(w http.ResponseWriter, err error, account string) {
	status, code, message := http.StatusBadGateway, "spotify_unavailable", "Cannot reach Spotify. Check the connection and try again. The saved list is kept."
	retry := 0
	var api *spotify.APIError
	switch {
	case errors.As(err, &api):
		status = api.Status
		message = api.Error()
		retry = api.RetryAfter
		switch status {
		case 400:
			code = "invalid_request"
		case 401:
			code = "login_required"
		case 403:
			code = "access_denied"
		case 404:
			code = "not_found"
		case 429:
			code = "rate_limited"
		}
	case errors.Is(err, context.DeadlineExceeded):
		message = "Spotify did not reply in time. Try again. The saved list is kept."
		code = "timeout"
		status = 504
	case strings.Contains(err.Error(), "not connected") || strings.Contains(err.Error(), "not logged"):
		status = 401
		code = "login_required"
		message = "Log in to Spotify in Spotseek, then try again."
	}
	if retry > 0 {
		w.Header().Set("Retry-After", strconv.Itoa(retry))
	}
	writeJSON(w, status, map[string]any{"error": message, "code": code, "retry_after_seconds": retry, "complete": false, "account_id": account})
}
func (s *Server) handleSearch(w http.ResponseWriter, r *http.Request) {
	params := r.URL.Query()
	offset := 0
	if params.Has("offset") {
		value, err := strconv.Atoi(params.Get("offset"))
		if err != nil {
			writeError(w, 400, "Search page is out of range.")
			return
		}
		offset = value
	}
	request := spotify.SearchRequest{Query: params.Get("q"), Kind: params.Get("type"), Offset: offset, Limit: 10}
	if err := request.Validate(); err != nil {
		writeError(w, 400, err.Error())
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 20*time.Second)
	defer cancel()
	version := s.source.SessionVersion()
	identity, err := s.ensureIdentity(ctx, false)
	if err != nil {
		writeCatalogError(w, err, "")
		return
	}
	if s.catalog == nil {
		writeError(w, 501, "This Spotseek build does not support catalogue search.")
		return
	}
	page, cached, err := s.catalog.search(ctx, identity.ID, request, params.Get("refresh") == "1")
	if err != nil {
		if !errors.Is(err, context.Canceled) {
			writeCatalogError(w, err, identity.ID)
		}
		return
	}
	if version != s.source.SessionVersion() {
		writeError(w, 409, "Spotify login changed. Search again.")
		return
	}
	writeJSON(w, 200, map[string]any{"account_id": identity.ID, "complete": true, "query": strings.TrimSpace(request.Query), "kind": request.Kind, "offset": request.Offset, "next_offset": page.NextOffset, "tracks": toTrackDTOs(page.Tracks), "items": toLibraryItemDTOs(page.Items), "cached": cached})
}

type DownloadedTrack struct {
	AccountID string    `json:"account_id"`
	Track     trackDTO  `json:"track"`
	Path      string    `json:"path"`
	UpdatedAt time.Time `json:"updated_at"`
}

func (m *Manager) rememberDownloadLocked(job *Job) {
	if job.Path == "" || (job.State != JobDone && job.State != JobAlready) {
		return
	}
	if m.downloads == nil {
		m.downloads = map[string]DownloadedTrack{}
	}
	key := accountTrackKey(job.AccountID, job.Track)
	asset := m.downloads[key]
	if !asset.UpdatedAt.After(job.UpdatedAt) {
		m.downloads[key] = DownloadedTrack{AccountID: job.AccountID, Track: toTrackDTO(job.Track), Path: job.Path, UpdatedAt: job.UpdatedAt}
	}
}
func (m *Manager) downloaded(account string) []DownloadedTrack {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := make([]DownloadedTrack, 0, len(m.downloads))
	for _, asset := range m.downloads {
		if asset.AccountID == account {
			out = append(out, asset)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].UpdatedAt.Equal(out[j].UpdatedAt) {
			return out[i].Track.ID < out[j].Track.ID
		}
		return out[i].UpdatedAt.After(out[j].UpdatedAt)
	})
	return out
}
func (s *Server) handleDownloaded(w http.ResponseWriter, r *http.Request) {
	identity, err := s.ensureIdentity(r.Context(), false)
	if err != nil {
		writeCatalogError(w, err, "")
		return
	}
	writeJSON(w, 200, map[string]any{"account_id": identity.ID, "complete": true, "downloads": s.manager.downloaded(identity.ID)})
}

// End AI-assisted code.
