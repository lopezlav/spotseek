// AI-assisted code: cache bounds, account isolation, and durable local assets.
package server

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"spotseek-app/internal/spotify"
)

type searchFunc func(context.Context, spotify.SearchRequest) (spotify.SearchPage, error)

func (f searchFunc) Search(ctx context.Context, r spotify.SearchRequest) (spotify.SearchPage, error) {
	return f(ctx, r)
}
func TestCatalogCacheIsBoundedAndAccountScoped(t *testing.T) {
	calls := 0
	service := newCatalogService(searchFunc(func(ctx context.Context, r spotify.SearchRequest) (spotify.SearchPage, error) {
		calls++
		return spotify.SearchPage{Tracks: []spotify.Track{{ID: r.Query, Title: r.Query}}, NextOffset: -1}, nil
	}))
	request := spotify.SearchRequest{Query: "one", Kind: "track", Limit: 10}
	page, cached, err := service.search(context.Background(), "a", request, false)
	if err != nil || cached {
		t.Fatal("first request must load")
	}
	page.Tracks[0].Title = "changed"
	page, cached, err = service.search(context.Background(), "a", request, false)
	if err != nil || !cached || page.Tracks[0].Title != "one" || calls != 1 {
		t.Fatal("cache copy is wrong")
	}
	service.search(context.Background(), "b", request, false)
	if calls != 2 {
		t.Fatal("account shared cached page")
	}
	for i := 0; i < 150; i++ {
		request.Query = fmt.Sprint(i)
		service.search(context.Background(), "a", request, false)
	}
	if len(service.pages) != 128 {
		t.Fatal("search cache exceeds bound")
	}
}
func TestCatalogRateLimitStopsRepeatedSpotifyCalls(t *testing.T) {
	calls := 0
	service := newCatalogService(searchFunc(func(context.Context, spotify.SearchRequest) (spotify.SearchPage, error) {
		calls++
		return spotify.SearchPage{}, &spotify.APIError{Status: 429, RetryAfter: 60}
	}))
	request := spotify.SearchRequest{Query: "one", Kind: "track", Limit: 10}
	for i := 0; i < 3; i++ {
		_, _, err := service.search(context.Background(), "a", request, true)
		var api *spotify.APIError
		if !errors.As(err, &api) || api.Status != 429 {
			t.Fatalf("error: %v", err)
		}
	}
	if calls != 1 {
		t.Fatal("rate limit was ignored")
	}
}
func TestSearchRouteValidationAndGracefulError(t *testing.T) {
	s, _ := metadataTestServer(t, t.TempDir(), newFakeMetadataSource())
	calls := 0
	s.catalog = newCatalogService(searchFunc(func(context.Context, spotify.SearchRequest) (spotify.SearchPage, error) {
		calls++
		return spotify.SearchPage{}, &spotify.APIError{Status: 429, RetryAfter: 60}
	}))
	for _, path := range []string{"/api/v1/catalog/search?q=song&type=track&offset=no", "/api/v1/catalog/search?q=song&type=playlist"} {
		recorder := httptest.NewRecorder()
		s.Handler().ServeHTTP(recorder, httptest.NewRequest("GET", path, nil))
		if recorder.Code != 400 {
			t.Fatalf("status %d", recorder.Code)
		}
	}
	if calls != 0 {
		t.Fatal("invalid request fetched Spotify")
	}
	recorder := httptest.NewRecorder()
	s.Handler().ServeHTTP(recorder, httptest.NewRequest("GET", "/api/v1/catalog/search?q=song&type=track", nil))
	if recorder.Code != 429 || recorder.Header().Get("Retry-After") != "60" || !strings.Contains(recorder.Body.String(), "rate_limited") {
		t.Fatalf("bad error %d %s", recorder.Code, recorder.Body.String())
	}
}
func TestDownloadedAssetsSurviveJobPruningAndRestart(t *testing.T) {
	dir := t.TempDir()
	manager, cancel := testJobManager(t, dir)
	manager.mu.Lock()
	manager.putLocked(&Job{ID: "asset", AccountID: "a", Track: spotify.Track{ID: "song", Title: "Song"}, State: JobDone, Path: "/music/song.mp3"})
	manager.putLocked(&Job{ID: "other", AccountID: "b", Track: spotify.Track{ID: "song", Title: "Other"}, State: JobDone, Path: "/music/other.mp3"})
	for i := 0; i < 3100; i++ {
		manager.putLocked(&Job{ID: fmt.Sprint(i), AccountID: "a", Track: spotify.Track{ID: "failed"}, State: JobFailed})
	}
	manager.mu.Unlock()
	if err := manager.Flush(); err != nil {
		t.Fatal(err)
	}
	cancel()
	manager.Wait()
	restored, _ := testJobManager(t, dir)
	assets := restored.downloaded("a")
	if len(assets) != 1 || assets[0].Track.ID != "song" || assets[0].Path != "/music/song.mp3" {
		t.Fatalf("assets %+v", assets)
	}
	if len(restored.downloaded("b")) != 1 {
		t.Fatal("lost other account")
	}
	restored.mu.Lock()
	restored.putLocked(&Job{ID: "retry", AccountID: "a", Track: spotify.Track{ID: "song"}, State: JobFailed})
	restored.mu.Unlock()
	if len(restored.downloaded("a")) != 1 {
		t.Fatal("failed retry removed completed download")
	}
}
func TestDownloadedRouteUsesSavedIdentityWithoutSpotify(t *testing.T) {
	source := newFakeMetadataSource()
	s, _ := metadataTestServer(t, t.TempDir(), source)
	s.metadata.saveIdentity(source.CacheKey(), accountIdentity{ID: "test-account", Name: "Test"})
	s.manager.mu.Lock()
	s.manager.putLocked(&Job{ID: "done", AccountID: "test-account", State: JobDone, Path: "/music/song.mp3", Track: spotify.Track{ID: "one", Title: "One"}, UpdatedAt: time.Now()})
	s.manager.mu.Unlock()
	recorder := httptest.NewRecorder()
	s.Handler().ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/api/v1/catalog/downloaded", nil))
	var body struct {
		Complete  bool              `json:"complete"`
		Downloads []DownloadedTrack `json:"downloads"`
	}
	if err := json.Unmarshal(recorder.Body.Bytes(), &body); err != nil || !body.Complete || len(body.Downloads) != 1 {
		t.Fatalf("body %s: %v", recorder.Body.String(), err)
	}
}

func TestSearchFilesUseOneFolderAndKeepCollectionRules(t *testing.T) {
	if scopeFolder("First query", "search") != "Search" || scopeFolder("Other query", "search") != "Search" {
		t.Fatal("search created separate query folders")
	}
	if scopeFolder("Song", "track") != "Singles" || scopeFolder("My Album", "album") != "My Album" {
		t.Fatal("changed collection folder rules")
	}
}

// End AI-assisted code.
