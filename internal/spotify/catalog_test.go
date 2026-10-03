// AI-assisted code: catalogue paging, errors, and canceled HTTP reads.
package spotify

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestCatalogSearchTrackFiltersAndPaging(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/search" || r.URL.Query().Get("q") != `artist:"AC/DC" year:2020` || r.URL.Query().Get("limit") != "10" || r.URL.Query().Get("offset") != "10" {
			t.Errorf("bad search request %s", r.URL)
		}
		fmt.Fprint(w, `{"tracks":{"items":[{"id":"one","name":"Song","duration_ms":123000,"artists":[{"name":"Artist"}],"album":{"name":"Album"}}],"next":"https://api.spotify.com/v1/search?offset=20"}}`)
	})
	page, err := auth.Search(context.Background(), SearchRequest{Query: `artist:"AC/DC" year:2020`, Kind: "track", Offset: 10, Limit: 10})
	if err != nil || page.NextOffset != 20 || len(page.Tracks) != 1 || page.Tracks[0].DurationSec != 123 || page.Tracks[0].Album != "Album" {
		t.Fatalf("page %+v: %v", page, err)
	}
}
func TestCatalogSearchAlbumsAndArtists(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("type") == "album" {
			fmt.Fprint(w, `{"albums":{"items":[{"id":"album","name":"Album","artists":[{"name":"Artist"}],"total_tracks":12}],"next":null}}`)
		} else {
			fmt.Fprint(w, `{"artists":{"items":[{"id":"artist","name":"Artist"}],"next":null}}`)
		}
	})
	for _, kind := range []string{"album", "artist"} {
		page, err := auth.Search(context.Background(), SearchRequest{Query: "name", Kind: kind, Limit: 10})
		if err != nil || page.NextOffset != -1 || len(page.Items) != 1 || page.Items[0].Kind != kind {
			t.Fatalf("page %+v: %v", page, err)
		}
	}
}
func TestCatalogSearchRejectsBadRequestsAndIncompletePages(t *testing.T) {
	calls := 0
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) { calls++; fmt.Fprint(w, `{}`) })
	for _, request := range []SearchRequest{{Query: "", Kind: "track", Limit: 10}, {Query: "ok", Kind: "playlist", Limit: 10}, {Query: "ok", Kind: "track", Limit: 50}, {Query: "ok", Kind: "track", Limit: 10, Offset: 1001}} {
		if _, err := auth.Search(context.Background(), request); err == nil {
			t.Fatal("invalid search accepted")
		}
	}
	if calls != 0 {
		t.Fatal("invalid search sent to Spotify")
	}
	if _, err := auth.Search(context.Background(), SearchRequest{Query: "ok", Kind: "track", Limit: 10}); err == nil {
		t.Fatal("incomplete result accepted")
	}
}
func TestSpotifyAPIErrorHasSafeTextAndRetryTime(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Retry-After", "42")
		w.WriteHeader(429)
		fmt.Fprint(w, "private upstream body")
	})
	_, err := auth.Search(context.Background(), SearchRequest{Query: "ok", Kind: "track", Limit: 10})
	var api *APIError
	if !errors.As(err, &api) || api.RetryAfter != 42 || strings.Contains(err.Error(), "private") {
		t.Fatalf("error: %v", err)
	}
}
func TestPlaylistAccessErrorIsClear(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("limit") != "50" {
			t.Error("playlist page limit must be 50")
		}
		w.WriteHeader(403)
	})
	_, err := auth.PlaylistTracks(context.Background(), "blocked")
	if err == nil || !strings.Contains(err.Error(), "collaborator") {
		t.Fatalf("error: %v", err)
	}
}
func TestCatalogSearchCancelsSpotifyRequest(t *testing.T) {
	entered := make(chan struct{})
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) { close(entered); <-r.Context().Done() })
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { _, err := auth.Search(ctx, SearchRequest{Query: "ok", Kind: "track", Limit: 10}); done <- err }()
	<-entered
	cancel()
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("error: %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("search did not cancel")
	}
}

func TestCatalogSearchRefreshesRejectedTokenOnce(t *testing.T) {
	apiCalls, tokenCalls := 0, 0
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/token" {
			tokenCalls++
			fmt.Fprint(w, `{"access_token":"new-access","expires_in":3600,"token_type":"Bearer"}`)
			return
		}
		apiCalls++
		if r.Header.Get("Authorization") != "Bearer new-access" {
			w.WriteHeader(401)
			return
		}
		fmt.Fprint(w, `{"tracks":{"items":[],"next":null}}`)
	})
	_, err := auth.Search(context.Background(), SearchRequest{Query: "ok", Kind: "track", Limit: 10})
	if err != nil || apiCalls != 2 || tokenCalls != 1 {
		t.Fatalf("calls %d token %d: %v", apiCalls, tokenCalls, err)
	}
}
func TestCatalogSearchDoesNotLoopOnRejectedLogin(t *testing.T) {
	apiCalls := 0
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/token" {
			fmt.Fprint(w, `{"access_token":"new-access","expires_in":3600,"token_type":"Bearer"}`)
			return
		}
		apiCalls++
		w.WriteHeader(401)
	})
	_, err := auth.Search(context.Background(), SearchRequest{Query: "ok", Kind: "track", Limit: 10})
	var api *APIError
	if !errors.As(err, &api) || api.Status != 401 || apiCalls != 2 {
		t.Fatalf("calls %d: %v", apiCalls, err)
	}
}

// End AI-assisted code.
