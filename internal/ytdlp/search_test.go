package ytdlp

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"spotseek-app/internal/timing"
)

func TestSearchEntryRejectsBrowseResults(t *testing.T) {
	for _, entry := range []searchEntry{
		{ID: "UCtHVFWQtBzPt8m0J9e59TSQ", URL: "https://music.youtube.com/browse/UCtHVFWQtBzPt8m0J9e59TSQ", IEKey: "YoutubeTab"},
		{ID: "MPREb_RrNUGwNPHsE", IEKey: "YoutubeTab"},
		{ID: "aaaaaaaaaaa", URL: "https://music.youtube.com/browse/aaaaaaaaaaa"},
		{ID: "aaaaaaaaaaa", URL: "https://example.test/watch?v=aaaaaaaaaaa"},
		{ID: "aaaaaaaaaaa", URL: "https://youtube.com/watch?v=bbbbbbbbbbb"},
	} {
		if entry.isVideo() {
			t.Errorf("accepted non-video: %+v", entry)
		}
	}
	if !(searchEntry{ID: "aaaaaaaaaaa", URL: "https://music.youtube.com/watch?v=aaaaaaaaaaa", IEKey: "Youtube"}).isVideo() {
		t.Fatal("rejected valid video")
	}
}

func fakeSearchBinary(t *testing.T, script string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("fixture uses a shell executable")
	}
	dir := t.TempDir()
	path := filepath.Join(dir, "yt-dlp")
	if err := os.WriteFile(path, []byte("#!/bin/sh\ncd "+fmt.Sprintf("%q", dir)+"\n"+script), 0o755); err != nil {
		t.Fatal(err)
	}
	old := Binary
	Binary = path
	t.Cleanup(func() { Binary = old })
	return dir
}

const searchFixture = `case "$*" in
  *music.youtube.com/search*#songs)
    echo started > music-started
    while [ ! -f video-started ]; do sleep .01; done
    echo '{"id":"aaaaaaaaaaa","title":"Song","artist":"Artist","duration":180}'
    echo '{"id":"bbbbbbbbbbb","title":"Song","artist":"Artist","duration":180}'
    echo '{"id":"ccccccccccc","title":"Song","artist":"Artist","duration":180}'
    echo '{"id":"UCtHVFWQtBzPt8m0J9e59TSQ","url":"https://music.youtube.com/browse/UCtHVFWQtBzPt8m0J9e59TSQ","ie_key":"YoutubeTab"}'
    ;;
  *ytsearch*)
    echo started > video-started
    while [ ! -f music-started ]; do sleep .01; done
    echo '{"id":"aaaaaaaaaaa","title":"Song","artist":"Artist","duration":180}'
    echo '{"id":"ddddddddddd","title":"Song","artist":"Artist","duration":180}'
    ;;
  *)
    for last; do :; done
    id=${last##*=}
    echo "$id" >> probes
    echo started > "$id-started"
    while [ ! -f aaaaaaaaaaa-started ] || [ ! -f bbbbbbbbbbb-started ] || [ ! -f ccccccccccc-started ]; do sleep .01; done
    case "$id" in
      aaaaaaaaaaa) echo '{"id":"aaaaaaaaaaa","title":"Song","artist":"Artist","album":"Album","duration":400}';;
      bbbbbbbbbbb) echo '{"id":"bbbbbbbbbbb","title":"Song","artist":"Artist","album":"Album","duration":180}';;
      ccccccccccc) echo '{"id":"ccccccccccc","title":"Song","artist":"Artist","album":"Album","duration":180}';;
      *) echo unexpected-probe >&2;exit 1;;
    esac
    ;;
esac
`

const musicSearchFixture = `case "$*" in
  *music.youtube.com/search*#songs)
    echo '{"id":"aaaaaaaaaaa","title":"Song One","artist":"Artist","duration":180}'
    echo '{"id":"bbbbbbbbbbb","title":"Song Two","channel":"Other Artist - Topic","duration":245.5}'
    echo '{"id":"UCtHVFWQtBzPt8m0J9e59TSQ","url":"https://music.youtube.com/browse/UCtHVFWQtBzPt8m0J9e59TSQ","ie_key":"YoutubeTab"}'
    echo '{"id":"aaaaaaaaaaa","title":"Song One","artist":"Artist","duration":180}'
    ;;
  *)
    echo unexpected-call >&2
    exit 1
    ;;
esac
`

func TestSearchMusicReturnsFlatResultsInOrder(t *testing.T) {
	fakeSearchBinary(t, musicSearchFixture)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cands, err := SearchMusic(ctx, "artist song", 20)
	if err != nil {
		t.Fatal(err)
	}
	if len(cands) != 2 || cands[0].ID != "aaaaaaaaaaa" || cands[1].ID != "bbbbbbbbbbb" {
		t.Fatalf("incorrect results: %+v", cands)
	}
	if cands[1].Artist != "Other Artist - Topic" || cands[1].Duration != 245 {
		t.Fatalf("incorrect metadata: %+v", cands[1])
	}
	if cands[0].URL != "https://www.youtube.com/watch?v=aaaaaaaaaaa" {
		t.Fatalf("incorrect url: %s", cands[0].URL)
	}
}

func TestSearchMusicRejectsEmptyQuery(t *testing.T) {
	if _, err := SearchMusic(context.Background(), "   ", 20); err == nil {
		t.Fatal("accepted empty query")
	}
}

func TestSearchConcurrentShortlistPreservesFullScoringAndTies(t *testing.T) {
	dir := fakeSearchBinary(t, searchFixture)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	var events []timing.Event
	ctx = timing.WithObserver(ctx, func(e timing.Event) { events = append(events, e) })
	cands, err := SearchCandidates(ctx, "Artist", "Song", 180, 5)
	if err != nil {
		t.Fatal(err)
	}
	if len(cands) != 3 || cands[0].ID != "bbbbbbbbbbb" || cands[0].Score != 12 || cands[1].ID != "ccccccccccc" {
		t.Fatalf("incorrect ranking: %+v", cands)
	}
	data, err := os.ReadFile(filepath.Join(dir, "probes"))
	if err != nil {
		t.Fatal(err)
	}
	if len(strings.Fields(string(data))) != 3 || strings.Contains(string(data), "ddddddddddd") || strings.Contains(string(data), "UC") {
		t.Fatalf("shortlist/filtering failed: %s", data)
	}
	if len(searchProcessSlots) != 0 {
		t.Fatal("process slots leaked")
	}
}

func TestSearchBroadensWhenShortlistIsNotMaximumScore(t *testing.T) {
	script := strings.ReplaceAll(searchFixture, `"album":"Album","duration":180`, `"album":"","duration":180`)
	script = strings.Replace(script, "*) echo unexpected-probe >&2;exit 1;;", `ddddddddddd) echo '{"id":"ddddddddddd","title":"Song","artist":"Artist","album":"Album","duration":180}';;
      *) echo unexpected-probe >&2;exit 1;;`, 1)
	dir := fakeSearchBinary(t, script)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cands, err := SearchCandidates(ctx, "Artist", "Song", 180, 5)
	if err != nil {
		t.Fatal(err)
	}
	if len(cands) != 4 || cands[0].ID != "ddddddddddd" || cands[0].Score != 12 {
		t.Fatalf("did not discover better later match: %+v", cands)
	}
	data, _ := os.ReadFile(filepath.Join(dir, "probes"))
	if len(strings.Fields(string(data))) != 4 {
		t.Fatalf("wrong probe count: %s", data)
	}
}

func TestSearchCancellationReleasesProcesses(t *testing.T) {
	script := strings.Replace(searchFixture, "case \"$id\" in", "exec sleep 30\n    case \"$id\" in", 1)
	dir := fakeSearchBinary(t, script)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	done := make(chan error, 1)
	go func() { _, err := SearchCandidates(ctx, "Artist", "Song", 180, 5); done <- err }()
	deadline := time.Now().Add(3 * time.Second)
	for {
		if _, err := os.Stat(filepath.Join(dir, "bbbbbbbbbbb-started")); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("metadata did not start")
		}
		time.Sleep(10 * time.Millisecond)
	}
	cancel()
	select {
	case err := <-done:
		if err != context.Canceled {
			t.Fatalf("cancellation error: %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("canceled subprocesses did not finish")
	}
	if len(searchProcessSlots) != 0 {
		t.Fatal("process slots leaked on cancellation")
	}
}

func TestParseEntriesSupportsFractionalDurationAndMissingArtist(t *testing.T) {
	out := `{"id":"aaaaaaaaaaa","title":"Song","duration":180.8,"artist":null,"channel":"Artist - Topic"}`
	entries := parseEntries(out)
	if len(entries) != 1 {
		t.Fatal("could not parse result")
	}
	c := entries[0].candidate()
	if c.Duration != 180 || c.Artist != "Artist - Topic" {
		t.Fatalf("incorrect candidate: %+v", c)
	}
}
