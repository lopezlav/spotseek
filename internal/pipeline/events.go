package pipeline

import (
	"encoding/json"
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
	jobs   map[string]int // Sockseek job id -> track index
}

func newTracker(o *Options, tracks []spotify.Track) *tracker {
	t := &tracker{
		o:      o,
		tracks: tracks,
		byKey:  map[string][]int{},
		state:  make([]string, len(tracks)),
		size:   make([]int64, len(tracks)),
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

// find returns the index of the track. With duplicates it prefers one that is
// not finished. It falls back to a title-only match (album mode).
func (t *tracker) find(artist, title string) int {
	idx := t.byKey[trackKey(artist, title)]
	if len(idx) == 0 {
		nt := normTitle(title)
		if nt == "" {
			return -1
		}
		for i, tr := range t.tracks {
			ot := normTitle(tr.Title)
			if ot != "" && (strings.Contains(nt, ot) || strings.Contains(ot, nt)) {
				idx = append(idx, i)
			}
		}
	}
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

func (t *tracker) set(i int, state string, e Event) {
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
		if i := t.find(d.Artist, d.Title); i >= 0 {
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
		i := t.find(d.Artist, d.Title)
		if i < 0 {
			return true
		}
		switch d.TerminalOutcome {
		case "Succeeded":
			t.set(i, StateDone, Event{Peer: d.Username, Total: d.Size, Source: "soulseek"})
		case "Skipped":
			// Already in the folder from an earlier run.
			t.set(i, StateDone, Event{Source: "soulseek"})
		default:
			t.set(i, StateFailed, Event{})
		}
	}
	return true
}
