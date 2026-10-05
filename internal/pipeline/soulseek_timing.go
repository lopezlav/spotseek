package pipeline

import (
	"context"
	"encoding/json"
	"time"

	"spotseek-app/internal/timing"
)

// Times are observed stdout boundaries. Startup includes login; selection
// includes the search and peer choice; transfer includes peer connection/queue.
// These intervals must not be represented as pure login or wire-transfer time.
type soulseekTiming struct {
	ctx        context.Context
	started    time.Time
	searchSeen bool
	active     map[string]*soulseekTrackTiming
}

type soulseekTrackTiming struct {
	stage           string
	started         time.Time
	firstByte       bool
	downloadStarted time.Time
}

func newSoulseekTiming(ctx context.Context) *soulseekTiming {
	return &soulseekTiming{ctx: ctx, started: time.Now(), active: map[string]*soulseekTrackTiming{}}
}

func (s *soulseekTiming) line(line string) {
	if !timing.Enabled(s.ctx) {
		return
	}
	var ev struct {
		Type string `json:"type"`
		Data struct {
			Artist  string `json:"artist"`
			Title   string `json:"title"`
			Bytes   int64  `json:"bytesTransferred"`
			Outcome string `json:"terminalOutcome"`
			Failure string `json:"failureReason"`
		} `json:"data"`
	}
	if json.Unmarshal([]byte(line), &ev) != nil {
		return
	}
	key := ev.Data.Artist + " - " + ev.Data.Title
	t := s.active[key]
	close := func(err error) {
		if t != nil {
			timing.Interval(s.ctx, t.stage, key, t.started, err)
		}
	}
	switch ev.Type {
	case "search_start":
		if !s.searchSeen {
			timing.Interval(s.ctx, "soulseek.startup_to_search", "includes process startup and login", s.started, nil)
			s.searchSeen = true
		}
		close(nil)
		s.active[key] = &soulseekTrackTiming{stage: "soulseek.search_and_selection", started: time.Now()}
	case "download_start":
		close(nil)
		now := time.Now()
		s.active[key] = &soulseekTrackTiming{stage: "soulseek.transfer", started: now, downloadStarted: now}
	case "download_progress":
		// Newer versions only include a jobId. A sole active download is
		// unambiguous; concurrent downloads cannot be attributed safely.
		if t == nil && len(s.active) == 1 {
			for activeKey, activeTrack := range s.active {
				key, t = activeKey, activeTrack
			}
		}
		if t != nil && t.stage == "soulseek.transfer" && !t.firstByte && ev.Data.Bytes > 0 {
			timing.Interval(s.ctx, "soulseek.time_to_first_progress", key, t.downloadStarted, nil)
			t.firstByte = true
		}
	case "track_state":
		if ev.Data.Outcome == "None" || ev.Data.Outcome == "" {
			return
		}
		var err error
		if ev.Data.Failure != "" && ev.Data.Failure != "None" {
			err = &stageError{ev.Data.Failure}
		}
		close(err)
		delete(s.active, key)
	}
}

type stageError struct{ message string }

func (e *stageError) Error() string { return e.message }

func (s *soulseekTiming) finish(err error) {
	if !s.searchSeen {
		timing.Interval(s.ctx, "soulseek.startup_to_search", "process ended before search", s.started, err)
	}
	for key, t := range s.active {
		timing.Interval(s.ctx, t.stage, key, t.started, err)
	}
}
