package pipeline

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"spotseek-app/internal/spotify"
)

func TestDragPreparationSearchesWithoutDownloadingAndReusesLink(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell fixture")
	}
	dir := t.TempDir()
	binary := filepath.Join(dir, "sockseek")
	script := `#!/bin/sh
case "$*" in
 *--print*) echo search >> "$TEST_PREP_DIR/calls"; echo 'slsk://peer/Artist - Song.mp3';;
 *)
    case "$1" in slsk://*) ;; *) echo 'search repeated instead of using prepared link' >&2;exit 1;;esac
    echo download >> "$TEST_PREP_DIR/calls"
    while [ $# -gt 0 ];do
      if [ "$1" = '-o' ];then out=$2;shift;fi
      shift
    done
    mkdir -p "$out"
    echo audio > "$out/Artist - Song.mp3"
    ;;
esac
`
	if err := os.WriteFile(binary, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("TEST_PREP_DIR", dir)
	probe := filepath.Join(dir, "ffprobe")
	os.WriteFile(probe, []byte("#!/bin/sh\necho 320000\n"), 0o755)
	track := spotify.Track{ID: "track", Artist: "Artist", Title: "Song", DurationSec: 180}
	warmOpts := &Options{WorkDir: filepath.Join(dir, "warm"), SockseekPath: binary, Bitrate: "320"}
	p := PrepareTrack(context.Background(), warmOpts, track, true)
	defer p.Stop()
	link, err := p.soulseekLink(context.Background())
	if err != nil || link == "" {
		t.Fatalf("prepare: %q %v", link, err)
	}
	calls, _ := os.ReadFile(filepath.Join(dir, "calls"))
	if strings.TrimSpace(string(calls)) != "search" {
		t.Fatalf("audio downloaded during drag: %s", calls)
	}
	res, err := Run(context.Background(), &Options{Collection: &spotify.Collection{Name: "Test", Kind: "track", Tracks: []spotify.Track{track}}, OutputDir: filepath.Join(dir, "audio"), WorkDir: filepath.Join(dir, "work"), SockseekPath: binary, FfprobePath: probe, Preparation: p})
	if err != nil || res.Files != 1 || res.Missing != 0 {
		t.Fatalf("drop failed: %+v %v", res, err)
	}
	calls, _ = os.ReadFile(filepath.Join(dir, "calls"))
	if strings.TrimSpace(string(calls)) != "search\ndownload" {
		t.Fatalf("wrong search/download sequence: %s", calls)
	}
}
