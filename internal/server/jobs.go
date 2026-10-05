package server

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"spotseek-app/internal/libraryindex"
	"spotseek-app/internal/pipeline"
	"spotseek-app/internal/spotify"
	"spotseek-app/internal/ytdlp"
)

// JobState is the life cycle state of one track download.
type JobState string

const (
	JobQueued      JobState = "queued"
	JobRunning     JobState = "running"
	JobDownloading JobState = "downloading"
	JobDone        JobState = "done"
	JobFailed      JobState = "failed"
	JobCanceled    JobState = "canceled"
	JobAlready     JobState = "already"
)

// Job is one track download. It is safe to copy for JSON.
type Job struct {
	ID              string        `json:"id"`
	AccountID       string        `json:"account_id,omitempty"`
	BatchID         string        `json:"batch_id"`
	BatchName       string        `json:"batch_name"`
	Kind            string        `json:"kind"`
	Track           spotify.Track `json:"track"`
	State           JobState      `json:"state"`
	Percent         float64       `json:"percent"`
	Path            string        `json:"path,omitempty"`
	Source          string        `json:"source,omitempty"`
	Error           string        `json:"error,omitempty"`
	LastLog         string        `json:"last_log,omitempty"`
	LogLines        []string      `json:"log,omitempty"`
	CreatedAt       time.Time     `json:"created_at"`
	UpdatedAt       time.Time     `json:"updated_at"`
	Progressive     bool          `json:"progressive,omitempty"`
	StreamPath      string        `json:"stream_path,omitempty"`
	BufferedSeconds float64       `json:"buffered_seconds,omitempty"`
	StreamComplete  bool          `json:"stream_complete,omitempty"`
	StreamError     string        `json:"stream_error,omitempty"`

	cancel    context.CancelFunc
	changeSeq uint64
}

// Manager owns the download queue and the worker pool.
type Manager struct {
	mu            sync.Mutex
	prepareMu     sync.Mutex
	ctx           context.Context
	jobs          map[string]*Job
	downloads     map[string]DownloadedTrack
	order         []string
	active        map[string]string // track key -> job id
	queue         chan string
	seq           uint64
	settings      *Settings
	index         *libraryindex.Index
	wg            sync.WaitGroup
	persistWG     sync.WaitGroup
	saveMu        sync.Mutex
	dirty         chan struct{}
	revision      uint64
	savedRevision uint64
	savedPath     string
	epoch         string
	accountID     string
	warm          *warmTrack

	// Soulseek is marked as unavailable after a login failure. New jobs
	// then use the YouTube fallback directly for a while.
	soulseekDownUntil time.Time
}

const soulseekCooldown = 15 * time.Minute

// NewManager starts the worker pool.
func NewManager(ctx context.Context, settings *Settings, index *libraryindex.Index) *Manager {
	m := &Manager{
		ctx:       ctx,
		jobs:      make(map[string]*Job),
		downloads: make(map[string]DownloadedTrack),
		active:    make(map[string]string),
		queue:     make(chan string, 10000),
		settings:  settings,
		index:     index,
		dirty:     make(chan struct{}, 1),
		epoch:     randomID(16),
	}
	if err := m.restore(); err != nil {
		log.Printf("jobs: cannot restore saved downloads: %v", err)
	}
	m.persistWG.Add(1)
	go m.persistLoop()
	for i := 0; i < settings.Concurrency; i++ {
		m.wg.Add(1)
		go m.worker()
	}
	return m
}

// Wait blocks until all workers stop.
func (m *Manager) Wait() {
	m.wg.Wait()
	m.mu.Lock()
	warm := m.warm
	m.warm = nil
	m.mu.Unlock()
	if warm != nil {
		warm.preparation.Stop()
	}
	if err := m.Flush(); err != nil {
		log.Printf("jobs: cannot save downloads: %v", err)
	}
	m.persistWG.Wait()
}

// SetSettings swaps the settings used by new jobs.
func (m *Manager) SetSettings(s *Settings) {
	m.mu.Lock()
	warm := m.warm
	m.warm = nil
	m.settings = s
	m.markDirtyLocked()
	m.mu.Unlock()
	if warm != nil {
		warm.preparation.Stop()
	}
}

func randomID(n int) string {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return fmt.Sprintf("%d", time.Now().UnixNano())
	}
	return hex.EncodeToString(b)
}

func trackKey(t spotify.Track) string {
	if t.ID != "" {
		return "id:" + t.ID
	}
	return libraryindex.TrackKey(t.Artist, t.Title)
}

func scopeFolder(name, kind string) string {
	if kind == "search" {
		return "Search"
	}
	if kind == "track" || strings.TrimSpace(name) == "" {
		return "Singles"
	}
	safe := ytdlp.SafeName(name)
	if safe == "" {
		return "Spotify"
	}
	return safe
}

func (m *Manager) nextSeqLocked() uint64 {
	m.seq++
	return m.seq
}

func (m *Manager) touchLocked(j *Job) {
	j.changeSeq = m.nextSeqLocked()
	j.UpdatedAt = time.Now()
	m.rememberDownloadLocked(j)
	m.markDirtyLocked()
}

func (m *Manager) putLocked(j *Job) {
	m.jobs[j.ID] = j
	m.order = append(m.order, j.ID)
	m.touchLocked(j)
	if len(m.order) > 3000 {
		remaining := make([]string, 0, len(m.order))
		drop := len(m.order) - 2000
		for _, id := range m.order {
			if old := m.jobs[id]; drop > 0 && old != nil && !activeState(old.State) {
				delete(m.jobs, id)
				drop--
			} else {
				remaining = append(remaining, id)
			}
		}
		m.order = remaining
	}
}

func (m *Manager) newJobLocked(batchID, kind, name string, t spotify.Track) *Job {
	return &Job{
		ID:        randomID(8),
		BatchID:   batchID,
		BatchName: name,
		Kind:      kind,
		Track:     t,
		State:     JobQueued,
		CreatedAt: time.Now(),
	}
}

// Enqueue adds tracks to the queue. Tracks that are already in the library
// index come back as "already" jobs with their path. Duplicate active jobs
// are returned once.
func (m *Manager) Enqueue(kind, name string, tracks []spotify.Track) []*Job {
	m.mu.Lock()
	accountID := m.accountID
	m.mu.Unlock()
	return m.EnqueueForAccount(kind, name, tracks, accountID)
}

// AI-generated code begins.
func (m *Manager) EnqueueForAccount(kind, name string, tracks []spotify.Track, accountID string) []*Job {
	return m.EnqueuePlaybackForAccount(kind, name, tracks, accountID, false)
}

func (m *Manager) EnqueuePlaybackForAccount(kind, name string, tracks []spotify.Track, accountID string, progressive bool) []*Job {
	batchID := randomID(8)
	var toQueue []string
	var out []*Job
	matches := make(map[string]libraryindex.LookupResult, len(tracks))
	for _, t := range tracks {
		matches[trackKey(t)] = m.index.Lookup(t)
	}

	m.mu.Lock()
	seen := make(map[string]bool, len(tracks))
	for _, t := range tracks {
		key := accountTrackKey(accountID, t)
		if seen[key] {
			continue
		}
		seen[key] = true

		if res := matches[trackKey(t)]; res.Found {
			j := m.newJobLocked(batchID, kind, name, t)
			j.AccountID = accountID
			j.State = JobAlready
			j.Percent = 100
			j.Path = res.Path
			j.Source = res.Source
			m.putLocked(j)
			out = append(out, copyJob(j))
			continue
		}
		if id, ok := m.active[key]; ok {
			if j, ok := m.jobs[id]; ok {
				out = append(out, copyJob(j))
				continue
			}
		}
		j := m.newJobLocked(batchID, kind, name, t)
		j.AccountID = accountID
		j.Progressive = progressive
		m.putLocked(j)
		m.active[key] = j.ID
		out = append(out, copyJob(j))
		toQueue = append(toQueue, j.ID)
	}
	m.mu.Unlock()

	if err := m.Flush(); err != nil {
		log.Printf("jobs: cannot save accepted downloads: %v", err)
	}
	for _, id := range toQueue {
		select {
		case m.queue <- id:
		case <-m.ctx.Done():
			m.finish(id, JobCanceled, "", "", "server stopped")
		}
	}
	return out
}

// AI-generated code ends.

func (m *Manager) worker() {
	defer m.wg.Done()
	for {
		select {
		case <-m.ctx.Done():
			return
		case id := <-m.queue:
			m.runJob(id)
		}
	}
}

func (m *Manager) setState(id string, state JobState, percent float64) {
	m.mu.Lock()
	if j, ok := m.jobs[id]; ok {
		if state != "" {
			j.State = state
		}
		if percent >= 0 {
			j.Percent = percent
		}
		m.touchLocked(j)
	}
	m.mu.Unlock()
}

func (m *Manager) finish(id string, state JobState, path, source, errMsg string) {
	m.mu.Lock()
	j, ok := m.jobs[id]
	if !ok {
		m.mu.Unlock()
		return
	}
	j.State = state
	if state == JobDone {
		j.Percent = 100
	}
	j.Path = path
	j.Source = source
	j.Error = errMsg
	j.cancel = nil
	delete(m.active, accountTrackKey(j.AccountID, j.Track))
	m.touchLocked(j)
	m.mu.Unlock()
	if err := m.Flush(); err != nil {
		log.Printf("jobs: cannot save completed download: %v", err)
	}
}

func (m *Manager) runJob(id string) {
	m.mu.Lock()
	j, ok := m.jobs[id]
	if !ok || j.State != JobQueued {
		m.mu.Unlock()
		return
	}
	ctx, cancel := context.WithCancel(m.ctx)
	j.cancel = cancel
	j.State = JobRunning
	j.Percent = 1
	m.touchLocked(j)
	settings := *m.settings
	track := j.Track
	batchName := j.BatchName
	batchKind := j.Kind
	skipSoulseek := time.Now().Before(m.soulseekDownUntil)
	progressive := j.Progressive
	var preparation *pipeline.PreparedTrack
	var unusedPreparation *pipeline.PreparedTrack
	if m.warm != nil && m.warm.key == accountTrackKey(j.AccountID, track) && time.Now().Before(m.warm.expires) {
		preparation = m.warm.preparation
		m.warm = nil
	} else if m.warm != nil {
		unusedPreparation = m.warm.preparation
		m.warm = nil
	}
	m.mu.Unlock()
	if unusedPreparation != nil {
		unusedPreparation.Stop()
	}

	outputDir := filepath.Join(settings.OutputDir, scopeFolder(batchName, batchKind))
	workDir := filepath.Join(settings.OutputDir, ".spotseek", "work", id)
	var playback *pipeline.ProgressivePlayback
	if progressive && track.DurationSec > 0 {
		var err error
		playback, err = pipeline.StartProgressivePlayback(ctx, workDir, track, settings.FfmpegPath, func(path string, status pipeline.PlaybackStatus) {
			m.mu.Lock()
			if job := m.jobs[id]; job != nil {
				job.StreamPath = path
				job.BufferedSeconds = float64(status.BufferedFrames) / float64(status.SampleRate)
				job.StreamComplete = status.Complete
				job.StreamError = status.Error
				m.touchLocked(job)
			}
			m.mu.Unlock()
		})
		if err != nil {
			m.handleLog(id, "Progressive playback unavailable: "+err.Error())
		}
	}
	opts := &pipeline.Options{
		Collection: &spotify.Collection{
			ID:     track.ID,
			Kind:   "track",
			Name:   batchName,
			Artist: track.Artist,
			Tracks: []spotify.Track{track},
		},
		Preparation:    preparation,
		OutputDir:      outputDir,
		WorkDir:        workDir,
		ScopeName:      "job-" + id,
		Bitrate:        settings.Bitrate,
		UseFallback:    settings.Fallback,
		ProgressJSON:   true,
		MeasureTimings: true,
		SockseekWait:   time.Duration(settings.SockseekWaitSec) * time.Second,
		SkipSoulseek:   skipSoulseek,
		MinScore:       settings.MinScore,
		SockseekPath:   settings.SockseekPath,
		FfmpegPath:     settings.FfmpegPath,
		FfprobePath:    settings.FfprobePath,
		SoulseekUser:   settings.SoulseekUser,
		SoulseekPass:   settings.SoulseekPass,
		Log: func(line string) {
			m.handleLog(id, line)
		},
	}
	if settings.Fallback {
		// A login failure repeats forever in sockseek. Stop it early and
		// let the fallback produce the file.
		opts.AbortSockseekOn = func(line string) bool {
			return strings.Contains(line, "Failed to ensure Soulseek connection and login")
		}
	}

	res, err := pipeline.Run(ctx, opts)
	if playback != nil {
		if err == nil && res != nil && res.Missing == 0 && res.Files > 0 {
			playback.Finish()
		} else {
			playback.Cancel()
		}
	}
	if err != nil {
		if ctx.Err() != nil {
			m.finish(id, JobCanceled, "", "", "canceled")
		} else {
			m.finish(id, JobFailed, "", "", err.Error())
		}
		return
	}

	path, source := findResult(outputDir, track, res.Missing)
	if path == "" {
		m.finish(id, JobFailed, "", "", "no audio file was produced")
		return
	}
	entry := libraryindex.Entry{
		SpotifyID:   track.ID,
		Artist:      track.Artist,
		Title:       track.Title,
		Album:       track.Album,
		DurationSec: track.DurationSec,
		Path:        path,
		Source:      source,
	}
	if err := m.index.Add(entry); err != nil {
		log.Printf("index: cannot save %s: %v", path, err)
	}
	m.finish(id, JobDone, path, source, "")
}

// findResult returns the final file of a job.
func findResult(outputDir string, track spotify.Track, missing int) (string, string) {
	source := "soulseek"
	if missing > 0 {
		source = "youtube"
	}
	wantTitle := libraryindex.Normalize(track.Title)
	wantArtist := libraryindex.Normalize(track.Artist)
	var best string
	var audio []string
	_ = filepath.Walk(outputDir, func(path string, info os.FileInfo, err error) error {
		if err != nil || info == nil || info.IsDir() {
			return nil
		}
		switch strings.ToLower(filepath.Ext(path)) {
		case ".mp3", ".flac", ".m4a", ".opus", ".wav", ".ogg", ".aac", ".webm":
		default:
			return nil
		}
		audio = append(audio, path)
		base := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
		artist, title := libraryindex.ParseFileName(base)
		if libraryindex.TitleMatches(wantTitle, libraryindex.Normalize(title)) &&
			libraryindex.ArtistMatches(wantArtist, libraryindex.Normalize(artist)) {
			best = path
		}
		return nil
	})
	if best != "" {
		return best, source
	}
	if len(audio) == 1 {
		return audio[0], source
	}
	return "", source
}

// handleLog parses sockseek progress lines. Plain text lines only update the
// last-log field for debugging.
func (m *Manager) handleLog(id, line string) {
	line = strings.TrimSpace(line)
	if line == "" {
		return
	}
	m.mu.Lock()
	j, ok := m.jobs[id]
	if !ok {
		m.mu.Unlock()
		return
	}
	j.LastLog = line
	j.LogLines = append(j.LogLines, line)
	if len(j.LogLines) > 100 {
		j.LogLines = append([]string(nil), j.LogLines[len(j.LogLines)-100:]...)
	}
	m.touchLocked(j)
	if strings.Contains(line, "Failed to ensure Soulseek connection and login") &&
		time.Now().After(m.soulseekDownUntil) {
		m.soulseekDownUntil = time.Now().Add(soulseekCooldown)
		log.Printf("Soulseek login failed; new jobs use the YouTube fallback for %s",
			soulseekCooldown)
	}
	m.mu.Unlock()

	if !strings.HasPrefix(line, "{") {
		return
	}
	var ev struct {
		Type string          `json:"type"`
		Data json.RawMessage `json:"data"`
	}
	if err := json.Unmarshal([]byte(line), &ev); err != nil {
		return
	}
	switch ev.Type {
	case "search_start":
		m.setState(id, JobRunning, 5)
	case "download_start":
		m.setState(id, JobDownloading, 10)
	case "download_progress":
		var d struct {
			Percent float64 `json:"percent"`
		}
		if json.Unmarshal(ev.Data, &d) == nil {
			m.setState(id, JobDownloading, 10+d.Percent*0.8)
		}
	case "progress":
		var d struct {
			Percent float64 `json:"percent"`
		}
		if json.Unmarshal(ev.Data, &d) == nil {
			m.setState(id, JobDownloading, 10+d.Percent*0.8)
		}
	case "track_state":
		var d struct {
			TerminalOutcome string `json:"terminalOutcome"`
		}
		if json.Unmarshal(ev.Data, &d) == nil && strings.EqualFold(d.TerminalOutcome, "Succeeded") {
			m.setState(id, JobDownloading, 95)
		}
	}
}

// Snapshot returns jobs changed after the given sequence number.
func (m *Manager) Snapshot(afterSeq uint64, batchID string) (uint64, []Job) {
	return m.snapshot(afterSeq, batchID, "", false)
}

// AI-generated code begins.
func (m *Manager) SnapshotForAccount(afterSeq uint64, batchID, accountID string) (uint64, []Job) {
	return m.snapshot(afterSeq, batchID, accountID, true)
}

func (m *Manager) snapshot(afterSeq uint64, batchID, accountID string, filterAccount bool) (uint64, []Job) {
	m.mu.Lock()
	defer m.mu.Unlock()
	seq := m.seq
	if afterSeq > seq {
		afterSeq = 0
	}
	var out []Job
	for _, id := range m.order {
		j, ok := m.jobs[id]
		if !ok || j.changeSeq <= afterSeq {
			continue
		}
		if batchID != "" && j.BatchID != batchID {
			continue
		}
		if filterAccount && j.AccountID != accountID {
			continue
		}
		out = append(out, *copyJob(j))
	}
	return seq, out
}

// AI-generated code ends.

// Cancel stops queued and running jobs. An empty id list with a batch id
// cancels the whole batch. An empty id list and batch id cancels everything.
func (m *Manager) Cancel(ids []string, batchID string) int {
	return m.cancelJobs(ids, batchID, "", false)
}

// AI-generated code begins.
func (m *Manager) CancelForAccount(ids []string, batchID, accountID string) int {
	return m.cancelJobs(ids, batchID, accountID, true)
}

func (m *Manager) cancelJobs(ids []string, batchID, accountID string, filterAccount bool) int {
	idSet := make(map[string]bool, len(ids))
	for _, id := range ids {
		idSet[id] = true
	}
	m.mu.Lock()
	var targets []*Job
	for _, id := range m.order {
		j, ok := m.jobs[id]
		if !ok {
			continue
		}
		if batchID != "" && j.BatchID != batchID {
			continue
		}
		if filterAccount && j.AccountID != accountID {
			continue
		}
		if len(idSet) > 0 && !idSet[j.ID] {
			continue
		}
		if j.State == JobQueued || j.State == JobRunning || j.State == JobDownloading {
			targets = append(targets, j)
		}
	}
	for _, j := range targets {
		if j.State == JobQueued {
			j.State = JobCanceled
			delete(m.active, accountTrackKey(j.AccountID, j.Track))
			m.touchLocked(j)
		}
	}
	m.mu.Unlock()

	for _, j := range targets {
		m.mu.Lock()
		cancel := j.cancel
		m.mu.Unlock()
		if cancel != nil {
			cancel()
		}
	}
	return len(targets)
}

// AI-generated code ends.

// ActiveCount returns how many jobs are not finished.
func (m *Manager) ActiveCount() int {
	return m.activeCount("", false)
}

// AI-generated code begins.
func (m *Manager) ActiveCountForAccount(accountID string) int {
	return m.activeCount(accountID, true)
}

func (m *Manager) activeCount(accountID string, filterAccount bool) int {
	m.mu.Lock()
	defer m.mu.Unlock()
	n := 0
	for _, id := range m.order {
		j, ok := m.jobs[id]
		if !ok {
			continue
		}
		if filterAccount && j.AccountID != accountID {
			continue
		}
		if j.State == JobQueued || j.State == JobRunning || j.State == JobDownloading {
			n++
		}
	}
	return n
}

// AI-generated code ends.

// AI-generated code begins.
func activeState(state JobState) bool {
	return state == JobQueued || state == JobRunning || state == JobDownloading
}

func accountTrackKey(accountID string, track spotify.Track) string {
	return accountID + "\x00" + trackKey(track)
}

func copyJob(job *Job) *Job {
	copy := *job
	copy.cancel = nil
	copy.LogLines = append([]string(nil), job.LogLines...)
	return &copy
}

func (m *Manager) Epoch() string {
	return m.epoch
}

func (m *Manager) SetAccount(accountID string) {
	m.mu.Lock()
	m.accountID = accountID
	m.mu.Unlock()
}

type savedJobs struct {
	Version   int               `json:"version"`
	Jobs      []Job             `json:"jobs"`
	Downloads []DownloadedTrack `json:"downloads,omitempty"`
}

func (m *Manager) restore() error {
	path := filepath.Join(m.settings.OutputDir, ".spotseek", "jobs.json")
	raw, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	var saved savedJobs
	if err := json.Unmarshal(raw, &saved); err != nil {
		return err
	}
	if saved.Version != 1 {
		return fmt.Errorf("unsupported download history version %d", saved.Version)
	}
	for _, asset := range saved.Downloads {
		m.downloads[accountTrackKey(asset.AccountID, asset.Track.toTrack())] = asset
	}
	for _, job := range saved.Jobs {
		if job.ID == "" || m.jobs[job.ID] != nil {
			continue
		}
		if activeState(job.State) {
			job.State = JobFailed
			job.Error = "Server stopped before the download finished. Start the download again to retry."
			job.UpdatedAt = time.Now()
			m.markDirtyLocked()
		}
		job.changeSeq = m.nextSeqLocked()
		m.jobs[job.ID] = copyJob(&job)
		m.order = append(m.order, job.ID)
		m.rememberDownloadLocked(&job)
	}
	return nil
}

func (m *Manager) markDirtyLocked() {
	m.revision++
	select {
	case m.dirty <- struct{}{}:
	default:
	}
}

func (m *Manager) persistLoop() {
	defer m.persistWG.Done()
	for {
		select {
		case <-m.ctx.Done():
			if err := m.Flush(); err != nil {
				log.Printf("jobs: cannot save downloads: %v", err)
			}
			return
		case <-m.dirty:
			timer := time.NewTimer(250 * time.Millisecond)
			select {
			case <-m.ctx.Done():
				timer.Stop()
			case <-timer.C:
			}
			if err := m.Flush(); err != nil {
				log.Printf("jobs: cannot save downloads: %v", err)
			}
		}
	}
}

func (m *Manager) Flush() error {
	m.saveMu.Lock()
	defer m.saveMu.Unlock()
	m.mu.Lock()
	path := filepath.Join(m.settings.OutputDir, ".spotseek", "jobs.json")
	revision := m.revision
	if m.savedRevision == revision && m.savedPath == path {
		m.mu.Unlock()
		return nil
	}
	saved := savedJobs{Version: 1, Jobs: make([]Job, 0, len(m.order))}
	for _, id := range m.order {
		if job := m.jobs[id]; job != nil {
			saved.Jobs = append(saved.Jobs, *copyJob(job))
		}
	}
	for _, asset := range m.downloads {
		saved.Downloads = append(saved.Downloads, asset)
	}
	m.mu.Unlock()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	raw, err := json.Marshal(saved)
	if err != nil {
		return err
	}
	file, err := os.CreateTemp(filepath.Dir(path), ".jobs-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(file.Name())
	if _, err := file.Write(raw); err != nil {
		file.Close()
		return err
	}
	if err := file.Sync(); err != nil {
		file.Close()
		return err
	}
	if err := file.Close(); err != nil {
		return err
	}
	if err := os.Rename(file.Name(), path); err != nil {
		return err
	}
	m.mu.Lock()
	m.savedRevision = revision
	m.savedPath = path
	m.mu.Unlock()
	return nil
}

// AI-generated code ends.
