package pipeline

import (
	"bufio"
	"context"
	"encoding/csv"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode"

	"spotseek-app/internal/spotify"
	"spotseek-app/internal/ytdlp"
)

// errSockseekAborted is returned when AbortSockseekOn stops the process.
var errSockseekAborted = errors.New("sockseek aborted")

// normTitle lowercases and removes everything except letters and digits, so a
// track title can be found inside a downloaded file name.
func normTitle(s string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(s) {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			b.WriteRune(r)
		}
	}
	return b.String()
}

// Options controls one download run.
type Options struct {
	Input        string
	Collection   *spotify.Collection // if set, Input is ignored
	OutputDir    string
	WorkDir      string
	ScopeName    string // unique work scope; defaults to the collection name
	Bitrate      string // "320", "256", "192", "128"
	UseFallback  bool
	ProgressJSON bool          // ask sockseek for NDJSON progress events
	SockseekWait time.Duration // time limit for the Soulseek phase; 0 = no limit
	SkipSoulseek bool          // go straight to the fallback
	// AbortSockseekOn stops the sockseek process when a log line matches.
	// The run then treats every track as missing, like a timeout.
	AbortSockseekOn func(string) bool
	MinScore        float64
	Limit           int
	SockseekPath    string
	FfmpegPath      string
	FfprobePath     string
	SoulseekUser    string
	SoulseekPass    string
	Log             func(string)
}

// Result summarizes a finished run.
type Result struct {
	Collection string
	Total      int
	Missing    int
	Files      int
	OutputDir  string
}

type missingTrack struct {
	Artist   string
	Title    string
	Duration int
}

func allMissing(tracks []spotify.Track) []missingTrack {
	out := make([]missingTrack, 0, len(tracks))
	for _, t := range tracks {
		out = append(out, missingTrack{Artist: t.Artist, Title: t.Title, Duration: t.DurationSec})
	}
	return out
}

func (o *Options) log(format string, a ...any) {
	if o.Log != nil {
		o.Log(fmt.Sprintf(format, a...))
	}
}

// Run executes the full pipeline.
func Run(ctx context.Context, o *Options) (*Result, error) {
	if o.WorkDir == "" {
		o.WorkDir = filepath.Join(o.OutputDir, ".work")
	}
	if o.Bitrate == "" {
		o.Bitrate = "320"
	}
	if o.SockseekPath == "" {
		o.SockseekPath = "sockseek"
	}
	if o.FfmpegPath == "" {
		o.FfmpegPath = "ffmpeg"
	}
	if o.FfprobePath == "" {
		o.FfprobePath = "ffprobe"
	}
	if o.MinScore == 0 {
		o.MinScore = 6
	}
	if err := os.MkdirAll(o.OutputDir, 0o755); err != nil {
		return nil, err
	}
	if err := os.MkdirAll(o.WorkDir, 0o755); err != nil {
		return nil, err
	}

	// 1. Spotify metadata
	var coll *spotify.Collection
	if o.Collection != nil {
		coll = o.Collection
	} else {
		o.log("Reading Spotify metadata...")
		c, err := spotify.Extract(o.Input)
		if err != nil {
			return nil, fmt.Errorf("spotify: %w", err)
		}
		coll = c
	}
	tracks := coll.Tracks
	if o.Limit > 0 && len(tracks) > o.Limit {
		tracks = tracks[:o.Limit]
	}
	if len(tracks) == 0 {
		return nil, fmt.Errorf("no tracks found")
	}
	o.log("Collection: %s (%d tracks)", coll.Name, len(tracks))

	safe := ytdlp.SafeName(coll.Name)
	if safe == "" {
		safe = "spotify"
	}
	if o.ScopeName != "" {
		safe = ytdlp.SafeName(o.ScopeName)
		if safe == "" {
			safe = "spotify"
		}
	}

	ssOut := filepath.Join(o.WorkDir, "soulseek")
	ytOut := filepath.Join(o.WorkDir, "youtube")
	var missing []missingTrack
	useAlbumMode := coll.Kind == "album" && coll.Artist != ""

	// The Soulseek phase gets its own time limit. A login problem or a dead
	// network must not block the YouTube fallback forever.
	ssCtx := ctx
	if o.SockseekWait > 0 {
		var ssCancel context.CancelFunc
		ssCtx, ssCancel = context.WithTimeout(ctx, o.SockseekWait)
		defer ssCancel()
	}
	sockseekGaveUp := func(err error) bool {
		if err == nil {
			return false
		}
		if errors.Is(err, errSockseekAborted) {
			return true
		}
		return errors.Is(err, context.DeadlineExceeded) && ctx.Err() == nil
	}

	// 2. Soulseek
	if o.SkipSoulseek {
		o.log("Soulseek is marked as unavailable; using the fallback directly")
		missing = allMissing(tracks)
		if o.UseFallback {
			o.log("Trying YouTube Music for all tracks...")
			fallback(ctx, o, missing, ytOut)
		}
	} else if useAlbumMode {
		// An album is usually shared as one folder. One search finds every track.
		o.log("Album mode: searching for the album folder...")
		n, err := runSockseekAlbum(ssCtx, o, coll.Artist, coll.Name, ssOut)
		if sockseekGaveUp(err) {
			o.log("Soulseek did not answer in %s", o.SockseekWait)
			missing = allMissing(tracks)
			if o.UseFallback {
				o.log("Trying YouTube Music for all tracks...")
				fallback(ctx, o, missing, ytOut)
			}
		} else if err != nil {
			return nil, err
		} else if n > 0 {
			o.log("Album folder found: %d audio files", n)
			var names string
			for _, f := range walkAudio(ssOut) {
				names += normTitle(filepath.Base(f)) + "|"
			}
			for _, t := range tracks {
				if !strings.Contains(names, normTitle(t.Title)) {
					missing = append(missing, missingTrack{Artist: t.Artist, Title: t.Title, Duration: t.DurationSec})
				}
			}
			if len(missing) > 0 {
				o.log("%d tracks are not in the album folder", len(missing))
				if o.UseFallback {
					o.log("Trying YouTube Music for the missing tracks...")
					fallback(ctx, o, missing, ytOut)
				}
			}
		} else {
			o.log("Album folder not found; using per-track search")
			useAlbumMode = false
		}
	}

	if !useAlbumMode && len(missing) == 0 {
		csvPath := filepath.Join(o.WorkDir, safe+".csv")
		if err := writeTracksCSV(csvPath, tracks); err != nil {
			return nil, err
		}
		o.log("Downloading from Soulseek (mp3, prefer %skbps)...", o.Bitrate)
		err := runSockseek(ssCtx, o, csvPath, ssOut)
		if sockseekGaveUp(err) {
			o.log("Soulseek did not answer in %s", o.SockseekWait)
			missing = allMissing(tracks)
		} else if err != nil {
			return nil, err
		} else {
			ssFolder := filepath.Join(ssOut, safe)
			indexPath := filepath.Join(ssFolder, "_index.csv")
			missing = readMissing(indexPath, tracks)
		}
		if len(missing) > 0 {
			o.log("%d tracks missing from Soulseek", len(missing))
		}
		if len(missing) > 0 && o.UseFallback {
			o.log("Trying YouTube Music for the missing tracks...")
			fallback(ctx, o, missing, ytOut)
		}
	}

	// 4. Normalize and merge
	o.log("Normalizing to mp3 %skbps...", o.Bitrate)
	files, err := normalizeAll(ctx, o, []string{ssOut, ytOut}, o.OutputDir)
	if err != nil {
		return nil, err
	}

	res := &Result{
		Collection: coll.Name,
		Total:      len(tracks),
		Missing:    len(missing),
		Files:      files,
		OutputDir:  o.OutputDir,
	}
	o.log("Done. %d files in %s", files, o.OutputDir)
	return res, nil
}

func writeTracksCSV(path string, tracks []spotify.Track) error {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	defer f.Close()
	w := csv.NewWriter(f)
	defer w.Flush()
	if err := w.Write([]string{"Artist", "Song", "Duration"}); err != nil {
		return err
	}
	for _, t := range tracks {
		if err := w.Write([]string{t.Artist, t.Title, strconv.Itoa(t.DurationSec)}); err != nil {
			return err
		}
	}
	return w.Error()
}

func runStream(ctx context.Context, o *Options, args ...string) error {
	cmd := exec.CommandContext(ctx, o.SockseekPath, args...)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	cmd.Stderr = cmd.Stdout
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("cannot start sockseek: %w", err)
	}
	scan := bufio.NewScanner(stdout)
	scan.Buffer(make([]byte, 1024*1024), 1024*1024)
	for scan.Scan() {
		line := scan.Text()
		o.log("%s", line)
		if o.AbortSockseekOn != nil && o.AbortSockseekOn(line) {
			_ = cmd.Process.Kill()
			_ = cmd.Wait()
			return errSockseekAborted
		}
	}
	if err := cmd.Wait(); err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		// Sockseek exits non-zero when some tracks fail. That is normal;
		// the missing tracks are handled by the fallback.
		o.log("sockseek finished with: %v", err)
	}
	return nil
}

func sockseekBaseArgs(o *Options) []string {
	var args []string
	if o.SoulseekUser != "" {
		args = append(args, "--user", o.SoulseekUser)
	}
	if o.SoulseekPass != "" {
		args = append(args, "--pass", o.SoulseekPass)
	}
	return args
}

func sockseekProgressArgs(o *Options) []string {
	if o.ProgressJSON {
		return []string{"--progress-json"}
	}
	return nil
}

func runSockseek(ctx context.Context, o *Options, csvPath, outDir string) error {
	args := []string{csvPath, "-o", outDir, "--format", "mp3", "--pref-min-bitrate", o.Bitrate}
	args = append(args, sockseekProgressArgs(o)...)
	args = append(args, sockseekBaseArgs(o)...)
	return runStream(ctx, o, args...)
}

// runSockseekAlbum searches for the album as one folder and downloads it.
func runSockseekAlbum(ctx context.Context, o *Options, artist, album, outDir string) (int, error) {
	query := album
	if artist != "" {
		query = artist + " - " + album
	}
	args := []string{query, "-o", outDir, "--format", "mp3", "--pref-min-bitrate", o.Bitrate}
	args = append(args, sockseekProgressArgs(o)...)
	args = append(args, sockseekBaseArgs(o)...)
	if err := runStream(ctx, o, args...); err != nil {
		return 0, err
	}
	return len(walkAudio(outDir)), nil
}

func readMissing(indexPath string, tracks []spotify.Track) []missingTrack {
	f, err := os.Open(indexPath)
	if err != nil {
		// No sockseek result file at all (for example a login failure).
		// Treat every track as missing so the fallback can run.
		return allMissing(tracks)
	}
	defer f.Close()
	r := csv.NewReader(f)
	rows, err := r.ReadAll()
	if err != nil || len(rows) < 2 {
		return nil
	}
	header := rows[0]
	col := func(name string) int {
		for i, h := range header {
			if strings.EqualFold(strings.TrimSpace(h), name) {
				return i
			}
		}
		return -1
	}
	fp, ar, ti, le := col("filepath"), col("artist"), col("title"), col("length")
	var out []missingTrack
	for _, row := range rows[1:] {
		get := func(i int) string {
			if i >= 0 && i < len(row) {
				return strings.TrimSpace(row[i])
			}
			return ""
		}
		if get(fp) != "" {
			continue
		}
		title, artist := get(ti), get(ar)
		if title == "" && artist == "" {
			continue
		}
		dur, _ := strconv.Atoi(get(le))
		if dur == 0 {
			for _, t := range tracks {
				if strings.EqualFold(t.Artist, artist) && strings.EqualFold(t.Title, title) {
					dur = t.DurationSec
					break
				}
			}
		}
		out = append(out, missingTrack{Artist: artist, Title: title, Duration: dur})
	}
	return out
}

func fallback(ctx context.Context, o *Options, missing []missingTrack, ytOut string) {
	for i, m := range missing {
		if ctx.Err() != nil {
			return
		}
		o.log("[%d/%d] %s - %s", i+1, len(missing), m.Artist, m.Title)
		cands, err := ytdlp.SearchCandidates(ctx, m.Artist, m.Title, m.Duration, 5)
		if err != nil || len(cands) == 0 {
			o.log("    no candidates")
			continue
		}
		best := cands[0]
		if best.Score < o.MinScore {
			o.log("    best score %.0f < %.0f, skipping", best.Score, o.MinScore)
			continue
		}
		name := ytdlp.SafeName(m.Artist + " - " + m.Title)
		if name == "" {
			name = ytdlp.SafeName(m.Title)
		}
		o.log("    downloading: %s (score %.0f)", best.Title, best.Score)
		if err := ytdlp.Download(ctx, best, ytOut, name, o.Bitrate+"K"); err != nil {
			o.log("    download failed: %v", err)
		}
	}
}

func audioExt(path string) bool {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".mp3", ".flac", ".m4a", ".opus", ".wav", ".ogg", ".aac", ".webm":
		return true
	}
	return false
}

func walkAudio(dir string) []string {
	var out []string
	_ = filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err != nil || info.IsDir() {
			return nil
		}
		if strings.HasSuffix(path, ".incomplete") {
			return nil
		}
		if audioExt(path) {
			out = append(out, path)
		}
		return nil
	})
	return out
}

func ffprobeBitrate(ctx context.Context, o *Options, path string) int {
	cctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, o.FfprobePath, "-v", "error", "-select_streams", "a:0",
		"-show_entries", "stream=bit_rate", "-of", "default=nw=1:nk=1", path)
	out, err := cmd.Output()
	if err != nil {
		return 0
	}
	line := strings.TrimSpace(strings.Split(string(out), "\n")[0])
	br, _ := strconv.Atoi(line)
	if br == 0 {
		return 0
	}
	return br / 1000
}

func uniqueDest(dir, base string) string {
	dest := filepath.Join(dir, base+".mp3")
	i := 1
	for {
		if _, err := os.Stat(dest); os.IsNotExist(err) {
			return dest
		}
		dest = filepath.Join(dir, fmt.Sprintf("%s (%d).mp3", base, i))
		i++
	}
}

func normalizeAll(ctx context.Context, o *Options, sources []string, destDir string) (int, error) {
	if err := os.MkdirAll(destDir, 0o755); err != nil {
		return 0, err
	}
	target, _ := strconv.Atoi(o.Bitrate)
	count := 0
	for _, srcDir := range sources {
		for _, src := range walkAudio(srcDir) {
			if ctx.Err() != nil {
				return count, ctx.Err()
			}
			base := ytdlp.SafeName(strings.TrimSuffix(filepath.Base(src), filepath.Ext(src)))
			if base == "" {
				base = "track"
			}
			dest := uniqueDest(destDir, base)
			if strings.EqualFold(filepath.Ext(src), ".mp3") && ffprobeBitrate(ctx, o, src) == target {
				if err := copyFile(src, dest); err != nil {
					o.log("copy failed: %v", err)
					continue
				}
				count++
				continue
			}
			cctx, cancel := context.WithTimeout(ctx, 10*time.Minute)
			cmd := exec.CommandContext(cctx, o.FfmpegPath, "-y", "-loglevel", "error", "-i", src,
				"-vn", "-map_metadata", "0", "-c:a", "libmp3lame", "-b:a", o.Bitrate+"k", dest)
			err := cmd.Run()
			cancel()
			if err != nil {
				o.log("transcode failed for %s: %v", filepath.Base(src), err)
				continue
			}
			count++
		}
	}
	return count, nil
}

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.Create(dst)
	if err != nil {
		return err
	}
	defer out.Close()
	_, err = out.ReadFrom(in)
	return err
}

// WriteReport saves a small JSON summary next to the output.
func WriteReport(res *Result) error {
	path := filepath.Join(res.OutputDir, "_report.json")
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	defer f.Close()
	enc := json.NewEncoder(f)
	enc.SetIndent("", "  ")
	return enc.Encode(res)
}
