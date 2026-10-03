// AI-assisted code: check complete metadata reads with a local HTTP server.
package spotify

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"sync"
	"testing"
	"time"
)

type rewriteTransport struct {
	base   http.RoundTripper
	target *url.URL
}

func (t rewriteTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	clone := request.Clone(request.Context())
	u := *clone.URL
	u.Scheme = t.target.Scheme
	u.Host = t.target.Host
	clone.URL = &u
	return t.base.RoundTrip(clone)
}
func metadataHTTP(t *testing.T, handler http.HandlerFunc) *Auth {
	t.Helper()
	server := httptest.NewServer(handler)
	previous := httpClient
	target, _ := url.Parse(server.URL)
	httpClient = &http.Client{Transport: rewriteTransport{base: server.Client().Transport, target: target}, Timeout: time.Second}
	t.Cleanup(func() { httpClient = previous; server.Close() })
	return &Auth{ClientID: "test-client", accessToken: "test-access", refreshToken: "test-refresh", expiresAt: time.Now().Add(time.Hour)}
}

func TestIdentityReturnsStableIDAndDisplayName(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, `{"id":"stable-user","display_name":"Shared display name"}`)
	})
	id, name, err := auth.Identity(context.Background())
	if err != nil || id != "stable-user" || name != "Shared display name" {
		t.Fatalf("identity %q %q: %v", id, name, err)
	}
}

func TestPlaylistPagingPreservesOrderAndRepeatedEntries(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("offset") == "2" {
			fmt.Fprint(w, `{"items":[{"item":{"id":"one","name":"One"}}],"next":null}`)
			return
		}
		fmt.Fprintf(w, `{"items":[{"item":{"id":"one","name":"One"}},{"item":{"id":"two","name":"Two"}}],"next":%q}`, apiBase+"/playlists/p/items?offset=2")
	})
	tracks, err := auth.PlaylistTracks(context.Background(), "p")
	if err != nil || len(tracks) != 3 || tracks[0].ID != "one" || tracks[1].ID != "two" || tracks[2].ID != "one" {
		t.Fatalf("tracks %v: %v", tracks, err)
	}
}

func TestFailedPlaylistPageDoesNotReturnPartialSuccess(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("offset") == "1" {
			http.Error(w, "page failed", http.StatusServiceUnavailable)
			return
		}
		fmt.Fprintf(w, `{"items":[{"item":{"id":"one","name":"One"}}],"next":%q}`, apiBase+"/playlists/p/items?offset=1")
	})
	tracks, err := auth.PlaylistTracks(context.Background(), "p")
	if err == nil || tracks != nil {
		t.Fatalf("partial success returned: %v, %v", tracks, err)
	}
}

func TestFailedAlbumInfoDoesNotReturnIncompleteMetadata(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "info failed", http.StatusServiceUnavailable)
	})
	tracks, err := auth.AlbumTracks(context.Background(), "a")
	if err == nil || tracks != nil {
		t.Fatalf("incomplete album returned: %v, %v", tracks, err)
	}
}

func TestArtistFallbackDoesNotIgnoreFailedAlbum(t *testing.T) {
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1/artists/ar/albums":
			fmt.Fprint(w, `{"items":[{"id":"a","name":"A"},{"id":"b","name":"B"}],"next":null}`)
		case "/v1/albums/a", "/v1/albums/b":
			fmt.Fprint(w, `{"name":"Album"}`)
		case "/v1/albums/a/tracks":
			fmt.Fprint(w, `{"items":[{"id":"one","name":"One"}],"next":null}`)
		case "/v1/albums/b/tracks":
			http.Error(w, "second album failed", http.StatusServiceUnavailable)
		default:
			http.NotFound(w, r)
		}
	})
	tracks, err := auth.ArtistAllTracks(context.Background(), "ar", 0)
	if err == nil || tracks != nil {
		t.Fatalf("failed album ignored: %v, %v", tracks, err)
	}
}

func TestTokenRefreshUsesContextAndDoesNotBlockSavedAccountKey(t *testing.T) {
	entered, release := make(chan struct{}), make(chan struct{})
	var once sync.Once
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		once.Do(func() { close(entered) })
		<-release
		http.Error(w, "late", http.StatusServiceUnavailable)
	})
	auth.expiresAt = time.Now().Add(-time.Minute)
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	done := make(chan error, 1)
	go func() { _, err := auth.ensureTokenContext(ctx); done <- err }()
	<-entered
	keyDone := make(chan string, 1)
	go func() { keyDone <- auth.CacheKey() }()
	select {
	case key := <-keyDone:
		if key == "" {
			t.Error("empty saved account key")
		}
	case <-time.After(50 * time.Millisecond):
		t.Error("saved account key blocked by token refresh")
	}
	select {
	case err := <-done:
		if err == nil {
			t.Error("canceled refresh succeeded")
		}
	case <-time.After(time.Second):
		t.Error("token refresh ignored context")
	}
	close(release)
}

func TestWaitingTokenRefreshCanBeCanceled(t *testing.T) {
	entered, release := make(chan struct{}), make(chan struct{})
	var once sync.Once
	auth := metadataHTTP(t, func(w http.ResponseWriter, r *http.Request) {
		once.Do(func() { close(entered) })
		<-release
		http.Error(w, "test refresh failed", http.StatusServiceUnavailable)
	})
	auth.expiresAt = time.Now().Add(-time.Minute)
	firstDone := make(chan error, 1)
	firstCtx, firstCancel := context.WithTimeout(context.Background(), time.Second)
	defer firstCancel()
	go func() { _, err := auth.ensureTokenContext(firstCtx); firstDone <- err }()
	<-entered
	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()
	start := time.Now()
	_, err := auth.ensureTokenContext(ctx)
	if err == nil || time.Since(start) > 500*time.Millisecond {
		t.Errorf("waiting refresh ignored cancellation: %v", err)
	}
	close(release)
	<-firstDone
}

// End AI-assisted code.
