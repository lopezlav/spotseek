// AI-generated code begins.
package libraryindex

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"spotseek-app/internal/spotify"
)

func TestIndexedLookupMatchingRules(t *testing.T) {
	for _, tc := range []struct {
		name                          string
		localArtist, localTitle       string
		remoteArtist, remoteTitle     string
		localDuration, remoteDuration int
		want                          bool
	}{
		{"local version", "Artist", "Long Song Extended Mix", "Artist", "Long Song", 200, 200, true},
		{"remote version", "Artist", "Long Song", "Artist", "Long Song Extended Mix", 200, 200, true},
		{"local artist version", "Artist Extra", "Long Song", "Artist", "Long Song", 200, 200, true},
		{"remote artist version", "Artist", "Long Song", "Artist Extra", "Long Song", 200, 200, true},
		{"unknown local artist", "", "Long Song", "Artist", "Long Song", 0, 200, true},
		{"unknown remote artist", "Artist", "Long Song", "", "Long Song", 200, 0, true},
		{"different artist", "Other", "Long Song", "Artist", "Long Song", 200, 200, false},
		{"wrong duration", "Artist", "Long Song Extended Mix", "Artist", "Long Song", 300, 200, false},
		{"short title", "Artist", "Hi Extended", "Artist", "Hi", 200, 200, false},
		{"accent", "Beyoncé", "Long Song Extended Mix", "Beyonce", "Long Song", 200, 200, true},
		{"unicode bytes", "Artist", "中文 Extended", "Artist", "中文", 200, 200, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			path := filepath.Join(dir, "audio.mp3")
			if err := os.WriteFile(path, []byte("audio"), 0o600); err != nil {
				t.Fatal(err)
			}
			ix := New(filepath.Join(dir, "index.json"), dir)
			if err := ix.Add(Entry{Artist: tc.localArtist, Title: tc.localTitle, DurationSec: tc.localDuration, Path: path}); err != nil {
				t.Fatal(err)
			}
			got := ix.Lookup(spotify.Track{Artist: tc.remoteArtist, Title: tc.remoteTitle, DurationSec: tc.remoteDuration})
			if got.Found != tc.want {
				t.Fatalf("got %+v, want found %v", got, tc.want)
			}
		})
	}
}

func TestLookupCacheInvalidationAndMissingFile(t *testing.T) {
	dir := t.TempDir()
	ix := New(filepath.Join(dir, ".spotseek", "index.json"), dir)
	track := spotify.Track{ID: "song", Artist: "Artist", Title: "Long Song"}
	if ix.Lookup(track).Found {
		t.Fatal("empty index matched")
	}
	path := filepath.Join(dir, "Artist - Long Song.mp3")
	if err := os.WriteFile(path, []byte("audio"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := ix.Add(Entry{SpotifyID: track.ID, Artist: track.Artist, Title: track.Title, Path: path}); err != nil {
		t.Fatal(err)
	}
	if !ix.Lookup(track).Found {
		t.Fatal("Add did not clear negative memo")
	}
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	if ix.Lookup(track).Found {
		t.Fatal("positive memo returned deleted file")
	}
	if err := os.WriteFile(path, []byte("audio"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := ix.Scan(nil); err != nil {
		t.Fatal(err)
	}
	if !ix.Lookup(track).Found {
		t.Fatal("Scan did not clear negative memo")
	}
	loaded := New(ix.path, dir)
	if loaded.Lookup(track).Found {
		t.Fatal("empty index matched before Load")
	}
	if err := loaded.Load(); err != nil {
		t.Fatal(err)
	}
	if !loaded.Lookup(track).Found {
		t.Fatal("Load did not clear negative memo")
	}
}

func TestRootChangeSavesNewFolder(t *testing.T) {
	oldRoot, newRoot := t.TempDir(), t.TempDir()
	ix := New(filepath.Join(oldRoot, ".spotseek", "library-index.json"), oldRoot)
	ix.SetRoot(newRoot)
	if err := ix.Save(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(newRoot, ".spotseek", "library-index.json")); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(oldRoot, ".spotseek", "library-index.json")); !os.IsNotExist(err) {
		t.Fatal("wrote old index location")
	}
}

func TestIndexLoadKeepsConfiguredRoot(t *testing.T) {
	oldRoot, configuredRoot := t.TempDir(), t.TempDir()
	path := filepath.Join(t.TempDir(), "index.json")
	original := New(path, oldRoot)
	if err := original.Save(); err != nil {
		t.Fatal(err)
	}
	loaded := New(path, configuredRoot)
	if err := loaded.Load(); err != nil {
		t.Fatal(err)
	}
	if loaded.Root() != configuredRoot {
		t.Fatal("saved root replaced the configured music folder")
	}
}

func BenchmarkLookup(b *testing.B) {
	for _, size := range []int{1000, 10000} {
		for _, mode := range []string{"indexed-positive", "indexed-negative", "indexed-cold-positive", "indexed-cold-negative", "legacy-positive", "legacy-negative"} {
			b.Run(fmt.Sprintf("%s/%d", mode, size), func(b *testing.B) {
				dir := b.TempDir()
				ix := New(filepath.Join(dir, "index.json"), dir)
				for i := 0; i < size; i++ {
					path := filepath.Join(dir, fmt.Sprintf("%d.mp3", i))
					if err := os.WriteFile(path, []byte("audio"), 0o600); err != nil {
						b.Fatal(err)
					}
					ix.putLocked(Entry{Artist: fmt.Sprintf("Artist %d", i), Title: fmt.Sprintf("Song Number %d Extended Mix", i), Path: path})
				}
				ix.rebuildMatchesLocked()
				positive := mode == "indexed-positive" || mode == "indexed-cold-positive" || mode == "legacy-positive"
				track := spotify.Track{Artist: fmt.Sprintf("Artist %d", size-1), Title: fmt.Sprintf("Song Number %d", size-1)}
				if !positive {
					track = spotify.Track{Artist: "Unmatched Artist", Title: "A Completely Absent Title"}
				}
				b.ReportAllocs()
				b.ResetTimer()
				for i := 0; i < b.N; i++ {
					if mode == "indexed-cold-positive" || mode == "indexed-cold-negative" {
						ix.mu.Lock()
						clear(ix.memo)
						ix.mu.Unlock()
					}
					var got LookupResult
					if mode == "legacy-positive" || mode == "legacy-negative" {
						got = legacyLookup(ix, track)
					} else {
						got = ix.Lookup(track)
					}
					if got.Found != positive {
						b.Fatalf("wrong result %+v", got)
					}
				}
			})
		}
	}
}

func legacyLookup(ix *Index, t spotify.Track) LookupResult {
	ix.mu.RLock()
	defer ix.mu.RUnlock()
	if e, ok := ix.entries[idKey(t.ID)]; t.ID != "" && ok && validFile(e.Path) {
		return LookupResult{Found: true, Path: e.Path, Match: "id"}
	}
	if e, ok := ix.entries[ttKey(t.Artist, t.Title)]; ok && validFile(e.Path) {
		return LookupResult{Found: true, Path: e.Path, Match: "artist-title"}
	}
	wantTitle, wantArtist := Normalize(t.Title), Normalize(t.Artist)
	seen := make(map[string]bool)
	for _, e := range ix.entries {
		if seen[e.Path] || !validFile(e.Path) {
			continue
		}
		seen[e.Path] = true
		if TitleMatches(wantTitle, Normalize(e.Title)) && ArtistMatches(wantArtist, Normalize(e.Artist)) && DurationClose(t.DurationSec, e.DurationSec) {
			return LookupResult{Found: true, Path: e.Path, Match: "title"}
		}
	}
	return LookupResult{}
}

// AI-generated code ends.
