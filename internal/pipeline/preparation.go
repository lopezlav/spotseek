package pipeline

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"spotseek-app/internal/spotify"
)

// PreparedTrack performs metadata/search work only. A drop can reuse the chosen
// Soulseek link, avoiding a second network search. No audio is fetched here.
type PreparedTrack struct {
	Track       spotify.Track
	HasSoulseek bool
	ctx         context.Context
	cancel      context.CancelFunc
	done        chan struct{}
	link        string
	err         error
	youtube     *fallbackSearch
}

func PrepareTrack(ctx context.Context, o *Options, track spotify.Track, searchSoulseek bool) *PreparedTrack {
	ctx, cancel := context.WithTimeout(ctx, 45*time.Second)
	p := &PreparedTrack{Track: track, HasSoulseek: searchSoulseek, ctx: ctx, cancel: cancel, done: make(chan struct{})}
	if o.UseFallback {
		p.youtube = startFallbackSearch(ctx, missingTrack{track.Artist, track.Title, track.DurationSec})
	}
	go func() {
		defer close(p.done)
		if !searchSoulseek {
			return
		}
		if err := os.MkdirAll(o.WorkDir, 0o755); err != nil {
			p.err = err
			return
		}
		csv := filepath.Join(o.WorkDir, "prepare.csv")
		if err := writeTracksCSV(csv, []spotify.Track{track}); err != nil {
			p.err = err
			return
		}
		binary := o.SockseekPath
		if binary == "" {
			binary = "sockseek"
		}
		args := []string{csv, "--format", "mp3", "--pref-min-bitrate", o.Bitrate, "--print", "link", "--no-progress", "-o", o.WorkDir}
		args = append(args, sockseekBaseArgs(o)...)
		out, err := exec.CommandContext(ctx, binary, args...).CombinedOutput()
		for _, line := range strings.Split(string(out), "\n") {
			line = strings.TrimSpace(line)
			if strings.HasPrefix(line, "slsk://") {
				p.link = line
				return
			}
		}
		if ctx.Err() != nil {
			p.err = ctx.Err()
			return
		}
		// A normal empty Soulseek search exits non-zero. Keep that negative
		// result, but retry through the normal pipeline for startup/login errors.
		if err != nil && !strings.Contains(string(out), "NoSearchResults") && !strings.Contains(string(out), "No suitable") && !strings.Contains(string(out), "No results") {
			p.err = fmt.Errorf("prepared Soulseek search: %w", err)
		}
	}()
	return p
}

func (p *PreparedTrack) soulseekLink(ctx context.Context) (string, error) {
	select {
	case <-ctx.Done():
		return "", ctx.Err()
	case <-p.done:
		return p.link, p.err
	}
}

func (p *PreparedTrack) Stop() {
	p.cancel()
	<-p.done
	if p.youtube != nil {
		p.youtube.stop()
	}
}
