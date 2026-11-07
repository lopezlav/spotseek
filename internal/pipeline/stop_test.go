package pipeline

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"testing"
	"time"

	"spotseek-app/internal/spotify"
)

// Stop ends the downloads, but the finished file is still converted, placed and reported.
func TestStopKeepsTheFinishedTracks(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell fixture")
	}
	dir := t.TempDir()
	sockseek := filepath.Join(dir, "sockseek")
	// One track done, then a long wait (like Soulseek's search limit).
	script := `#!/bin/sh
while [ $# -gt 0 ];do
  if [ "$1" = '-o' ];then out=$2;shift;fi
  shift
done
mkdir -p "$out/Test"
echo audio > "$out/Test/Artist - Song.mp3"
exec sleep 30
`
	if err := os.WriteFile(sockseek, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	probe := filepath.Join(dir, "ffprobe")
	_ = os.WriteFile(probe, []byte("#!/bin/sh\necho 320000\n"), 0o755)
	stop := make(chan struct{})
	var events []Event
	o := &Options{
		Collection:   &spotify.Collection{Name: "Test", Kind: "playlist", Tracks: []spotify.Track{{Artist: "Artist", Title: "Song"}, {Artist: "Other", Title: "Later"}}},
		OutputDir:    filepath.Join(dir, "out"),
		SockseekPath: sockseek,
		FfprobePath:  probe,
		UseFallback:  true, // must not run after a stop
		Stop:         stop,
		Event:        func(e Event) { events = append(events, e) },
	}
	go func() {
		time.Sleep(1500 * time.Millisecond)
		close(stop)
	}()
	start := time.Now()
	res, err := Run(context.Background(), o)
	if err != nil {
		t.Fatal(err)
	}
	if time.Since(start) > 10*time.Second {
		t.Fatalf("the stop did not end the downloads (%s)", time.Since(start))
	}
	if res.Files != 1 {
		t.Fatalf("want the finished track converted, got %d files", res.Files)
	}
	var file, result *Event
	for i := range events {
		switch events[i].Ev {
		case "file":
			file = &events[i]
		case "result":
			result = &events[i]
		case "phase":
			if events[i].Phase == "youtube" {
				t.Fatal("the YouTube fallback ran after a stop")
			}
		}
	}
	if file == nil || *file.I != 0 {
		t.Fatalf("no file event for the finished track: %+v", events)
	}
	if result == nil || !result.Stopped {
		t.Fatalf("the result does not say stopped: %+v", result)
	}
	if _, err := os.Stat(file.Path); err != nil {
		t.Fatalf("the file is not in the output folder: %v", err)
	}
}
