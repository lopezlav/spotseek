// AI-generated code begins.
package libraryindex

import (
	"sort"
	"time"

	"spotseek-app/internal/spotify"
)

type normalizedEntry struct {
	entry  Entry
	title  string
	artist string
}

type matchBuckets struct {
	exact   map[string]map[string]bool
	grams   map[string]map[string]bool
	unknown map[string]bool
}

func newMatchBuckets() matchBuckets {
	return matchBuckets{
		exact:   make(map[string]map[string]bool),
		grams:   make(map[string]map[string]bool),
		unknown: make(map[string]bool),
	}
}

func addBucket(buckets map[string]map[string]bool, key, path string) {
	if buckets[key] == nil {
		buckets[key] = make(map[string]bool)
	}
	buckets[key][path] = true
}

func (b *matchBuckets) add(text, path string) {
	addBucket(b.exact, text, path)
	if text == "" {
		b.unknown[path] = true
	}
	if len(text) < 3 {
		return
	}
	for i := 0; i+3 <= len(text); i++ {
		addBucket(b.grams, text[i:i+3], path)
	}
}

func (b *matchBuckets) candidates(text string, minContains int) map[string]bool {
	paths := make(map[string]bool)
	for path := range b.exact[text] {
		paths[path] = true
	}
	if len(text) < minContains {
		return paths
	}
	var smallest map[string]bool
	for i := 0; i+3 <= len(text); i++ {
		bucket := b.grams[text[i:i+3]]
		if i == 0 || len(bucket) < len(smallest) {
			smallest = bucket
		}
	}
	for path := range smallest {
		allGrams := true
		for i := 0; i+3 <= len(text); i++ {
			if !b.grams[text[i:i+3]][path] {
				allGrams = false
				break
			}
		}
		if allGrams {
			paths[path] = true
		}
	}
	for start := 0; start+minContains <= len(text); start++ {
		for end := start + minContains; end <= len(text); end++ {
			for path := range b.exact[text[start:end]] {
				paths[path] = true
			}
		}
	}
	return paths
}

func (ix *Index) rebuildMatchesLocked() {
	ix.generation++
	ix.memo = make(map[lookupKey]lookupMemo)
	ix.byTitle = newMatchBuckets()
	ix.byArtist = newMatchBuckets()
	ix.matchEntries = make(map[string]normalizedEntry)
	for _, entry := range ix.entries {
		if _, exists := ix.matchEntries[entry.Path]; exists {
			continue
		}
		e := normalizedEntry{entry: entry, title: Normalize(entry.Title), artist: Normalize(entry.Artist)}
		ix.matchEntries[entry.Path] = e
		ix.byTitle.add(e.title, entry.Path)
		ix.byArtist.add(e.artist, entry.Path)
	}
}

type lookupKey struct {
	id, artist, title string
	duration          int
}

type lookupMemo struct {
	result     LookupResult
	generation uint64
	expires    time.Time
}

func (ix *Index) lookupIndexed(track spotify.Track) LookupResult {
	key := lookupKey{track.ID, Normalize(track.Artist), Normalize(track.Title), track.DurationSec}
	for {
		ix.mu.RLock()
		generation := ix.generation
		cached, haveCache := ix.memo[key]
		ix.mu.RUnlock()
		if haveCache && cached.generation == generation && time.Now().Before(cached.expires) {
			if !cached.result.Found || validFile(cached.result.Path) {
				ix.mu.RLock()
				unchanged := ix.generation == generation
				ix.mu.RUnlock()
				if unchanged {
					return cached.result
				}
			}
		}

		ix.mu.RLock()
		generation = ix.generation
		var exact []struct {
			entry Entry
			match string
		}
		if entry, ok := ix.entries[idKey(track.ID)]; track.ID != "" && ok {
			exact = append(exact, struct {
				entry Entry
				match string
			}{entry, "id"})
		}
		if entry, ok := ix.entries["tt:"+key.artist+"|"+key.title]; ok {
			exact = append(exact, struct {
				entry Entry
				match string
			}{entry, "artist-title"})
		}
		titlePaths := ix.byTitle.candidates(key.title, 6)
		var artistPaths map[string]bool
		if key.artist != "" && len(titlePaths) > 32 {
			artistPaths = ix.byArtist.candidates(key.artist, 4)
		}
		candidates := make([]normalizedEntry, 0, len(titlePaths))
		for path := range titlePaths {
			if artistPaths != nil && !artistPaths[path] && !ix.byArtist.unknown[path] {
				continue
			}
			candidates = append(candidates, ix.matchEntries[path])
		}
		ix.mu.RUnlock()

		var result LookupResult
		for _, candidate := range exact {
			if validFile(candidate.entry.Path) {
				result = LookupResult{true, candidate.entry.Path, candidate.entry.Source, candidate.match}
				break
			}
		}
		if !result.Found {
			sort.Slice(candidates, func(i, j int) bool { return candidates[i].entry.Path < candidates[j].entry.Path })
			for _, candidate := range candidates {
				if TitleMatches(key.title, candidate.title) && ArtistMatches(key.artist, candidate.artist) &&
					DurationClose(key.duration, candidate.entry.DurationSec) && validFile(candidate.entry.Path) {
					result = LookupResult{true, candidate.entry.Path, candidate.entry.Source, "title"}
					break
				}
			}
		}
		ix.mu.Lock()
		if ix.generation != generation {
			ix.mu.Unlock()
			continue
		}
		if ix.memo == nil || len(ix.memo) >= 10000 {
			ix.memo = make(map[lookupKey]lookupMemo)
		}
		ix.memo[key] = lookupMemo{result, generation, time.Now().Add(2 * time.Second)}
		ix.mu.Unlock()
		return result
	}
}

// AI-generated code ends.
