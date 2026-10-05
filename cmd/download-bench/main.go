// download-bench runs the real Spotseek pipeline into fresh, isolated folders.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"spotseek-app/internal/pipeline"
	"spotseek-app/internal/server"
	"spotseek-app/internal/spotify"
	"spotseek-app/internal/timing"
	"spotseek-app/internal/ytdlp"
)

type caseResult struct {
	Track     spotify.Track    `json:"track"`
	Mode      string           `json:"mode"`
	Run       int              `json:"run"`
	Source    string           `json:"source"`
	ElapsedMS float64          `json:"elapsed_ms"`
	Success   bool             `json:"success"`
	Error     string           `json:"error,omitempty"`
	Result    *pipeline.Result `json:"result,omitempty"`
	Timings   []timing.Event   `json:"timings"`
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func writeJSON(path string, value any) error {
	data, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, append(data, '\n'), 0o600)
}

func run() error {
	manifest := flag.String("tracks", "", "JSON array of Spotify tracks")
	output := flag.String("out", "", "new benchmark directory (must not exist)")
	mode := flag.String("mode", "auto", "auto, direct-youtube, or both")
	repeat := flag.Int("repeat", 1, "runs per track and mode")
	wait := flag.Duration("soulseek-wait", 0, "override configured Soulseek time limit")
	timeout := flag.Duration("timeout", 10*time.Minute, "overall limit per track")
	fetchMetadata := flag.Bool("fetch-metadata", false, "read the public Spotify track page instead of using supplied metadata")
	flag.Parse()
	if *manifest == "" || *output == "" || *repeat < 1 {
		return fmt.Errorf("--tracks and --out are required; --repeat must be positive")
	}
	modes := []string{*mode}
	if *mode == "both" {
		modes = []string{"auto", "direct-youtube"}
	}
	if *mode != "both" && *mode != "auto" && *mode != "direct-youtube" {
		return fmt.Errorf("invalid mode %q", *mode)
	}
	data, err := os.ReadFile(*manifest)
	if err != nil {
		return err
	}
	var tracks []spotify.Track
	if err := json.Unmarshal(data, &tracks); err != nil {
		return err
	}
	if len(tracks) == 0 {
		return fmt.Errorf("empty manifest")
	}
	settings, err := server.LoadSettings()
	if err != nil {
		return err
	}
	if settings.YtdlpPath != "" {
		ytdlp.Binary = settings.YtdlpPath
	}
	if *wait <= 0 {
		*wait = time.Duration(settings.SockseekWaitSec) * time.Second
	}
	// Refuse reused folders so cached/indexed files cannot become fast downloads.
	if err := os.Mkdir(*output, 0o755); err != nil {
		return err
	}
	if err := writeJSON(filepath.Join(*output, "tracks.json"), tracks); err != nil {
		return err
	}
	if err := writeJSON(filepath.Join(*output, "parameters.json"), map[string]any{
		"started_at": time.Now(), "modes": modes, "repeat": *repeat, "concurrency": 1,
		"bitrate": settings.Bitrate, "soulseek_wait": wait.String(), "timeout": timeout.String(),
		"metadata_from_manifest": !*fetchMetadata, "min_score": settings.MinScore,
	}); err != nil {
		return err
	}
	var results []caseResult
	for _, mode := range modes {
		for iteration := 1; iteration <= *repeat; iteration++ {
			for i, track := range tracks {
				dir := filepath.Join(*output, fmt.Sprintf("%s-%02d-%02d", mode, iteration, i+1))
				if err := os.Mkdir(dir, 0o755); err != nil {
					return err
				}
				logFile, err := os.Create(filepath.Join(dir, "timeline.log"))
				if err != nil {
					return err
				}
				cr := caseResult{Track: track, Mode: mode, Run: iteration}
				fmt.Printf("START %s: %s - %s (%ds)\n", mode, track.Artist, track.Title, track.DurationSec)
				ctx, cancel := context.WithTimeout(context.Background(), *timeout)
				opts := &pipeline.Options{
					Collection: &spotify.Collection{Kind: "track", Name: track.Artist + " - " + track.Title, Tracks: []spotify.Track{track}},
					OutputDir:  filepath.Join(dir, "audio"), WorkDir: filepath.Join(dir, "work"),
					Bitrate: settings.Bitrate, UseFallback: settings.Fallback || mode == "direct-youtube", ProgressJSON: true,
					SockseekWait: *wait, SkipSoulseek: mode == "direct-youtube", MinScore: settings.MinScore,
					SockseekPath: settings.SockseekPath, FfmpegPath: settings.FfmpegPath, FfprobePath: settings.FfprobePath,
					SoulseekUser: settings.SoulseekUser, SoulseekPass: settings.SoulseekPass,
					Log: func(line string) { fmt.Fprintf(logFile, "%s %s\n", time.Now().Format(time.RFC3339Nano), line) },
					OnTiming: func(e timing.Event) {
						cr.Timings = append(cr.Timings, e)
						fmt.Printf("  %-37s %8.3fs %s\n", e.Stage, e.DurationMS/1000, e.Detail)
					},
				}
				if settings.Fallback {
					opts.AbortSockseekOn = func(line string) bool {
						return strings.Contains(line, "Failed to ensure Soulseek connection and login")
					}
				}
				if *fetchMetadata {
					opts.Collection = nil
					opts.Input = "https://open.spotify.com/track/" + track.ID
				}
				started := time.Now()
				cr.Result, err = pipeline.Run(ctx, opts)
				cr.ElapsedMS = float64(time.Since(started)) / float64(time.Millisecond)
				cancel()
				cr.Success = err == nil && cr.Result != nil && cr.Result.Files > 0
				if err != nil {
					cr.Error = err.Error()
				}
				cr.Source = "none"
				if cr.Success {
					cr.Source = "soulseek"
					if cr.Result.Missing > 0 {
						cr.Source = "youtube"
					}
				}
				logFile.Close()
				if err := writeJSON(filepath.Join(dir, "result.json"), cr); err != nil {
					return err
				}
				results = append(results, cr)
				if err := writeJSON(filepath.Join(*output, "results.json"), results); err != nil {
					return err
				}
				fmt.Printf("END source=%s success=%t elapsed=%.3fs\n", cr.Source, cr.Success, cr.ElapsedMS/1000)
			}
		}
	}
	return nil
}
