package pipeline

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"spotseek-app/internal/spotify"
)

func TestProgressivePlaybackGrowsBeforeDownloadFinishes(t *testing.T) {
	ffmpeg, err := exec.LookPath("ffmpeg")
	if err != nil {
		t.Skip("ffmpeg not installed")
	}
	dir := t.TempDir()
	original := filepath.Join(dir, "fixture.mp3")
	if output, err := exec.Command(ffmpeg, "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=6", "-c:a", "libmp3lame", "-b:a", "320k", original).CombinedOutput(); err != nil {
		t.Fatalf("fixture: %v %s", err, output)
	}
	data, err := os.ReadFile(original)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	playback, err := StartProgressivePlayback(ctx, filepath.Join(dir, "work"), spotify.Track{Title: "Test", DurationSec: 6}, ffmpeg, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(playback.Cancel)
	sourceDir := filepath.Join(dir, "work", "soulseek")
	os.MkdirAll(sourceDir, 0o755)
	partial := filepath.Join(sourceDir, "test.mp3.incomplete")
	f, err := os.Create(partial)
	if err != nil {
		t.Fatal(err)
	}
	f.Write(data[:len(data)/2])
	f.Sync()
	deadline := time.Now().Add(4 * time.Second)
	var status PlaybackStatus
	for {
		encoded, _ := os.ReadFile(playback.Path)
		json.Unmarshal(encoded, &status)
		if status.BufferedFrames > 48000 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("no decoded prefix before completion: %+v", status)
		}
		time.Sleep(30 * time.Millisecond)
	}
	if status.Complete {
		t.Fatal("incomplete download was marked complete")
	}
	prefix := status.BufferedFrames
	f.Write(data[len(data)/2:])
	f.Close()
	if err := os.Rename(partial, filepath.Join(sourceDir, "test.mp3")); err != nil {
		t.Fatal(err)
	}
	playback.Finish()
	encoded, _ := os.ReadFile(playback.Path)
	json.Unmarshal(encoded, &status)
	if !status.Complete || status.Error != "" || status.BufferedFrames <= prefix || status.BufferedFrames < 5*48000 {
		t.Fatalf("completion did not extend same stream: %+v", status)
	}
}

func TestProgressiveCancellationKeepsDescriptorAndEnds(t *testing.T) {
	p, err := StartProgressivePlayback(context.Background(), t.TempDir(), spotify.Track{Title: "Test", DurationSec: 180}, "ffmpeg", nil)
	if err != nil {
		t.Fatal(err)
	}
	p.Cancel()
	data, err := os.ReadFile(p.Path)
	if err != nil {
		t.Fatal(err)
	}
	var status PlaybackStatus
	if err := json.Unmarshal(data, &status); err != nil {
		t.Fatal(err)
	}
	if status.Error == "" || status.Complete {
		t.Fatalf("cancellation lost: %+v", status)
	}
}
