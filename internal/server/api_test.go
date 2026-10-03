package server

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"spotseek-app/internal/libraryindex"
	"spotseek-app/internal/spotify"
)

func newTestServer(t *testing.T) (*Server, string) {
	t.Helper()
	// Keep the real server.json untouched.
	t.Setenv("HOME", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	dir := t.TempDir()
	settings := DefaultSettings()
	settings.OutputDir = dir
	settings.Concurrency = 1
	ctx, cancel := context.WithCancel(context.Background())

	srv, err := New(ctx, settings, spotify.NewAuth(""))
	if err != nil {
		t.Fatal(err)
	}
	srv.source = newFakeMetadataSource()
	t.Cleanup(func() { cancel(); srv.Wait() })
	return srv, dir
}

func doJSON(t *testing.T, ts *httptest.Server, method, path string, body any) (*http.Response, map[string]any) {
	t.Helper()
	var reader *bytes.Reader
	if body != nil {
		raw, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		reader = bytes.NewReader(raw)
	} else {
		reader = bytes.NewReader(nil)
	}
	req, err := http.NewRequest(method, ts.URL+path, reader)
	if err != nil {
		t.Fatal(err)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var out map[string]any
	_ = json.NewDecoder(resp.Body).Decode(&out)
	return resp, out
}

func TestSoulseekCooldown(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	srv, _ := newTestServer(t)
	srv.manager.mu.Lock()
	srv.manager.jobs["x"] = &Job{ID: "x"}
	srv.manager.mu.Unlock()

	srv.manager.handleLog("x",
		"[error] [soulseek] Failed to ensure Soulseek connection and login: timeout")

	srv.manager.mu.Lock()
	downUntil := srv.manager.soulseekDownUntil
	srv.manager.mu.Unlock()
	if !time.Now().Before(downUntil) {
		t.Fatal("cooldown was not set after a Soulseek login failure")
	}
}

func TestSettingsRoundTrip(t *testing.T) {
	srv, dir := newTestServer(t)
	ts := httptest.NewServer(srv.Handler())
	defer ts.Close()

	resp, body := doJSON(t, ts, "PUT", "/api/v1/settings", map[string]any{
		"output_dir":  filepath.Join(dir, "music"),
		"bitrate":     "256",
		"fallback":    false,
		"concurrency": 3,
	})
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("PUT settings status %d: %v", resp.StatusCode, body)
	}
	if body["bitrate"] != "256" || body["concurrency"].(float64) != 3 {
		t.Fatalf("settings not applied: %v", body)
	}

	_, body = doJSON(t, ts, "GET", "/api/v1/settings", nil)
	if body["bitrate"] != "256" {
		t.Fatalf("GET settings wrong: %v", body)
	}
}

func TestLookupAndAlreadyDownloaded(t *testing.T) {
	srv, dir := newTestServer(t)
	ts := httptest.NewServer(srv.Handler())
	defer ts.Close()

	audio := filepath.Join(dir, "Chill", "Daft Punk - Around the World.mp3")
	if err := os.MkdirAll(filepath.Dir(audio), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(audio, []byte("fake"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := srv.index.Add(libraryindex.Entry{
		SpotifyID: "abc123",
		Artist:    "Daft Punk",
		Title:     "Around the World",
		Path:      audio,
		Source:    "soulseek",
	}); err != nil {
		t.Fatal(err)
	}

	_, body := doJSON(t, ts, "POST", "/api/v1/index/lookup", map[string]any{
		"tracks": []map[string]any{
			{"id": "abc123", "artist": "Daft Punk", "title": "Around the World"},
			{"id": "zzz", "artist": "Nobody", "title": "Nothing"},
		},
	})
	results := body["results"].([]any)
	first := results[0].(map[string]any)
	second := results[1].(map[string]any)
	if first["found"] != true || first["path"] != audio {
		t.Fatalf("first lookup wrong: %v", first)
	}
	if second["found"] != false {
		t.Fatalf("second lookup wrong: %v", second)
	}

	// Starting a download for a track that is already there must return an
	// "already" job without touching sockseek.
	resp, body := doJSON(t, ts, "POST", "/api/v1/downloads", map[string]any{
		"kind": "playlist",
		"name": "Chill",
		"tracks": []map[string]any{
			{"id": "abc123", "artist": "Daft Punk", "title": "Around the World"},
		},
	})
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("start download status %d: %v", resp.StatusCode, body)
	}
	jobs := body["jobs"].([]any)
	if len(jobs) != 1 {
		t.Fatalf("want 1 job, got %v", jobs)
	}
	job := jobs[0].(map[string]any)
	if job["state"] != "already" || job["path"] != audio {
		t.Fatalf("job wrong: %v", job)
	}

	// The change must be visible in the download snapshot.
	_, body = doJSON(t, ts, "GET", "/api/v1/downloads?seq=0", nil)
	if body["seq"].(float64) < 1 {
		t.Fatalf("snapshot seq wrong: %v", body)
	}
	if len(body["jobs"].([]any)) != 1 {
		t.Fatalf("snapshot jobs wrong: %v", body)
	}
}
