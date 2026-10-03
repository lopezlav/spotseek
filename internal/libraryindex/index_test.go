package libraryindex

import (
	"os"
	"path/filepath"
	"testing"

	"spotseek-app/internal/spotify"
)

func TestIndexAddLookupSaveLoad(t *testing.T) {
	dir := t.TempDir()
	audio := filepath.Join(dir, "Chill", "Daft Punk - Around the World.mp3")
	if err := os.MkdirAll(filepath.Dir(audio), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(audio, []byte("fake"), 0o644); err != nil {
		t.Fatal(err)
	}

	path := filepath.Join(dir, ".spotseek", "library-index.json")
	ix := New(path, dir)
	if err := ix.Add(Entry{
		SpotifyID: "abc123",
		Artist:    "Daft Punk",
		Title:     "Around the World",
		Path:      audio,
		Source:    "soulseek",
	}); err != nil {
		t.Fatal(err)
	}

	// Match by Spotify id.
	res := ix.Lookup(spotify.Track{ID: "abc123"})
	if !res.Found || res.Path != audio || res.Match != "id" {
		t.Fatalf("id lookup failed: %+v", res)
	}

	// Match by artist and title without an id.
	res = ix.Lookup(spotify.Track{Artist: "Daft Punk", Title: "Around the World"})
	if !res.Found || res.Path != audio {
		t.Fatalf("artist-title lookup failed: %+v", res)
	}

	// Match by title when the artist differs slightly.
	res = ix.Lookup(spotify.Track{Artist: "Daft Punk feat. X", Title: "Around the World"})
	if !res.Found {
		t.Fatalf("title lookup failed: %+v", res)
	}

	// Reload from disk.
	ix2 := New(path, dir)
	if err := ix2.Load(); err != nil {
		t.Fatal(err)
	}
	if res := ix2.Lookup(spotify.Track{ID: "abc123"}); !res.Found {
		t.Fatalf("lookup after load failed: %+v", res)
	}
}

func TestIndexScan(t *testing.T) {
	dir := t.TempDir()
	track := filepath.Join(dir, "My Playlist", "Artist One - First Song.mp3")
	if err := os.MkdirAll(filepath.Dir(track), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(track, []byte("fake"), 0o644); err != nil {
		t.Fatal(err)
	}

	ix := New(filepath.Join(dir, ".spotseek", "library-index.json"), dir)
	n, err := ix.Scan(nil)
	if err != nil {
		t.Fatal(err)
	}
	if n != 1 {
		t.Fatalf("scan counted %d files, want 1", n)
	}
	res := ix.Lookup(spotify.Track{Artist: "Artist One", Title: "First Song"})
	if !res.Found || res.Path != track {
		t.Fatalf("scan lookup failed: %+v", res)
	}
}
