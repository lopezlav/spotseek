package pipeline

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"spotseek-app/internal/spotify"
)

// PlaybackStatus describes the append-only PCM buffer shared with Mixxx.
type PlaybackStatus struct {
	Version        int           `json:"version"`
	PCMPath        string        `json:"pcm_path"`
	SampleRate     int           `json:"sample_rate"`
	Channels       int           `json:"channels"`
	TotalFrames    int64         `json:"total_frames"`
	BufferedFrames int64         `json:"buffered_frames"`
	Complete       bool          `json:"complete"`
	Error          string        `json:"error,omitempty"`
	Track          spotify.Track `json:"track"`
}

// ProgressivePlayback tails one Soulseek transfer into ffmpeg's streaming MP3
// decoder. The deck reads a separate PCM file which is never renamed/reencoded.
type ProgressivePlayback struct {
	Path       string
	status     PlaybackStatus
	ctx        context.Context
	cancel     context.CancelFunc
	finished   chan struct{}
	done       chan struct{}
	onUpdate   func(string, PlaybackStatus)
	finishOnce sync.Once
}

func StartProgressivePlayback(ctx context.Context, workDir string, track spotify.Track, ffmpeg string, update func(string, PlaybackStatus)) (*ProgressivePlayback, error) {
	if track.DurationSec <= 0 || track.DurationSec > 24*60*60 {
		return nil, fmt.Errorf("progressive playback needs a valid duration")
	}
	if err := os.MkdirAll(workDir, 0o755); err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(ctx)
	p := &ProgressivePlayback{Path: filepath.Join(workDir, "playback.mixxx-stream"), ctx: ctx, cancel: cancel, finished: make(chan struct{}), done: make(chan struct{}), onUpdate: update}
	p.status = PlaybackStatus{Version: 1, PCMPath: filepath.Join(workDir, "playback.f32"), SampleRate: 48000, Channels: 2, TotalFrames: int64(track.DurationSec+5) * 48000, Track: track}
	f, err := os.Create(p.status.PCMPath)
	if err != nil {
		cancel()
		return nil, err
	}
	f.Close()
	if err := p.publish(); err != nil {
		cancel()
		return nil, err
	}
	if ffmpeg == "" {
		ffmpeg = "ffmpeg"
	}
	go p.run(workDir, ffmpeg)
	return p, nil
}

func (p *ProgressivePlayback) publish() error {
	if info, err := os.Stat(p.status.PCMPath); err == nil {
		p.status.BufferedFrames = info.Size() / int64(p.status.Channels*4)
	}
	if p.status.Complete {
		p.status.TotalFrames = p.status.BufferedFrames
	}
	data, err := json.Marshal(p.status)
	if err != nil {
		return err
	}
	tmp := p.Path + ".new"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	if err := os.Rename(tmp, p.Path); err != nil {
		return err
	}
	if p.onUpdate != nil {
		p.onUpdate(p.Path, p.status)
	}
	return nil
}

func (p *ProgressivePlayback) Finish() {
	p.finishOnce.Do(func() { close(p.finished) })
	<-p.done
}

func (p *ProgressivePlayback) Cancel() {
	p.cancel()
	<-p.done
}

func (p *ProgressivePlayback) run(workDir, ffmpeg string) {
	defer close(p.done)
	defer p.cancel()
	var source *os.File
	tick := time.NewTicker(50 * time.Millisecond)
	defer tick.Stop()
	for source == nil {
		paths := walkMP3Transfers(filepath.Join(workDir, "soulseek"))
		for _, path := range paths {
			source, _ = os.Open(path)
			if source != nil {
				break
			}
		}
		if source != nil {
			break
		}
		select {
		case <-p.ctx.Done():
			p.status.Error = p.ctx.Err().Error()
			_ = p.publish()
			return
		case <-p.finished:
			// YouTube fallback is finalized normally. Keep this placeholder
			// unavailable so the deck loader can use the completed MP3.
			p.status.Error = "no Soulseek audio was available"
			_ = p.publish()
			return
		case <-tick.C:
		}
	}
	defer source.Close()
	cmd := exec.CommandContext(p.ctx, ffmpeg, "-hide_banner", "-loglevel", "error", "-probesize", "65536", "-analyzeduration", "100000", "-f", "mp3", "-i", "pipe:0", "-vn", "-ar", "48000", "-ac", "2", "-f", "f32le", "-y", p.status.PCMPath)
	input, err := cmd.StdinPipe()
	if err != nil {
		p.status.Error = err.Error()
		_ = p.publish()
		return
	}
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	if err := cmd.Start(); err != nil {
		input.Close()
		p.status.Error = err.Error()
		_ = p.publish()
		return
	}
	feedDone := make(chan error, 1)
	go func() { feedDone <- p.feed(source, input); input.Close() }()
	processDone := make(chan error, 1)
	go func() { processDone <- cmd.Wait() }()
	for {
		select {
		case err := <-processDone:
			input.Close()
			p.cancel()
			feedErr := <-feedDone
			if err != nil {
				p.status.Error = fmt.Sprintf("progressive decoder: %v: %s", err, strings.TrimSpace(stderr.String()))
			} else if feedErr != nil {
				p.status.Error = feedErr.Error()
			} else {
				p.status.Complete = true
			}
			_ = p.publish()
			return
		case <-tick.C:
			_ = p.publish()
		}
	}
}

func (p *ProgressivePlayback) feed(source *os.File, dest io.Writer) error {
	buf := make([]byte, 64*1024)
	var offset int64
	for {
		// Retries may delete or replace the peer's partial file. Never mark an
		// old prefix complete merely because a different attempt succeeded.
		originalInfo, statErr := source.Stat()
		if statErr != nil {
			return statErr
		}
		path := source.Name()
		currentInfo, pathErr := os.Stat(path)
		if os.IsNotExist(pathErr) && strings.HasSuffix(path, ".incomplete") {
			currentInfo, pathErr = os.Stat(strings.TrimSuffix(path, ".incomplete"))
		}
		if pathErr != nil || !os.SameFile(originalInfo, currentInfo) {
			return fmt.Errorf("Soulseek transfer file was replaced or removed")
		}
		if info, err := source.Stat(); err == nil && info.Size() < offset {
			return fmt.Errorf("Soulseek transfer was restarted")
		}
		n, err := source.Read(buf)
		if n > 0 {
			if _, writeErr := dest.Write(buf[:n]); writeErr != nil {
				return writeErr
			}
			offset += int64(n)
		}
		if err != nil && err != io.EOF {
			return err
		}
		if n > 0 {
			continue
		}
		select {
		case <-p.ctx.Done():
			return p.ctx.Err()
		case <-p.finished:
			// Drain bytes which arrived between the final read and completion.
			_, err := io.Copy(dest, source)
			return err
		case <-time.After(30 * time.Millisecond):
		}
	}
}

func walkMP3Transfers(dir string) []string {
	var paths []string
	_ = filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() && (strings.HasSuffix(strings.ToLower(path), ".mp3.incomplete") || strings.HasSuffix(strings.ToLower(path), ".mp3")) {
			paths = append(paths, path)
		}
		return nil
	})
	return paths
}
