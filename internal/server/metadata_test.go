// AI-assisted code: check local metadata persistence without Spotify or audio jobs.
package server

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"spotseek-app/internal/spotify"
)

type fakeMetadataSource struct {
	mu           sync.Mutex
	session      string
	account      string
	version      uint64
	trackCalls   int
	libraryCalls int
	albumError   error
	trackError   error
	entered      chan struct{}
	release      chan struct{}
}

func newFakeMetadataSource() *fakeMetadataSource {
	return &fakeMetadataSource{session: "test-session", account: "test-account"}
}
func (f *fakeMetadataSource) CacheKey() string { f.mu.Lock(); defer f.mu.Unlock(); return f.session }
func (f *fakeMetadataSource) SessionVersion() uint64 {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.version
}
func (f *fakeMetadataSource) Identity(context.Context) (string, string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.account, "Test name", nil
}
func (f *fakeMetadataSource) Playlists(context.Context) ([]spotify.LibraryItem, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.libraryCalls++
	return []spotify.LibraryItem{{ID: "p", Kind: "playlist", Name: "Saved"}, {ID: "p", Kind: "playlist", Name: "Duplicate"}}, nil
}
func (f *fakeMetadataSource) Albums(context.Context) ([]spotify.LibraryItem, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	return []spotify.LibraryItem{}, f.albumError
}
func (f *fakeMetadataSource) Artists(context.Context) ([]spotify.LibraryItem, error) {
	return []spotify.LibraryItem{}, nil
}
func (f *fakeMetadataSource) ArtistAlbums(context.Context, string) ([]spotify.LibraryItem, error) {
	return []spotify.LibraryItem{{ID: "a", Kind: "album", Name: "Album"}, {ID: "a", Kind: "album", Name: "Album"}}, nil
}
func (f *fakeMetadataSource) SearchArtist(context.Context, string) (string, error) {
	return "artist", nil
}
func (f *fakeMetadataSource) PlaylistTracks(ctx context.Context, id string) ([]spotify.Track, error) {
	f.mu.Lock()
	f.trackCalls++
	err, entered, release, account := f.trackError, f.entered, f.release, f.account
	f.mu.Unlock()
	if entered != nil {
		select {
		case entered <- struct{}{}:
		default:
		}
	}
	if release != nil {
		select {
		case <-release:
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
	if err != nil {
		return nil, err
	}
	return []spotify.Track{{ID: "one", Title: account + " One"}, {ID: "two", Title: "Two"}, {ID: "one", Title: account + " One"}}, nil
}
func (f *fakeMetadataSource) AlbumTracks(ctx context.Context, id string) ([]spotify.Track, error) {
	return f.PlaylistTracks(ctx, id)
}
func (f *fakeMetadataSource) ArtistTopTracks(ctx context.Context, id string) ([]spotify.Track, error) {
	return f.PlaylistTracks(ctx, id)
}
func (f *fakeMetadataSource) ArtistAllTracks(ctx context.Context, id string, _ int) ([]spotify.Track, error) {
	return f.PlaylistTracks(ctx, id)
}
func (f *fakeMetadataSource) LikedTracks(ctx context.Context) ([]spotify.Track, error) {
	return f.PlaylistTracks(ctx, "")
}
func (f *fakeMetadataSource) Track(context.Context, string) (spotify.Track, error) {
	return spotify.Track{ID: "one", Title: "One"}, nil
}

func metadataTestServer(t *testing.T, dir string, source *fakeMetadataSource) (*Server, *httptest.Server) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	settings := DefaultSettings()
	settings.OutputDir = dir
	settings.Concurrency = 1
	srv, err := New(ctx, settings, spotify.NewAuth(""))
	if err != nil {
		t.Fatal(err)
	}
	srv.source = source
	ts := httptest.NewServer(srv.Handler())
	t.Cleanup(func() { ts.Close(); cancel(); srv.Wait() })
	return srv, ts
}

func TestMetadataPersistsOrderAndAccountOnRestart(t *testing.T) {
	dir := t.TempDir()
	source := newFakeMetadataSource()
	_, ts := metadataTestServer(t, dir, source)
	resp, first := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	if resp.StatusCode != http.StatusOK || first["complete"] != true || first["account_id"] != "test-account" || first["cached"] != false {
		t.Fatalf("first response: %v", first)
	}
	rows := first["tracks"].([]any)
	if len(rows) != 3 || rows[0].(map[string]any)["id"] != rows[2].(map[string]any)["id"] {
		t.Fatalf("playlist order or repeated item lost: %v", rows)
	}
	restarted := newFakeMetadataSource()
	restarted.trackError = errors.New("offline")
	_, ts2 := metadataTestServer(t, dir, restarted)
	_, saved := doJSON(t, ts2, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	if saved["cached"] != true || saved["revision"] != first["revision"] || saved["last_sync"] != first["last_sync"] {
		t.Fatalf("saved response: %v", saved)
	}
	restarted.mu.Lock()
	calls := restarted.trackCalls
	restarted.mu.Unlock()
	if calls != 0 {
		t.Fatalf("cached read fetched %d times", calls)
	}
}

func TestMetadataFailedRefreshKeepsLastCompleteData(t *testing.T) {
	source := newFakeMetadataSource()
	_, ts := metadataTestServer(t, t.TempDir(), source)
	_, before := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	source.mu.Lock()
	source.trackError = errors.New("page two failed")
	source.mu.Unlock()
	resp, failed := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks?refresh=1", nil)
	if resp.StatusCode != http.StatusBadGateway || failed["complete"] != false {
		t.Fatalf("failed refresh: %v", failed)
	}
	_, after := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	if after["revision"] != before["revision"] || after["last_sync"] != before["last_sync"] || after["complete"] != true {
		t.Fatalf("good data replaced: %v", after)
	}
}

func TestMetadataLibraryRejectsPartialCategoriesAndDeduplicatesItems(t *testing.T) {
	source := newFakeMetadataSource()
	source.albumError = errors.New("album page failed")
	srv, ts := metadataTestServer(t, t.TempDir(), source)
	resp, failed := doJSON(t, ts, "GET", "/api/v1/library", nil)
	if resp.StatusCode != http.StatusBadGateway || failed["complete"] != false {
		t.Fatalf("partial library marked complete: %v", failed)
	}
	srv.metadata.mu.Lock()
	_, saved := srv.metadata.disk.Entries[metadataKey("test-account", "library", "")]
	srv.metadata.mu.Unlock()
	if saved {
		t.Fatal("partial library saved")
	}
	source.mu.Lock()
	source.albumError = nil
	source.mu.Unlock()
	_, good := doJSON(t, ts, "GET", "/api/v1/library", nil)
	if good["complete"] != true || len(good["playlists"].([]any)) != 1 {
		t.Fatalf("complete library: %v", good)
	}
	_, albums := doJSON(t, ts, "GET", "/api/v1/artists/artist/albums", nil)
	if len(albums["albums"].([]any)) != 1 {
		t.Fatalf("duplicate artist albums: %v", albums)
	}
}

func TestMetadataAccountSwitchUsesSeparateSavedCollections(t *testing.T) {
	source := newFakeMetadataSource()
	_, ts := metadataTestServer(t, t.TempDir(), source)
	_, first := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	source.mu.Lock()
	source.session = "second-session"
	source.account = "second-account"
	source.version++
	source.mu.Unlock()
	_, second := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	if second["account_id"] != "second-account" || second["cached"] != false || second["revision"] == first["revision"] {
		t.Fatalf("account data mixed: %v", second)
	}
	source.mu.Lock()
	source.session = "test-session"
	source.account = "test-account"
	source.version++
	source.mu.Unlock()
	_, restored := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	if restored["account_id"] != "test-account" || restored["cached"] != true || restored["revision"] != first["revision"] {
		t.Fatalf("first account not restored: %v", restored)
	}
}

func TestMetadataRefreshSharesOneReadAndCachedViewDoesNotWait(t *testing.T) {
	source := newFakeMetadataSource()
	srv, ts := metadataTestServer(t, t.TempDir(), source)
	_, before := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	source.mu.Lock()
	source.entered = make(chan struct{}, 1)
	source.release = make(chan struct{})
	entered, release := source.entered, source.release
	source.mu.Unlock()
	key := metadataKey("test-account", "collection-playlist", "p")
	load := func(ctx context.Context) (any, error) { return srv.loadCollection(ctx, "playlist", "p") }
	flight := srv.metadata.fetch(srv.ctx, key, load)
	<-entered
	for i := 0; i < 20; i++ {
		if other := srv.metadata.fetch(srv.ctx, key, load); other != flight {
			t.Fatal("refresh was not shared")
		}
	}
	start := time.Now()
	_, cached := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	if time.Since(start) > time.Second || cached["revision"] != before["revision"] {
		t.Fatalf("cached view waited: %v", cached)
	}
	close(release)
	<-flight.done
	if flight.err != nil {
		t.Fatal(flight.err)
	}
	source.mu.Lock()
	calls := source.trackCalls
	source.mu.Unlock()
	if calls != 2 {
		t.Fatalf("want initial read plus shared refresh, got %d", calls)
	}
}

func TestMetadataLoginChangeDuringRefreshCannotReplaceSavedData(t *testing.T) {
	source := newFakeMetadataSource()
	srv, ts := metadataTestServer(t, t.TempDir(), source)
	_, before := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	source.mu.Lock()
	source.entered = make(chan struct{}, 1)
	source.release = make(chan struct{})
	entered, release := source.entered, source.release
	source.mu.Unlock()
	recorder := httptest.NewRecorder()
	request := httptest.NewRequest("GET", "/api/v1/collections/playlist/p/tracks?refresh=1", nil)
	done := make(chan struct{})
	go func() { srv.Handler().ServeHTTP(recorder, request); close(done) }()
	<-entered
	source.mu.Lock()
	source.session = "new-session"
	source.account = "new-account"
	source.version++
	source.mu.Unlock()
	close(release)
	<-done
	if recorder.Code != http.StatusBadGateway {
		t.Fatalf("account switch reply status %d", recorder.Code)
	}
	srv.metadata.mu.Lock()
	old := srv.metadata.disk.Entries[metadataKey("test-account", "collection-playlist", "p")]
	srv.metadata.mu.Unlock()
	if old.Revision != before["revision"] {
		t.Fatal("account switch replaced old metadata")
	}
}

func TestMetadataStaleFailureReturnsSavedDataAndBacksOff(t *testing.T) {
	source := newFakeMetadataSource()
	srv, ts := metadataTestServer(t, t.TempDir(), source)
	_, before := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	key := metadataKey("test-account", "collection-playlist", "p")
	srv.metadata.mu.Lock()
	entry := srv.metadata.disk.Entries[key]
	entry.LastSync = time.Now().Add(-2 * metadataMaxAge)
	srv.metadata.disk.Entries[key] = entry
	srv.metadata.attempted[key] = entry.LastSync
	srv.metadata.mu.Unlock()
	source.mu.Lock()
	source.trackError = errors.New("offline")
	source.entered = make(chan struct{}, 1)
	entered := source.entered
	source.mu.Unlock()
	_, saved := doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	if saved["cached"] != true || saved["revision"] != before["revision"] {
		t.Fatalf("stale response: %v", saved)
	}
	select {
	case <-entered:
	case <-time.After(time.Second):
		t.Fatal("no background refresh")
	}
	for i := 0; i < 5; i++ {
		doJSON(t, ts, "GET", "/api/v1/collections/playlist/p/tracks", nil)
	}
	source.mu.Lock()
	calls := source.trackCalls
	source.mu.Unlock()
	if calls != 2 {
		t.Fatalf("failed background refresh repeated %d times", calls)
	}
}

func TestMetadataCacheRejectsUnsupportedDiskVersion(t *testing.T) {
	cache, err := newMetadataCache(filepath.Join(t.TempDir(), "metadata.json"))
	if err != nil {
		t.Fatal(err)
	}
	cache.mu.Lock()
	cache.disk.Version = 999
	err = cache.saveLocked()
	cache.mu.Unlock()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := newMetadataCache(cache.path); err == nil {
		t.Fatal("unsupported cache accepted")
	}
}

func TestIndexRescanReplyWaitsUntilLocalFileCanBeFound(t *testing.T) {
	source := newFakeMetadataSource()
	dir := t.TempDir()
	srv, ts := metadataTestServer(t, dir, source)
	<-srv.startIndexScan().done
	path := filepath.Join(dir, "Artist - Fresh Song.mp3")
	if err := os.WriteFile(path, []byte("local test file"), 0600); err != nil {
		t.Fatal(err)
	}
	response, body := doJSON(t, ts, "POST", "/api/v1/index/rescan", map[string]any{})
	if response.StatusCode != http.StatusOK || body["complete"] != true {
		t.Fatalf("scan reply: %v", body)
	}
	_, lookup := doJSON(t, ts, "POST", "/api/v1/index/lookup", map[string]any{"tracks": []map[string]any{{"artist": "Artist", "title": "Fresh Song"}}})
	rows := lookup["results"].([]any)
	if len(rows) != 1 || rows[0].(map[string]any)["path"] != path {
		t.Fatalf("scan replied before local file was indexed: %v", lookup)
	}
}

func TestIndexRescanCanceledRequestReportsIncomplete(t *testing.T) {
	srv, _ := metadataTestServer(t, t.TempDir(), newFakeMetadataSource())
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	request := httptest.NewRequest("POST", "/api/v1/index/rescan", nil).WithContext(ctx)
	reply := httptest.NewRecorder()
	srv.Handler().ServeHTTP(reply, request)
	if reply.Code != http.StatusRequestTimeout {
		t.Fatalf("canceled scan status %d", reply.Code)
	}
}

// End AI-assisted code.
