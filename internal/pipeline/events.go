package pipeline

import (
	"encoding/json"
	"path/filepath"
	"regexp"
	"strings"
	"sync"

	"spotseek-app/internal/spotify"
)

// Event is one structured progress message for a front end. The CLI writes
// each event as one JSON line when it runs with --events.
type Event struct {
	Ev string `json:"ev"`

	// phase: spotify, soulseek, youtube, convert, done
	Phase string `json:"phase,omitempty"`

	// collection
	Name   string       `json:"name,omitempty"`
	Kind   string       `json:"kind,omitempty"`
	Artist string       `json:"artist,omitempty"`
	Tracks []TrackEvent `json:"tracks,omitempty"`

	// track and bytes. I is the track index in the collection.
	I      *int   `json:"i,omitempty"`
	State  string `json:"state,omitempty"`
	Peer   string `json:"peer,omitempty"`
	Source string `json:"source,omitempty"`
	Bytes  int64  `json:"bytes,omitempty"`
	Total  int64  `json:"total,omitempty"`
	// BitRate is the kbps of the Soulseek file; URL is the YouTube page of a
	// fallback download.
	BitRate int    `json:"bitrate,omitempty"`
	URL     string `json:"url,omitempty"`

	// file: Path is the final mp3 of track I in the output folder.
	Path string `json:"path,omitempty"`

	// collection: Cover is a URL of the cover image (albums and tracks).
	Cover string `json:"cover,omitempty"`

	// convert and result
	Done    int    `json:"done,omitempty"`
	Count   int    `json:"count,omitempty"`
	Files   int    `json:"files,omitempty"`
	Missing int    `json:"missing,omitempty"`
	Dir     string `json:"dir,omitempty"`

	// log and error
	Line    string `json:"line,omitempty"`
	Message string `json:"message,omitempty"`
}

// TrackEvent is one track in a collection event.
type TrackEvent struct {
	Artist   string `json:"artist"`
	Title    string `json:"title"`
	Duration int    `json:"duration"`
	Album    string `json:"album,omitempty"`
	ID       string `json:"id,omitempty"` // Spotify track id
}

// Track states sent in track events.
const (
	StateSearching   = "searching"
	StateDownloading = "downloading"
	StateDone        = "done"
	StateFailed      = "failed"      // not found on Soulseek; the fallback may still find it
	StateYTSearch    = "yt_search"   // looking on YouTube Music
	StateYTDownload  = "yt_download" // downloading from YouTube Music
	StateMissing     = "missing"     // not found anywhere
)

func (o *Options) emit(e Event) {
	if o.Event != nil {
		o.Event(e)
	}
}

func intp(i int) *int { return &i }

// tracker maps Sockseek's JSON progress lines to track indexes.
type tracker struct {
	mu     sync.Mutex
	o      *Options
	tracks []spotify.Track
	byKey  map[string][]int // normalized "artist|title" -> indexes
	state  []string
	size   []int64        // file size of the current download per track
	src    []string       // downloaded file (before conversion) per track
	jobs   map[string]int // Sockseek job id -> track index
	// album: Sockseek downloads one album folder at a time. A failed file
	// then means "this folder failed", and Sockseek tries the next folder;
	// only the end of the process says what is missing.
	album bool
}

func newTracker(o *Options, tracks []spotify.Track) *tracker {
	t := &tracker{
		o:      o,
		tracks: tracks,
		byKey:  map[string][]int{},
		state:  make([]string, len(tracks)),
		size:   make([]int64, len(tracks)),
		src:    make([]string, len(tracks)),
		jobs:   map[string]int{},
	}
	for i, tr := range tracks {
		k := trackKey(tr.Artist, tr.Title)
		t.byKey[k] = append(t.byKey[k], i)
	}
	return t
}

func trackKey(artist, title string) string {
	return normTitle(artist) + "|" + normTitle(title)
}

// find returns the index of the track that Sockseek means. It tries, in order:
// the exact artist and title; the exact title (also without "(feat. X)"-style
// parts); the title in the peer's file name; and a part of the title, but
// only when that points at one track. Sockseek reports album files with the
// album artist and the peer's file name as the title, so loose matching
// would put them on the wrong rows. With duplicates it prefers a track that
// is not done.
func (t *tracker) find(artist, title string) int {
	return t.findFile(artist, title, "")
}

func (t *tracker) findFile(artist, title, filename string) int {
	pick := func(idx []int) int {
		for _, i := range idx {
			if t.state[i] != StateDone {
				return i
			}
		}
		if len(idx) > 0 {
			return idx[0]
		}
		return -1
	}
	if i := pick(t.byKey[trackKey(artist, title)]); i >= 0 {
		return i
	}
	exact := func(name string) []int {
		n := normTitle(name)
		if n == "" {
			return nil
		}
		var idx []int
		for i, tr := range t.tracks {
			if normTitle(tr.Title) == n {
				idx = append(idx, i)
			}
		}
		if len(idx) == 0 {
			for i, tr := range t.tracks {
				if normTitle(baseTitle(tr.Title)) == n || normTitle(tr.Title) == normTitle(baseTitle(name)) {
					idx = append(idx, i)
				}
			}
		}
		return idx
	}
	if i := pick(exact(title)); i >= 0 {
		return i
	}
	stem := ""
	if filename != "" {
		// Peers use \ or / in paths.
		base := filename[strings.LastIndexAny(filename, `\/`)+1:]
		stem = leadingNumber.ReplaceAllString(strings.TrimSuffix(base, filepath.Ext(base)), "")
		if i := pick(exact(stem)); i >= 0 {
			return i
		}
	}
	for _, name := range []string{title, stem} {
		nt := normTitle(name)
		if nt == "" {
			continue
		}
		var idx []int
		for i, tr := range t.tracks {
			ot := normTitle(tr.Title)
			if ot != "" && (strings.Contains(nt, ot) || strings.Contains(ot, nt)) {
				idx = append(idx, i)
			}
		}
		if len(idx) == 1 {
			return idx[0]
		}
	}
	return -1
}

// set sends a state for track i. A track that is done stays done: Sockseek
// can report a later attempt for a title that is already in the folder.
func (t *tracker) set(i int, state string, e Event) {
	if t.state[i] == StateDone && state != StateDone {
		return
	}
	t.setAny(i, state, e)
}

// setAny is set without the guard, for the final check of an album folder.
func (t *tracker) setAny(i int, state string, e Event) {
	t.state[i] = state
	e.Ev = "track"
	e.I = intp(i)
	e.State = state
	t.o.emit(e)
}

type ssLine struct {
	Type string `json:"type"`
	Data struct {
		Artist           string `json:"artist"`
		Title            string `json:"title"`
		Username         string `json:"username"`
		Size             int64  `json:"size"`
		JobID            string `json:"jobId"`
		BytesTransferred int64  `json:"bytesTransferred"`
		TotalBytes       int64  `json:"totalBytes"`
		TerminalOutcome  string `json:"terminalOutcome"`
		Filename         string `json:"filename"`
		Extension        string `json:"extension"`
		LifecycleState   string `json:"lifecycleState"`
		ActivityPhase    string `json:"activityPhase"`
		DownloadPath     string `json:"downloadPath"`
		BitRate          int    `json:"bitRate"`
	} `json:"data"`
}

// handle reads one Sockseek line. It returns false when the line is not a
// Sockseek JSON event, so the caller logs it as text.
func (t *tracker) handle(line string) bool {
	if !strings.HasPrefix(line, "{") {
		return false
	}
	var l ssLine
	if err := json.Unmarshal([]byte(line), &l); err != nil || l.Type == "" {
		return false
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	d := l.Data
	switch l.Type {
	case "search_start":
		if i := t.find(d.Artist, d.Title); i >= 0 {
			t.set(i, StateSearching, Event{})
		}
	case "download_start":
		// Album folders also hold cover images and text files.
		if ext := strings.TrimPrefix(strings.ToLower(d.Extension), "."); ext != "" && !audioExt("x."+ext) {
			return true
		}
		if i := t.findFile(d.Artist, d.Title, d.Filename); i >= 0 {
			t.size[i] = d.Size
			t.set(i, StateDownloading, Event{Peer: d.Username, Total: d.Size, Source: "soulseek"})
		}
	case "download_progress":
		// Progress lines carry only a job id. Match the job to the track
		// whose current download has the same file size.
		i, ok := t.jobs[d.JobID]
		if !ok || t.size[i] != d.TotalBytes {
			// Bind the job only when exactly one active download has this size.
			i = -1
			for k, s := range t.size {
				if s == d.TotalBytes && t.state[k] == StateDownloading {
					if i >= 0 {
						return true
					}
					i = k
				}
			}
			if i < 0 {
				return true
			}
			t.jobs[d.JobID] = i
		}
		t.o.emit(Event{Ev: "bytes", I: intp(i), Bytes: d.BytesTransferred, Total: d.TotalBytes})
	case "track_state":
		i := t.findFile(d.Artist, d.Title, d.Filename)
		if i < 0 {
			return true
		}
		// Sockseek sends track_state on every change of a job, also while it
		// still searches, waits in a queue or downloads (outcome "None"). Only
		// a terminal job has a result.
		if d.LifecycleState != "" && d.LifecycleState != "Terminal" {
			return true
		}
		switch d.TerminalOutcome {
		case "Succeeded", "PartialSuccess":
			if d.DownloadPath != "" {
				t.src[i] = filepath.Clean(d.DownloadPath)
			}
			t.set(i, StateDone, Event{Peer: d.Username, Total: d.Size, BitRate: d.BitRate, Source: "soulseek"})
		case "Skipped":
			// Already in the folder from an earlier run.
			t.set(i, StateDone, Event{Source: "soulseek"})
		case "Failed", "Cancelled":
			if t.album {
				// This folder failed; Sockseek goes on with the next one.
				t.set(i, StateSearching, Event{})
			} else {
				t.set(i, StateFailed, Event{})
			}
		}
	}
	return true
}

var leadingNumber = regexp.MustCompile(`^[0-9]{1,3}\s*[-._)]?\s*`)

// fileIndex returns the track that a downloaded file belongs to, or -1. It
// uses the path Sockseek reported, then the file name: "Artist - Title"
// (YouTube, per-track Soulseek) or "07 - Title" (album folders).
func (t *tracker) fileIndex(src string, used map[int]bool) int {
	t.mu.Lock()
	defer t.mu.Unlock()
	clean := filepath.Clean(src)
	for i, p := range t.src {
		if p != "" && p == clean && !used[i] {
			return i
		}
	}
	name := strings.TrimSuffix(filepath.Base(src), filepath.Ext(src))
	name = leadingNumber.ReplaceAllString(name, "")
	nn := normTitle(name)
	best, bestLen := -1, 0
	for i, tr := range t.tracks {
		if used[i] {
			continue
		}
		for _, title := range []string{tr.Title, baseTitle(tr.Title)} {
			nt := normTitle(title)
			if nt == "" || !strings.Contains(nn, nt) {
				continue
			}
			// The longest matching title wins ("Thriller" vs "Thriller - Instrumental").
			score := len(nt)
			if na := normTitle(tr.Artist); na != "" && strings.Contains(nn, na) {
				score += 1000
			}
			if score > bestLen {
				best, bestLen = i, score
			}
		}
	}
	return best
}

// findLocked is find for callers that do not hold the lock.
func (t *tracker) findLocked(artist, title string) int {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.find(artist, title)
}

// setSrc records the downloaded file of track i (YouTube downloads).
func (t *tracker) setSrc(i int, path string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.src[i] = filepath.Clean(path)
}
