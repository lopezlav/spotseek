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
	"sync"
	"time"
	"unicode"

	"spotseek-app/internal/spotify"
	"spotseek-app/internal/timing"
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
	MeasureTimings  bool
	OnTiming        func(timing.Event)
	Preparation     *PreparedTrack
	// Event, if set, receives structured progress events (see events.go).
	Event func(Event)
	// Select, if set, is called after the collection event with the number
	// of tracks. It returns the indexes to download (nil: all of them), so a
	// front end can skip tracks the user already has. Events keep the
	// indexes of the whole collection.
	Select func(ctx context.Context, n int) ([]int, error)
	logMu  sync.Mutex
	track  *tracker
}

// Result summarizes a finished run.
type Result struct {
	Collection string
	Total      int
	Missing    int
	Files      int
	OutputDir  string
	Timings    []timing.Event `json:",omitempty"`
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
	line := fmt.Sprintf(format, a...)
	if o.Log != nil {
		o.logMu.Lock()
		o.Log(line)
		o.logMu.Unlock()
	}
	// JSON lines are for the Fyne window and the server; front ends that
	// use events get the same facts as events.
	if !strings.HasPrefix(line, "{") {
		o.emit(Event{Ev: "log", Line: line})
	}
}

// baseTitle removes the parts of a Spotify title that file names often leave
// out: "(feat. X)", "[Remastered]", and " - Remix" style suffixes. It keeps the
// full title when nothing would be left.
func baseTitle(s string) string {
	cut := len(s)
	for _, sep := range []string{" (", " [", " - "} {
		if i := strings.Index(s, sep); i > 0 && i < cut {
			cut = i
		}
	}
	if b := strings.TrimSpace(s[:cut]); b != "" {
		return b
	}
	return s
}

// Run executes the full pipeline.
func Run(ctx context.Context, o *Options) (result *Result, runErr error) {
	var events []timing.Event
	if o.MeasureTimings || o.OnTiming != nil {
		ctx = timing.WithObserver(ctx, func(e timing.Event) {
			events = append(events, e)
			if o.OnTiming != nil {
				o.OnTiming(e)
			}
			data, _ := json.Marshal(e)
			o.log("TIMING %s", data)
		})
	}
	finish := timing.Begin(ctx, "pipeline.total", o.ScopeName)
	defer func() {
		finish(runErr)
		if result != nil {
			result.Timings = events
		}
	}()
	setup := timing.Begin(ctx, "pipeline.setup", "")
	if o.WorkDir == "" {
		// Each run gets its own work folder. A shared one made every run
		// convert (and copy again) all the files of the runs before it.
		o.WorkDir = filepath.Join(o.OutputDir, ".work", "run-"+time.Now().Format("20060102-150405.000"))
		defer func() {
			// Keep the files of a stopped or failed run, as the front ends say.
			if runErr == nil || len(walkAudio(o.WorkDir)) == 0 {
				_ = os.RemoveAll(o.WorkDir)
			}
		}()
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
		setup(err)
		return nil, err
	}
	if err := os.MkdirAll(o.WorkDir, 0o755); err != nil {
		setup(err)
		return nil, err
	}
	setup(nil)

	// 1. Spotify metadata
	var coll *spotify.Collection
	metadata := timing.Begin(ctx, "spotify.metadata", "")
	if o.Collection != nil {
		coll = o.Collection
	} else {
		o.emit(Event{Ev: "phase", Phase: "spotify"})
		o.log("Reading Spotify metadata...")
		c, err := spotify.Extract(o.Input)
		if err != nil {
			metadata(err)
			return nil, fmt.Errorf("spotify: %w", err)
		}
		coll = c
	}
	metadata(nil)
	tracks := coll.Tracks
	if o.Limit > 0 && len(tracks) > o.Limit {
		tracks = tracks[:o.Limit]
	}
	if len(tracks) == 0 {
		return nil, fmt.Errorf("no tracks found")
	}
	o.log("Collection: %s (%d tracks)", coll.Name, len(tracks))
	if o.Event != nil {
		ce := Event{Ev: "collection", Name: coll.Name, Kind: coll.Kind, Artist: coll.Artist, Cover: coll.Cover}
		for _, t := range tracks {
			ce.Tracks = append(ce.Tracks, TrackEvent{Artist: t.Artist, Title: t.Title, Duration: t.DurationSec, Album: t.Album, ID: t.ID})
		}
		o.emit(ce)
		o.track = newTracker(o, tracks)
	}
	all := tracks
	if o.Select != nil {
		only, err := o.Select(ctx, len(tracks))
		if err != nil {
			return nil, err
		}
		if only != nil {
			var sel []spotify.Track
			seen := map[int]bool{}
			for _, i := range only {
				if i >= 0 && i < len(all) && !seen[i] {
					seen[i] = true
					sel = append(sel, all[i])
				}
			}
			tracks = sel
		}
	}
	if len(tracks) == 0 {
		// Every track was skipped: nothing to download.
		o.emit(Event{Ev: "phase", Phase: "done"})
		o.emit(Event{Ev: "result", Files: 0, Count: 0, Dir: o.OutputDir, Name: coll.Name})
		return &Result{Collection: coll.Name, OutputDir: o.OutputDir}, nil
	}
	o.emit(Event{Ev: "phase", Phase: "soulseek"})

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
	// The album folder holds every track, so album mode only fits a run of
	// the whole album.
	useAlbumMode := coll.Kind == "album" && coll.Artist != "" && len(tracks) == len(all)
	// Server jobs contain one track. Prepare a fallback candidate while the
	// Soulseek process searches/transfers, but never download speculative audio.
	// Multi-track CLI/album runs stay lazy to avoid searching every song twice.
	var prepared *fallbackSearch
	preparation := o.Preparation
	if preparation != nil && (len(tracks) != 1 || preparation.Track != tracks[0]) {
		preparation.Stop()
		preparation = nil
	}
	if preparation != nil {
		prepared = preparation.youtube
		defer preparation.Stop()
	} else if o.UseFallback && !o.SkipSoulseek && !useAlbumMode && len(tracks) == 1 {
		prepared = startFallbackSearch(ctx, allMissing(tracks)[0])
		defer prepared.stop()
	}
	soulseekHandled := false

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
	if preparation != nil && preparation.HasSoulseek && !o.SkipSoulseek {
		end := timing.Begin(ctx, "soulseek.wait_for_prepared_search", "")
		link, err := preparation.soulseekLink(ssCtx)
		end(err)
		if err == nil {
			soulseekHandled = true
			if link != "" {
				o.log("Downloading the Soulseek match found while dragging...")
				args := []string{link, "-o", ssOut, "--format", "mp3"}
				args = append(args, sockseekProgressArgs(o)...)
				args = append(args, sockseekBaseArgs(o)...)
				err = runStream(ssCtx, o, args...)
			}
			if err != nil || len(walkAudio(ssOut)) == 0 {
				missing = allMissing(tracks)
			}
			if len(missing) > 0 && o.UseFallback {
				fallback(ctx, o, missing, ytOut, prepared)
			}
		}
	}
	if soulseekHandled {
		// The prepared result already completed the source/fallback decision.
	} else if o.SkipSoulseek {
		o.log("Soulseek is marked as unavailable; using the fallback directly")
		missing = allMissing(tracks)
		if o.UseFallback {
			o.log("Trying YouTube Music for all tracks...")
			fallback(ctx, o, missing, ytOut, prepared)
		}
	} else if useAlbumMode {
		// An album is usually shared as one folder. One search finds every track.
		o.log("Album mode: searching for the album folder...")
		if o.track != nil {
			o.track.mu.Lock()
			o.track.album = true
			o.track.mu.Unlock()
		}
		n, err := runSockseekAlbum(ssCtx, o, coll.Artist, coll.Name, ssOut, len(tracks))
		if o.track != nil {
			o.track.mu.Lock()
			o.track.album = false
			o.track.mu.Unlock()
		}
		if sockseekGaveUp(err) {
			o.log("Soulseek did not answer in %s", o.SockseekWait)
			missing = allMissing(tracks)
			if o.UseFallback {
				o.log("Trying YouTube Music for all tracks...")
				fallback(ctx, o, missing, ytOut, prepared)
			}
		} else if err != nil {
			return nil, err
		} else if n > 0 {
			o.log("Album folder found: %d audio files", n)
			missing = albumMissing(o, tracks, walkAudio(ssOut))
			o.markAlbum(missing)
			if len(missing) > 0 {
				o.log("%d tracks are not in the album folder", len(missing))
				// Search Soulseek for each of them before YouTube: other
				// peers often have the tracks this folder is missing.
				missing = o.searchTracks(ssCtx, missing, safe+" (tracks)", ssOut)
				if len(missing) > 0 && o.UseFallback {
					o.log("Trying YouTube Music for the missing tracks...")
					fallback(ctx, o, missing, ytOut, prepared)
				}
			}
		} else {
			o.log("Album folder not found; using per-track search")
			useAlbumMode = false
		}
	}

	if !soulseekHandled && !useAlbumMode && len(missing) == 0 {
		csvPath := filepath.Join(o.WorkDir, safe+".csv")
		end := timing.Begin(ctx, "soulseek.csv_write", "")
		err := writeTracksCSV(csvPath, tracks)
		end(err)
		if err != nil {
			return nil, err
		}
		o.log("Downloading from Soulseek (mp3, prefer %skbps)...", o.Bitrate)
		err = runSockseek(ssCtx, o, csvPath, ssOut)
		if sockseekGaveUp(err) {
			o.log("Soulseek did not answer in %s", o.SockseekWait)
			missing = allMissing(tracks)
		} else if err != nil {
			return nil, err
		} else {
			ssFolder := filepath.Join(ssOut, safe)
			indexPath := filepath.Join(ssFolder, "_index.csv")
			end = timing.Begin(ctx, "soulseek.missing_check", "")
			missing = readMissing(indexPath, tracks)
			end(nil)
		}
		if len(missing) > 0 {
			o.log("%d tracks missing from Soulseek", len(missing))
		}
		if len(missing) > 0 && o.UseFallback {
			o.log("Trying YouTube Music for the missing tracks...")
			fallback(ctx, o, missing, ytOut, prepared)
		}
	}
	if prepared != nil {
		prepared.stop()
	}
	if !o.UseFallback {
		for _, m := range missing {
			o.trackState(m, StateMissing, Event{})
		}
	}

	// 4. Normalize and merge
	o.emit(Event{Ev: "phase", Phase: "convert"})
	o.log("Normalizing to mp3 %skbps...", o.Bitrate)
	end := timing.Begin(ctx, "normalize.total", "")
	files, err := normalizeAll(ctx, o, []string{ssOut, ytOut}, o.OutputDir)
	end(err)
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
	o.emit(Event{Ev: "phase", Phase: "done"})
	o.emit(Event{Ev: "result", Files: files, Count: len(tracks), Missing: len(missing), Dir: o.OutputDir, Name: coll.Name})
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

func runStream(ctx context.Context, o *Options, args ...string) (retErr error) {
	o.log(`{"type":"source","data":{"source":"soulseek"}}`)
	finish := timing.Begin(ctx, "soulseek.process_total", "")
	defer func() { finish(retErr) }()
	progress := newSoulseekTiming(ctx)
	defer func() { progress.finish(retErr) }()
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
		progress.line(line)
		if o.track != nil {
			o.track.handle(line)
		}
		o.log("%s", line)
		// Sockseek cannot sign in: tell the front end, so it can ask for the account.
		if strings.Contains(line, "Missing Soulseek username") || strings.Contains(line, "Soulseek login failed") ||
			strings.Contains(line, "Failed to ensure Soulseek connection and login") {
			o.emit(Event{Ev: "login", Message: line})
		}
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
	if o.ProgressJSON || o.MeasureTimings || o.OnTiming != nil || o.Event != nil {
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
func runSockseekAlbum(ctx context.Context, o *Options, artist, album, outDir string, tracks int) (int, error) {
	query := album
	if artist != "" {
		query = artist + " - " + album
	}
	args := []string{query, "-o", outDir, "--format", "mp3", "--pref-min-bitrate", o.Bitrate,
		// A folder with far fewer files is another edition: skip it. The
		// tracks it would leave out are searched one by one afterwards.
		"--min-album-track-count", strconv.Itoa(max(1, tracks*85/100)),
		// A peer that sends one file and then waits is common. Give the rest
		// of the folder more time before trying another folder.
		"--max-stale-time", "90000"}
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

func fallback(ctx context.Context, o *Options, missing []missingTrack, ytOut string, prepared *fallbackSearch) {
	if len(missing) > 0 {
		o.log(`{"type":"source","data":{"source":"youtube"}}`)
		o.emit(Event{Ev: "phase", Phase: "youtube"})
	}
	finish := timing.Begin(ctx, "youtube.fallback_total", "")
	defer func() { finish(ctx.Err()) }()
	for i, m := range missing {
		if ctx.Err() != nil {
			return
		}
		o.log("[%d/%d] %s - %s", i+1, len(missing), m.Artist, m.Title)
		o.trackState(m, StateYTSearch, Event{Source: "youtube"})
		var cands []ytdlp.Candidate
		var err error
		if prepared != nil && prepared.track == m {
			end := timing.Begin(ctx, "youtube.wait_for_prepared_search", m.Artist+" - "+m.Title)
			cands, err = prepared.get(ctx)
			end(err)
			if err != nil && ctx.Err() == nil {
				cands, err = ytdlp.SearchCandidates(ctx, m.Artist, m.Title, m.Duration, 5)
			}
		} else {
			cands, err = ytdlp.SearchCandidates(ctx, m.Artist, m.Title, m.Duration, 5)
		}
		if err != nil || len(cands) == 0 {
			if err != nil {
				o.log("    search failed: %v", err)
			}
			o.log("    no candidates")
			o.trackState(m, StateMissing, Event{})
			continue
		}
		best := cands[0]
		if best.Score < o.MinScore {
			o.log("    best score %.0f < %.0f, skipping", best.Score, o.MinScore)
			o.trackState(m, StateMissing, Event{})
			continue
		}
		name := ytdlp.SafeName(m.Artist + " - " + m.Title)
		if name == "" {
			name = ytdlp.SafeName(m.Title)
		}
		o.log("    downloading: %s (score %.0f)", best.Title, best.Score)
		o.log(`{"type":"download_start","data":{}}`)
		o.trackState(m, StateYTDownload, Event{Source: "youtube", Peer: "YouTube Music"})
		if err := ytdlp.Download(ctx, best, ytOut, name, o.Bitrate+"K"); err != nil {
			o.log("    download failed: %v", err)
			o.trackState(m, StateMissing, Event{})
			continue
		}
		if o.track != nil {
			if i := o.track.findLocked(m.Artist, m.Title); i >= 0 {
				o.track.setSrc(i, filepath.Join(ytOut, name+".mp3"))
			}
		}
		o.trackState(m, StateDone, Event{Source: "youtube", Peer: "YouTube Music", URL: best.URL})
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
		if err != nil {
			return nil
		}
		if info.IsDir() {
			// Sockseek moves the files of an album folder that did not
			// finish to "failed".
			if path != dir && info.Name() == "failed" {
				return filepath.SkipDir
			}
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
	end := timing.Begin(ctx, "normalize.ffprobe", filepath.Base(path))
	cctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, o.FfprobePath, "-v", "error", "-select_streams", "a:0",
		"-show_entries", "stream=bit_rate", "-of", "default=nw=1:nk=1", path)
	out, err := cmd.Output()
	end(err)
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
	total, seen := 0, 0
	if o.Event != nil {
		for _, srcDir := range sources {
			total += len(walkAudio(srcDir))
		}
	}
	var assigned map[string]int
	if o.track != nil {
		var all []string
		for _, srcDir := range sources {
			all = append(all, walkAudio(srcDir)...)
		}
		assigned = o.track.assignFiles(all)
	}
	placed := func(src, dest string) {
		if i, ok := assigned[src]; ok {
			o.emit(Event{Ev: "file", I: intp(i), Path: dest})
		}
	}
	for _, srcDir := range sources {
		end := timing.Begin(ctx, "normalize.scan", filepath.Base(srcDir))
		audio := walkAudio(srcDir)
		end(nil)
		for _, src := range audio {
			if ctx.Err() != nil {
				return count, ctx.Err()
			}
			o.emit(Event{Ev: "convert", Done: seen, Count: total})
			seen++
			base := ytdlp.SafeName(strings.TrimSuffix(filepath.Base(src), filepath.Ext(src)))
			if base == "" {
				base = "track"
			}
			dest := uniqueDest(destDir, base)
			bitrate := 0
			if strings.EqualFold(filepath.Ext(src), ".mp3") {
				bitrate = ffprobeBitrate(ctx, o, src)
			}
			if strings.EqualFold(filepath.Ext(src), ".mp3") && bitrate == target {
				end = timing.Begin(ctx, "normalize.copy", filepath.Base(src))
				err := copyFile(src, dest)
				end(err)
				if err != nil {
					o.log("copy failed: %v", err)
					continue
				}
				count++
				placed(src, dest)
				continue
			}
			end = timing.Begin(ctx, "normalize.ffmpeg", filepath.Base(src))
			// Keep the cover picture. A source whose picture ffmpeg cannot
			// put in an mp3 is converted again without it.
			err := transcode(ctx, o, src, dest, true)
			if err != nil && ctx.Err() == nil {
				err = transcode(ctx, o, src, dest, false)
			}
			end(err)
			if err != nil {
				o.log("transcode failed for %s: %v", filepath.Base(src), err)
				continue
			}
			count++
			placed(src, dest)
		}
	}
	o.emit(Event{Ev: "convert", Done: total, Count: total})
	return count, nil
}

func transcode(ctx context.Context, o *Options, src, dest string, cover bool) error {
	cctx, cancel := context.WithTimeout(ctx, 10*time.Minute)
	defer cancel()
	args := []string{"-y", "-loglevel", "error", "-i", src}
	if cover {
		args = append(args, "-map", "0:a:0", "-map", "0:v:0?", "-c:v", "copy", "-disposition:v:0", "attached_pic", "-id3v2_version", "3")
	} else {
		args = append(args, "-vn")
	}
	args = append(args, "-map_metadata", "0", "-c:a", "libmp3lame", "-b:a", o.Bitrate+"k", dest)
	err := exec.CommandContext(cctx, o.FfmpegPath, args...).Run()
	if err != nil {
		_ = os.Remove(dest)
	}
	return err
}

// albumMissing returns the tracks that no file of the album folder is for.
// Each file goes to the track with the longest matching title, so "Billie
// Jean (Underground Mix)" does not count for "Billie Jean".
func albumMissing(o *Options, tracks []spotify.Track, files []string) []missingTrack {
	var missing []missingTrack
	if o.track != nil {
		used := map[int]bool{}
		for _, i := range o.track.assignFiles(files) {
			used[i] = true
		}
		for _, t := range tracks {
			if i := o.track.findLocked(t.Artist, t.Title); i < 0 || !used[i] {
				missing = append(missing, missingTrack{Artist: t.Artist, Title: t.Title, Duration: t.DurationSec})
			}
		}
		return missing
	}
	var names string
	for _, f := range files {
		names += normTitle(filepath.Base(f)) + "|"
	}
	for _, t := range tracks {
		if !strings.Contains(names, normTitle(baseTitle(t.Title))) {
			missing = append(missing, missingTrack{Artist: t.Artist, Title: t.Title, Duration: t.DurationSec})
		}
	}
	return missing
}

// searchTracks runs Sockseek for each of the missing tracks and returns the
// ones it did not find. A failure to run it leaves them all missing.
func (o *Options) searchTracks(ctx context.Context, missing []missingTrack, name, ssOut string) []missingTrack {
	var tracks []spotify.Track
	for _, m := range missing {
		tracks = append(tracks, spotify.Track{Artist: m.Artist, Title: m.Title, DurationSec: m.Duration})
	}
	csvPath := filepath.Join(o.WorkDir, name+".csv")
	if err := writeTracksCSV(csvPath, tracks); err != nil {
		return missing
	}
	o.log("Searching Soulseek for the %d tracks one by one...", len(tracks))
	for _, m := range missing {
		o.trackState(m, StateSearching, Event{})
	}
	if err := runSockseek(ctx, o, csvPath, ssOut); err != nil {
		return missing
	}
	return readMissing(filepath.Join(ssOut, name, "_index.csv"), tracks)
}

// markAlbum marks the tracks found in an album folder as done, and the rest as failed.
func (o *Options) markAlbum(missing []missingTrack) {
	if o.track == nil {
		return
	}
	miss := map[int]bool{}
	o.track.mu.Lock()
	defer o.track.mu.Unlock()
	for _, m := range missing {
		if i := o.track.find(m.Artist, m.Title); i >= 0 {
			miss[i] = true
		}
	}
	for i := range o.track.tracks {
		if miss[i] {
			o.track.setAny(i, StateFailed, Event{})
		} else if o.track.state[i] != StateDone {
			o.track.set(i, StateDone, Event{Source: "soulseek"})
		}
	}
}

// trackState sends a track event for a track that is known by artist and title.
func (o *Options) trackState(m missingTrack, state string, e Event) {
	if o.track == nil {
		return
	}
	o.track.mu.Lock()
	defer o.track.mu.Unlock()
	if i := o.track.find(m.Artist, m.Title); i >= 0 {
		o.track.set(i, state, e)
	}
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
