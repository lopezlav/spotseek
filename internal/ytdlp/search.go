package ytdlp

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"spotseek-app/internal/timing"
)

// Shared across jobs, so concurrent server downloads cannot each create an
// unbounded set of Python processes. Individual metadata batches use three.
var searchProcessSlots = make(chan struct{}, 4)

const metadataBatchSize = 3

var videoID = regexp.MustCompile(`^[A-Za-z0-9_-]{11}$`)

type searchEntry struct {
	ID       string  `json:"id"`
	URL      string  `json:"url"`
	IEKey    string  `json:"ie_key"`
	Title    string  `json:"title"`
	Artist   string  `json:"artist"`
	Channel  string  `json:"channel"`
	Uploader string  `json:"uploader"`
	Album    string  `json:"album"`
	Duration float64 `json:"duration"`
}

func (e searchEntry) isVideo() bool {
	if !videoID.MatchString(e.ID) || (e.IEKey != "" && e.IEKey != "Youtube") {
		return false
	}
	if e.URL == "" || e.URL == e.ID {
		return true
	}
	u, err := url.Parse(e.URL)
	if err != nil {
		return false
	}
	switch u.Hostname() {
	case "www.youtube.com", "youtube.com", "music.youtube.com":
		return u.Path == "/watch" && u.Query().Get("v") == e.ID
	case "youtu.be":
		return strings.TrimPrefix(u.Path, "/") == e.ID
	}
	return false
}

func (e searchEntry) candidate() Candidate {
	artist := e.Artist
	if artist == "" {
		artist = e.Channel
	}
	if artist == "" {
		artist = e.Uploader
	}
	return Candidate{Title: e.Title, Artist: artist, Album: e.Album,
		Duration: int(e.Duration), ID: e.ID, URL: "https://www.youtube.com/watch?v=" + e.ID}
}

func parseEntries(out string) []searchEntry {
	var entries []searchEntry
	for _, line := range strings.Split(out, "\n") {
		var entry searchEntry
		if json.Unmarshal([]byte(line), &entry) == nil {
			entries = append(entries, entry)
		}
	}
	return entries
}

// SearchCandidates searches both sources concurrently and validates full
// metadata in bounded batches. Flat metadata only orders the shortlist; it
// never supplies the final score used to select a download.
func SearchCandidates(ctx context.Context, artist, title string, length, limit int) (cands []Candidate, retErr error) {
	finish := timing.Begin(ctx, "youtube.search_total", strings.TrimSpace(artist+" "+title))
	defer func() { finish(retErr) }()
	if limit <= 0 {
		return nil, fmt.Errorf("search limit must be positive")
	}
	q := strings.TrimSpace(artist + " " + title)
	type sourceResult struct {
		entries []searchEntry
		err     error
	}
	results := make([]sourceResult, 2)
	var wg sync.WaitGroup
	sources := []struct {
		stage string
		args  []string
	}{
		{"youtube.music_search", []string{"--skip-download", "--flat-playlist", "--dump-json", "--playlist-end", strconv.Itoa(limit), "https://music.youtube.com/search?q=" + url.QueryEscape(q)}},
		{"youtube.video_search", []string{"--skip-download", "--flat-playlist", "--dump-json", fmt.Sprintf("ytsearch%d:%s", limit, q)}},
	}
	for i, source := range sources {
		wg.Add(1)
		go func() {
			defer wg.Done()
			end := timing.Begin(ctx, source.stage, q)
			out, err := runYtDlp(ctx, 90*time.Second, source.args...)
			end(err)
			results[i] = sourceResult{parseEntries(out), err}
		}()
	}
	wg.Wait()
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}
	var entries []searchEntry
	seen := map[string]bool{}
	for _, result := range results {
		for _, entry := range result.entries {
			if entry.isVideo() && !seen[entry.ID] {
				seen[entry.ID] = true
				entries = append(entries, entry)
			}
		}
	}
	if len(entries) == 0 {
		return nil, errors.Join(results[0].err, results[1].err)
	}
	if len(entries) > limit*2 {
		entries = entries[:limit*2]
	}
	// Preserve original source order for equal final scores, even though flat
	// metadata chooses which candidates to validate first.
	order := make([]int, len(entries))
	for i := range order {
		order[i] = i
	}
	sort.SliceStable(order, func(i, j int) bool {
		return score(entries[order[i]].candidate(), title, artist, length) > score(entries[order[j]].candidate(), title, artist, length)
	})
	validated := make([]*Candidate, len(entries))
	metadataCtx, cancelMetadata := context.WithTimeout(ctx, 180*time.Second)
	defer cancelMetadata()
	metadataEnd := timing.Begin(ctx, "youtube.candidate_metadata", fmt.Sprintf("%s; %d valid candidates", q, len(entries)))
	var probeErrors []error
	probed := 0
	for len(order) > 0 {
		batch := order
		if len(batch) > metadataBatchSize {
			batch = batch[:metadataBatchSize]
		}
		order = order[len(batch):]
		batchErrors := make([]error, len(batch))
		for i, index := range batch {
			wg.Add(1)
			go func() {
				defer wg.Done()
				entry := entries[index]
				end := timing.Begin(ctx, "youtube.candidate_metadata_item", entry.ID+"; "+entry.Title)
				out, err := runYtDlp(metadataCtx, 60*time.Second, "--skip-download", "--no-playlist", "--dump-json", entry.candidate().URL)
				batchErrors[i] = err
				end(err)
				for _, full := range parseEntries(out) {
					if full.ID == entry.ID {
						c := full.candidate()
						c.Score = score(c, title, artist, length)
						validated[index] = &c
						break
					}
				}
			}()
		}
		wg.Wait()
		probed += len(batch)
		for _, err := range batchErrors {
			if err != nil {
				probeErrors = append(probeErrors, err)
			}
		}
		if ctx.Err() != nil {
			metadataEnd(ctx.Err())
			return nil, ctx.Err()
		}
		if metadataCtx.Err() != nil {
			break
		}
		// Twelve is the maximum possible score under the existing matcher.
		// Only then can later candidates be omitted without hiding a better
		// match. Earlier entries still get checked to preserve stable ties.
		bestIndex := -1
		for i, c := range validated {
			if c != nil && c.Score >= 12 {
				bestIndex = i
				break
			}
		}
		if bestIndex >= 0 {
			pending := order[:0]
			for _, i := range order {
				if i < bestIndex {
					pending = append(pending, i)
				}
			}
			order = pending
		}
	}
	metadataEnd(errors.Join(probeErrors...))
	statsEnd := timing.Begin(ctx, "youtube.metadata_candidates_checked", fmt.Sprintf("%s; checked %d of %d", q, probed, len(entries)))
	statsEnd(nil)
	rankingEnd := timing.Begin(ctx, "youtube.ranking", q)
	for _, c := range validated {
		if c != nil {
			cands = append(cands, *c)
		}
	}
	sort.SliceStable(cands, func(i, j int) bool { return cands[i].Score > cands[j].Score })
	rankingEnd(nil)
	if len(cands) == 0 {
		return nil, errors.Join(probeErrors...)
	}
	return cands, nil
}
