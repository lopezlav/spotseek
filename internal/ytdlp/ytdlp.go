package ytdlp

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"
	"unicode"

	"spotseek-app/internal/timing"
)

// Candidate is one YouTube search result with a relevance score.
type Candidate struct {
	Title    string
	Artist   string
	Album    string
	Duration int
	ID       string
	URL      string
	Score    float64
}

var badTerms = []string{
	"lyrics", "lyric", "letra", "subtitulad", "subtitulado", "traduccion",
	"reaction", "review", "tutorial", "cover", "karaoke", "instrumental",
	"remix", "sped up", "slowed", "nightcore", "8d", "live", "en vivo",
	"mashup", "type beat", "full album", "backstage",
}

var badVideoRe = regexp.MustCompile(`\b(official (music )?video|videoclip|video oficial)\b`)

var nonAlnum = regexp.MustCompile(`[^a-z0-9 ]+`)
var spaces = regexp.MustCompile(`\s+`)

func norm(s string) string {
	s = strings.Map(func(r rune) rune {
		if unicode.Is(unicode.Mn, r) {
			return -1
		}
		return unicode.ToLower(r)
	}, s)
	s = nonAlnum.ReplaceAllString(s, " ")
	return strings.TrimSpace(spaces.ReplaceAllString(s, " "))
}

func isBadVideo(videoTitle, wantTitle string) bool {
	t := norm(videoTitle)
	want := norm(wantTitle)
	for _, term := range badTerms {
		pat := regexp.MustCompile(`\b` + regexp.QuoteMeta(term) + `\b`)
		if pat.MatchString(t) && !pat.MatchString(want) {
			return true
		}
	}
	return badVideoRe.MatchString(t) && !strings.Contains(want, "official")
}

func score(c Candidate, title, artist string, length int) float64 {
	var s float64
	vt := norm(c.Title)
	va := norm(c.Artist)
	nt := norm(title)
	na := norm(artist)

	if nt != "" {
		if strings.Contains(vt, nt) {
			s += 4
		} else {
			all := true
			for _, w := range strings.Fields(nt) {
				if len([]rune(w)) > 2 && !strings.Contains(vt, w) {
					all = false
					break
				}
			}
			if all && len(strings.Fields(nt)) > 0 {
				s += 2
			}
		}
	}
	if na != "" && strings.Contains(va, na) {
		s += 3
	}
	if c.Album != "" {
		s += 1
	}
	if length > 0 && c.Duration > 0 {
		diff := c.Duration - length
		if diff < 0 {
			diff = -diff
		}
		switch {
		case diff <= 3:
			s += 4
		case diff <= 10:
			s += 2
		case diff > 45:
			s -= 4
		}
	}
	if isBadVideo(c.Title, title) {
		s -= 8
	}
	return s
}

// Binary is the yt-dlp executable. Override it to use a custom path.
var Binary = "yt-dlp"

func runYtDlp(ctx context.Context, timeout time.Duration, args ...string) (string, error) {
	select {
	case searchProcessSlots <- struct{}{}:
		defer func() { <-searchProcessSlots }()
	case <-ctx.Done():
		return "", ctx.Err()
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	full := append([]string{"--no-warnings", "--ignore-config"}, args...)
	cmd := exec.CommandContext(ctx, Binary, full...)
	out, err := cmd.Output()
	if ctx.Err() != nil {
		return "", ctx.Err()
	}
	var exitErr *exec.ExitError
	if errors.As(err, &exitErr) && len(exitErr.Stderr) > 0 {
		err = fmt.Errorf("yt-dlp: %w: %s", err, strings.TrimSpace(string(exitErr.Stderr)))
	}
	return string(out), err
}

// SafeName turns a string into a safe file name.
func SafeName(s string) string {
	re := regexp.MustCompile(`[\\/:*?"<>|]+`)
	s = re.ReplaceAllString(s, " ")
	return strings.TrimSpace(spaces.ReplaceAllString(s, " "))
}

// Download fetches one candidate as an mp3 file.
func Download(ctx context.Context, c Candidate, outdir, filename, quality string) (retErr error) {
	finish := timing.Begin(ctx, "youtube.download_total", filename)
	defer func() { finish(retErr) }()
	if err := os.MkdirAll(outdir, 0o755); err != nil {
		return err
	}
	template := filepath.Join(outdir, filename+".%(ext)s")
	attempts := [][]string{
		{},
		{"--extractor-args", "youtube:player_client=default,web_safari"},
		{"--extractor-args", "youtube:player_client=web_safari,tv"},
	}
	var lastErr error
	for i, extra := range attempts {
		args := []string{"--no-warnings", "--ignore-config",
			"-f", "bestaudio/best",
			"-x", "--audio-format", "mp3", "--audio-quality", quality,
			"--embed-metadata", "--embed-thumbnail",
			"-o", template}
		args = append(args, extra...)
		args = append(args, c.URL)
		end := timing.Begin(ctx, "youtube.download_attempt", fmt.Sprintf("%s; attempt %d", filename, i+1))
		out, err := downloadOutput(ctx, args, filename, i+1)
		end(err)
		if err == nil {
			return nil
		}
		lastErr = fmt.Errorf("yt-dlp failed: %v: %s", err, string(out))
		if ctx.Err() != nil {
			return ctx.Err()
		}
	}
	return lastErr
}

// yt-dlp's emitted markers separate extraction, audio transfer, encoding, and
// tagging. These are observed process-output boundaries, not network traces.
func downloadOutput(ctx context.Context, args []string, filename string, attempt int) ([]byte, error) {
	if !timing.Enabled(ctx) {
		return exec.CommandContext(ctx, Binary, args...).CombinedOutput()
	}
	args = append([]string{"--newline"}, args...)
	cmd := exec.CommandContext(ctx, Binary, args...)
	pipe, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	cmd.Stderr = cmd.Stdout
	start := time.Now()
	stage := "youtube.extract_before_transfer"
	detail := fmt.Sprintf("%s; attempt %d", filename, attempt)
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	var output strings.Builder
	scan := bufio.NewScanner(pipe)
	scan.Buffer(make([]byte, 65536), 1024*1024)
	transition := func(next string) {
		if stage == next {
			return
		}
		timing.Interval(ctx, stage, detail, start, nil)
		stage, start = next, time.Now()
	}
	for scan.Scan() {
		line := scan.Text()
		output.WriteString(line + "\n")
		switch {
		case strings.HasPrefix(line, "[download] Destination:"):
			transition("youtube.audio_transfer")
		case strings.HasPrefix(line, "[ExtractAudio]"):
			transition("youtube.mp3_encode")
		case strings.HasPrefix(line, "[Metadata]"):
			transition("youtube.metadata_embed")
		case strings.HasPrefix(line, "[ThumbnailsConvertor]"):
			transition("youtube.thumbnail_convert")
		case strings.HasPrefix(line, "[EmbedThumbnail]"):
			transition("youtube.thumbnail_embed")
		}
	}
	scanErr := scan.Err()
	if scanErr != nil {
		_ = cmd.Process.Kill()
	}
	err = cmd.Wait()
	if scanErr != nil {
		err = scanErr
	}
	timing.Interval(ctx, stage, detail, start, err)
	return []byte(output.String()), err
}
