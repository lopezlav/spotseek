package pipeline

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"spotseek-app/internal/spotify"
	"spotseek-app/internal/timing"
	"spotseek-app/internal/ytdlp"
)

func TestPreparedFallbackPolicy(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("fixtures use shell executables")
	}
	for _, tc := range []struct {
		name                             string
		fallback, success, blockMetadata bool
	}{
		{"soulseek_success_cancels_pending_metadata", true, true, true},
		{"missing_track_reuses_search_after_soulseek_failure", true, false, false},
		{"fallback_disabled_does_not_search_youtube", false, true, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			writeTool := func(name, body string) string {
				t.Helper()
				path := filepath.Join(dir, name)
				if err := os.WriteFile(path, []byte("#!/bin/sh\ncd "+fmt.Sprintf("%q", dir)+"\n"+body), 0o755); err != nil {
					t.Fatal(err)
				}
				return path
			}
			metadataAction := `echo '{"id":"aaaaaaaaaaa","title":"Song","artist":"Artist","album":"Album","duration":180}'`
			if tc.blockMetadata {
				metadataAction = "exec sleep 30"
			}
			yt := writeTool("yt-dlp", `case "$*" in
  *--flat-playlist*)
    echo query >> flat-queries
    echo '{"id":"aaaaaaaaaaa","title":"Song"}'
    ;;
  *--audio-format*)
    if [ ! -f soulseek-failed ]; then echo premature-download >&2;exit 1;fi
    echo download >> downloads
    while [ $# -gt 0 ]; do
      if [ "$1" = '-o' ]; then template=$2;shift;fi
      shift
    done
    echo audio > "${template%.*}.mp3"
    ;;
  *) echo started > metadata-started
    `+metadataAction+`
    ;;
esac
`)
			wait := ""
			if tc.fallback {
				wait = `while [ ! -f metadata-started ]; do sleep .01;done
`
			}
			outcome := `echo audio > "$output/$folder/Artist - Song.mp3"
printf '%s\n' 'filepath,artist,title,length' "$output/$folder/Artist - Song.mp3,Artist,Song,180" > "$output/$folder/_index.csv"
`
			if !tc.success {
				outcome = `echo failed > soulseek-failed
printf '%s\n' 'filepath,artist,title,length' ',Artist,Song,180' > "$output/$folder/_index.csv"
`
			}
			ss := writeTool("sockseek", `csv=$1
shift
while [ $# -gt 0 ]; do
  if [ "$1" = '-o' ]; then output=$2;shift;fi
  shift
done
folder=$(basename "$csv" .csv)
mkdir -p "$output/$folder"
`+wait+outcome)
			probe := writeTool("ffprobe", "echo 320000\n")
			old := ytdlp.Binary
			ytdlp.Binary = yt
			defer func() { ytdlp.Binary = old }()
			ctx, cancel := context.WithTimeout(context.Background(), 4*time.Second)
			defer cancel()
			var events []timing.Event
			opts := &Options{
				Collection: &spotify.Collection{Kind: "track", Name: "Artist - Song", Tracks: []spotify.Track{{Artist: "Artist", Title: "Song", DurationSec: 180}}},
				OutputDir:  filepath.Join(dir, "output"), WorkDir: filepath.Join(dir, "work"), UseFallback: tc.fallback,
				SockseekPath: ss, FfprobePath: probe,
				OnTiming: func(e timing.Event) { events = append(events, e) },
			}
			res, err := Run(ctx, opts)
			if err != nil {
				t.Fatal(err)
			}
			if res.Files != 1 {
				t.Fatalf("wrong output: %+v", res)
			}
			if tc.success && res.Missing != 0 {
				t.Fatalf("Soulseek success lost: %+v", res)
			}
			queries, _ := os.ReadFile(filepath.Join(dir, "flat-queries"))
			downloads, _ := os.ReadFile(filepath.Join(dir, "downloads"))
			if tc.fallback && len(strings.Fields(string(queries))) != 2 {
				t.Fatalf("search was missing or repeated: %s", queries)
			}
			if !tc.fallback && len(queries) > 0 {
				t.Fatal("searched with fallback disabled")
			}
			if tc.success && len(downloads) > 0 {
				t.Fatal("YouTube downloaded despite Soulseek success")
			}
			if !tc.success && len(strings.Fields(string(downloads))) != 1 {
				t.Fatal("fallback did not download exactly once")
			}
			if tc.blockMetadata {
				canceled := false
				for _, e := range events {
					if e.Stage == "youtube.search_total" && strings.Contains(e.Error, "canceled") {
						canceled = true
					}
				}
				if !canceled {
					t.Fatalf("metadata was not canceled: %+v", events)
				}
			}
		})
	}
}
