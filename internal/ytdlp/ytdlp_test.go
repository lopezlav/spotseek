package ytdlp

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"spotseek-app/internal/timing"
)

func TestDownloadInstrumentationPreservesRetries(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("fixture uses a shell executable")
	}
	dir := t.TempDir()
	script := filepath.Join(dir, "yt-dlp")
	// Fail the default client, succeed with the second configured client.
	err := os.WriteFile(script, []byte(`#!/bin/sh
case "$*" in
  *default,web_safari*) ;;
  *) echo 'temporary extractor failure' >&2; exit 1 ;;
esac
echo '[download] Destination: song.webm'
echo '[ExtractAudio] Destination: song.mp3'
echo '[Metadata] Adding metadata'
echo '[ThumbnailsConvertor] Converting thumbnail'
echo '[EmbedThumbnail] Adding thumbnail'
`), 0o755)
	if err != nil {
		t.Fatal(err)
	}
	old := Binary
	Binary = script
	t.Cleanup(func() { Binary = old })
	var events []timing.Event
	ctx := timing.WithObserver(context.Background(), func(e timing.Event) { events = append(events, e) })
	if err := Download(ctx, Candidate{URL: "https://example.test/song"}, dir, "song", "320K"); err != nil {
		t.Fatal(err)
	}
	attempts, failed, transfers, encodes := 0, 0, 0, 0
	for _, e := range events {
		switch e.Stage {
		case "youtube.download_attempt":
			attempts++
			if e.Error != "" {
				failed++
			}
		case "youtube.audio_transfer":
			transfers++
		case "youtube.mp3_encode":
			encodes++
		}
	}
	if attempts != 2 || failed != 1 || transfers != 1 || encodes != 1 {
		t.Fatalf("wrong retry/phase events: %+v", events)
	}
	if last := events[len(events)-1]; last.Stage != "youtube.download_total" || last.Error != "" {
		t.Fatalf("wrong total: %+v", last)
	}
}

func TestCanceledDownloadRecordsFailure(t *testing.T) {
	var events []timing.Event
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	ctx = timing.WithObserver(ctx, func(e timing.Event) { events = append(events, e) })
	if err := Download(ctx, Candidate{}, t.TempDir(), "song", "320K"); err == nil {
		t.Fatal("canceled download succeeded")
	}
	if len(events) == 0 || events[len(events)-1].Error == "" {
		t.Fatalf("failure not recorded: %+v", events)
	}
}
