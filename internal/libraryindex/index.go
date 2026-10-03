package libraryindex

import (
	"encoding/csv"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"spotseek-app/internal/spotify"
)

// Entry is one known local audio file.
type Entry struct {
	SpotifyID   string    `json:"spotify_id,omitempty"`
	Artist      string    `json:"artist,omitempty"`
	Title       string    `json:"title,omitempty"`
	Album       string    `json:"album,omitempty"`
	DurationSec int       `json:"duration,omitempty"`
	Path        string    `json:"path"`
	Source      string    `json:"source,omitempty"` // soulseek | youtube | scan
	AddedAt     time.Time `json:"added_at,omitempty"`
}

// LookupResult tells whether a Spotify track is already downloaded.
type LookupResult struct {
	Found  bool
	Path   string
	Source string
	Match  string // id | artist-title | title
}

type indexFile struct {
	Version int     `json:"version"`
	Root    string  `json:"root"`
	Entries []Entry `json:"entries"`
}

// Index maps Spotify tracks to local files. It is safe for concurrent use.
type Index struct {
	mu           sync.RWMutex
	saveMu       sync.Mutex
	path         string
	root         string
	entries      map[string]Entry
	generation   uint64
	byTitle      matchBuckets
	byArtist     matchBuckets
	matchEntries map[string]normalizedEntry
	memo         map[lookupKey]lookupMemo
}

// New creates an empty index. path is the JSON file, root the music folder.
func New(path, root string) *Index {
	return &Index{
		path:    path,
		root:    root,
		entries: make(map[string]Entry),
	}
}

func idKey(id string) string { return "id:" + id }
func ttKey(artist, title string) string {
	return "tt:" + TrackKey(artist, title)
}

// Load reads the JSON file. A missing file is not an error.
func (ix *Index) Load() error {
	raw, err := os.ReadFile(ix.path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	var f indexFile
	if err := json.Unmarshal(raw, &f); err != nil {
		return fmt.Errorf("bad index file %s: %w", ix.path, err)
	}
	ix.mu.Lock()
	defer ix.mu.Unlock()
	if ix.root == "" && f.Root != "" {
		ix.root = f.Root
	}
	ix.entries = make(map[string]Entry, len(f.Entries))
	for _, e := range f.Entries {
		ix.putLocked(e)
	}
	ix.rebuildMatchesLocked()
	return nil
}

// Save writes the index to disk atomically.
func (ix *Index) Save() error {
	ix.saveMu.Lock()
	defer ix.saveMu.Unlock()

	ix.mu.RLock()
	path := ix.path
	f := indexFile{Version: 1, Root: ix.root, Entries: make([]Entry, 0, len(ix.entries))}
	seen := make(map[string]bool, len(ix.entries))
	for _, e := range ix.entries {
		if seen[e.Path] {
			continue
		}
		seen[e.Path] = true
		f.Entries = append(f.Entries, e)
	}
	ix.mu.RUnlock()

	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	raw, err := json.MarshalIndent(f, "", "  ")
	if err != nil {
		return err
	}
	if err := os.WriteFile(tmp, raw, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func (ix *Index) putLocked(e Entry) {
	if e.Path == "" {
		return
	}
	e.Path = filepath.Clean(e.Path)
	if e.AddedAt.IsZero() {
		e.AddedAt = time.Now()
	}
	if e.SpotifyID != "" {
		ix.entries[idKey(e.SpotifyID)] = e
	}
	if e.Artist != "" || e.Title != "" {
		ix.entries[ttKey(e.Artist, e.Title)] = e
	}
}

// Add stores an entry and saves the file.
func (ix *Index) Add(e Entry) error {
	ix.mu.Lock()
	ix.putLocked(e)
	ix.rebuildMatchesLocked()
	ix.mu.Unlock()
	return ix.Save()
}

// Root returns the music folder.
func (ix *Index) Root() string {
	ix.mu.RLock()
	defer ix.mu.RUnlock()
	return ix.root
}

// SetRoot changes the music folder.
func (ix *Index) SetRoot(root string) {
	ix.mu.Lock()
	if root != ix.root {
		ix.path = filepath.Join(root, ".spotseek", filepath.Base(ix.path))
	}
	ix.root = root
	ix.generation++
	ix.memo = nil
	ix.mu.Unlock()
}

// Len returns the number of entries (both keys counted).
func (ix *Index) Len() int {
	ix.mu.RLock()
	defer ix.mu.RUnlock()
	return len(ix.entries)
}

func validFile(path string) bool {
	if path == "" {
		return false
	}
	info, err := os.Stat(path)
	return err == nil && !info.IsDir()
}

// Lookup finds a local file for a Spotify track.
func (ix *Index) Lookup(t spotify.Track) LookupResult {
	return ix.lookupIndexed(t)
}

func isAudio(path string) bool {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".mp3", ".flac", ".m4a", ".opus", ".wav", ".ogg", ".aac", ".webm":
		return true
	}
	return false
}

// Scan rebuilds the index from the music folder. It reads sockseek
// `_index.csv` files for exact metadata and parses file names for the rest.
// Entries learned from downloads (with a Spotify id) survive a rescan.
func (ix *Index) Scan(logf func(string)) (int, error) {
	if logf == nil {
		logf = func(string) {}
	}
	ix.mu.RLock()
	root := ix.root
	old := make(map[string]Entry, len(ix.entries))
	for k, v := range ix.entries {
		old[k] = v
	}
	ix.mu.RUnlock()

	fresh := make(map[string]Entry)
	put := func(e Entry) {
		tmp := &Index{entries: fresh}
		tmp.putLocked(e)
	}
	// Keep known Spotify ids whose file still exists.
	for k, e := range old {
		if strings.HasPrefix(k, "id:") && validFile(e.Path) {
			put(e)
		}
	}

	var audioFiles []string
	var indexFiles []string
	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}
		if info.IsDir() {
			name := info.Name()
			if name == ".spotseek" || name == ".work" || strings.HasPrefix(name, ".") {
				return filepath.SkipDir
			}
			return nil
		}
		if strings.EqualFold(info.Name(), "_index.csv") {
			indexFiles = append(indexFiles, path)
			return nil
		}
		if isAudio(path) && !strings.HasSuffix(path, ".incomplete") {
			audioFiles = append(audioFiles, path)
		}
		return nil
	})
	if err != nil {
		return 0, err
	}

	covered := map[string]bool{}
	for _, idx := range indexFiles {
		entries := parseSockseekIndex(idx)
		for _, e := range entries {
			if validFile(e.Path) {
				covered[filepath.Clean(e.Path)] = true
				put(e)
			}
		}
	}
	for _, path := range audioFiles {
		path = filepath.Clean(path)
		if covered[path] {
			continue
		}
		artist, title := ParseFileName(strings.TrimSuffix(filepath.Base(path), filepath.Ext(path)))
		put(Entry{
			Artist: artist,
			Title:  title,
			Path:   path,
			Source: "scan",
		})
	}

	ix.mu.Lock()
	if ix.root != root {
		ix.mu.Unlock()
		return 0, fmt.Errorf("music folder changed during scan")
	}
	// Keep entries that were added by a download while the scan was running.
	for k, e := range ix.entries {
		if !strings.HasPrefix(k, "id:") {
			continue
		}
		if previous, ok := old[k]; !ok || previous != e {
			put(e)
		}
	}
	ix.entries = fresh
	ix.rebuildMatchesLocked()
	ix.mu.Unlock()
	if err := ix.Save(); err != nil {
		return 0, err
	}
	logf(fmt.Sprintf("library index: %d files in %s", len(audioFiles), root))
	return len(audioFiles), nil
}

func parseSockseekIndex(path string) []Entry {
	f, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer f.Close()
	rows, err := csv.NewReader(f).ReadAll()
	if err != nil || len(rows) < 2 {
		return nil
	}
	col := func(name string) int {
		for i, h := range rows[0] {
			if strings.EqualFold(strings.TrimSpace(h), name) {
				return i
			}
		}
		return -1
	}
	fp, ar, al, ti, le := col("filepath"), col("artist"), col("album"), col("title"), col("length")
	if fp < 0 {
		return nil
	}
	dir := filepath.Dir(path)
	var out []Entry
	for _, row := range rows[1:] {
		get := func(i int) string {
			if i >= 0 && i < len(row) {
				return strings.TrimSpace(row[i])
			}
			return ""
		}
		file := get(fp)
		if file == "" {
			continue
		}
		file = filepath.FromSlash(file)
		if !filepath.IsAbs(file) {
			file = filepath.Join(dir, file)
		}
		if !isAudio(file) {
			continue
		}
		dur, _ := strconv.Atoi(get(le))
		out = append(out, Entry{
			Artist:      get(ar),
			Title:       get(ti),
			Album:       get(al),
			DurationSec: dur,
			Path:        filepath.Clean(file),
			Source:      "soulseek",
		})
	}
	return out
}
