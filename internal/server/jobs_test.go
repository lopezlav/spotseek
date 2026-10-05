// AI-generated code begins.
package server

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"spotseek-app/internal/libraryindex"
	"spotseek-app/internal/spotify"
)

func testJobManager(t *testing.T, dir string) (*Manager, context.CancelFunc) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	settings := &Settings{OutputDir: dir, Concurrency: 0}
	index := libraryindex.New(filepath.Join(dir, ".spotseek", "library-index.json"), dir)
	m := NewManager(ctx, settings, index)
	t.Cleanup(func() { cancel(); m.Wait() })
	return m, cancel
}

func TestJobsPersistAndRestoreWithoutRestartingDownloads(t *testing.T) {
	dir := t.TempDir()
	m, cancel := testJobManager(t, dir)
	states := []JobState{JobDone, JobAlready, JobFailed, JobCanceled, JobQueued, JobRunning, JobDownloading}
	m.mu.Lock()
	for i, state := range states {
		m.putLocked(&Job{ID: fmt.Sprint(i), AccountID: "account-a", State: state, Path: "/saved/song.mp3", Percent: 35,
			Track: spotify.Track{ID: fmt.Sprint(i), Title: "Saved Title"}, Error: "Original error", LogLines: []string{"Saved log"}})
	}
	m.mu.Unlock()
	if err := m.Flush(); err != nil {
		t.Fatal(err)
	}
	cancel()
	m.Wait()
	restored, _ := testJobManager(t, dir)
	if restored.Epoch() == m.Epoch() || restored.Epoch() == "" {
		t.Fatal("new process did not get a new epoch")
	}
	seq, jobs := restored.SnapshotForAccount(0, "", "account-a")
	if len(jobs) != len(states) {
		t.Fatalf("got %d jobs, want %d", len(jobs), len(states))
	}
	if seq == 0 || restored.ActiveCount() != 0 || len(restored.queue) != 0 {
		t.Fatal("restore queued a download")
	}
	for i, job := range jobs {
		if activeState(states[i]) {
			if job.State != JobFailed || !strings.Contains(job.Error, "Server stopped") {
				t.Fatalf("interrupted job not failed: %+v", job)
			}
		} else if job.State != states[i] || job.Error != "Original error" {
			t.Fatalf("terminal job changed: %+v", job)
		}
		if job.AccountID != "account-a" || job.Path != "/saved/song.mp3" || job.Track.Title != "Saved Title" || len(job.LogLines) != 1 {
			t.Fatalf("history lost: %+v", job)
		}
	}
	if _, jobs := restored.SnapshotForAccount(seq, "", "account-a"); len(jobs) != 0 {
		t.Fatal("unchanged jobs repeated")
	}
	if _, jobs := restored.SnapshotForAccount(seq+100, "", "account-a"); len(jobs) != len(states) {
		t.Fatal("future cursor hid restored jobs")
	}
}

func TestJobsAccountIsolationAndSnapshotCopies(t *testing.T) {
	m, _ := testJobManager(t, t.TempDir())
	track := spotify.Track{ID: "same-song", Title: "Song"}
	a := m.EnqueueForAccount("playlist", "A", []spotify.Track{track}, "account-a")
	b := m.EnqueueForAccount("playlist", "B", []spotify.Track{track}, "account-b")
	if a[0].ID == b[0].ID {
		t.Fatal("active jobs shared across accounts")
	}
	if m.ActiveCountForAccount("account-a") != 1 || m.ActiveCountForAccount("account-b") != 1 {
		t.Fatal("active counts mixed accounts")
	}
	_, jobs := m.SnapshotForAccount(0, "", "account-a")
	if len(jobs) != 1 || jobs[0].ID != a[0].ID {
		t.Fatal("snapshot included another account")
	}
	m.handleLog(a[0].ID, "First log")
	_, jobs = m.SnapshotForAccount(0, "", "account-a")
	jobs[0].LogLines[0] = "Modified copy"
	a[0].Track.Title = "Modified copy"
	_, fresh := m.SnapshotForAccount(0, "", "account-a")
	if fresh[0].LogLines[0] != "First log" || fresh[0].Track.Title != "Song" {
		t.Fatal("response shares mutable state")
	}
	if m.CancelForAccount(nil, "", "account-a") != 1 {
		t.Fatal("cancel count wrong")
	}
	if m.ActiveCountForAccount("account-b") != 1 {
		t.Fatal("cancel stopped another account")
	}
	m.SetAccount("account-b")
	if got := m.Enqueue("playlist", "B", []spotify.Track{track}); got[0].ID != b[0].ID {
		t.Fatal("default account not used")
	}
}

func TestJobLogChangesAreInSnapshots(t *testing.T) {
	m, _ := testJobManager(t, t.TempDir())
	job := m.EnqueueForAccount("track", "", []spotify.Track{{ID: "x"}}, "account")[0]
	seq, _ := m.Snapshot(0, "")
	m.handleLog(job.ID, "Plain diagnostic line")
	_, jobs := m.Snapshot(seq, "")
	if len(jobs) != 1 || jobs[0].LastLog != "Plain diagnostic line" {
		t.Fatal("log did not advance sequence")
	}
}

func TestAcceptedAndTerminalJobsAreSavedBeforeReturn(t *testing.T) {
	dir := t.TempDir()
	m, _ := testJobManager(t, dir)
	job := m.EnqueueForAccount("track", "", []spotify.Track{{ID: "x"}}, "account")[0]
	read := func() Job {
		t.Helper()
		raw, err := os.ReadFile(filepath.Join(dir, ".spotseek", "jobs.json"))
		if err != nil {
			t.Fatal(err)
		}
		var saved savedJobs
		if err := json.Unmarshal(raw, &saved); err != nil {
			t.Fatal(err)
		}
		if len(saved.Jobs) != 1 {
			t.Fatalf("got %d saved jobs", len(saved.Jobs))
		}
		return saved.Jobs[0]
	}
	if got := read(); got.State != JobQueued {
		t.Fatalf("accepted job not saved: %+v", got)
	}
	m.finish(job.ID, JobFailed, "", "", "Test failure")
	if got := read(); got.State != JobFailed || got.Error != "Test failure" {
		t.Fatalf("terminal result not saved: %+v", got)
	}
}

func TestJobFlushSkipsUnchangedWritesAndMovesWithOutput(t *testing.T) {
	dir := t.TempDir()
	m, _ := testJobManager(t, dir)
	m.EnqueueForAccount("track", "", []spotify.Track{{ID: "x"}}, "account")
	if err := m.Flush(); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, ".spotseek", "jobs.json")
	before, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := m.Flush(); err != nil {
		t.Fatal(err)
	}
	after, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if !os.SameFile(before, after) {
		t.Fatal("unchanged Flush replaced the file")
	}
	newDir := t.TempDir()
	m.SetSettings(&Settings{OutputDir: newDir})
	if err := m.Flush(); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(filepath.Join(newDir, ".spotseek", "jobs.json"))
	if err != nil {
		t.Fatal(err)
	}
	var saved savedJobs
	if err := json.Unmarshal(raw, &saved); err != nil {
		t.Fatal(err)
	}
	if len(saved.Jobs) != 1 {
		t.Fatal("history lost after folder change")
	}
}

func TestJobHistoryTrimKeepsActiveJobsVisible(t *testing.T) {
	m, _ := testJobManager(t, t.TempDir())
	m.mu.Lock()
	for i := 0; i < 3002; i++ {
		state := JobDone
		if i == 0 {
			state = JobQueued
		}
		m.putLocked(&Job{ID: fmt.Sprint(i), State: state})
	}
	m.mu.Unlock()
	_, jobs := m.Snapshot(0, "")
	if m.ActiveCount() != 1 || len(jobs) > 3000 {
		t.Fatal("history limit lost active job")
	}
	found := false
	for _, job := range jobs {
		if job.ID == "0" {
			found = true
		}
	}
	if !found {
		t.Fatal("active job removed from ordered snapshots")
	}
}

// AI-generated code ends.

func TestDownloadSourceChangesBeforeCompletion(t *testing.T) {
	m, _ := testJobManager(t, t.TempDir())
	m.mu.Lock()
	job := &Job{ID: "source-test", AccountID: "account-a", State: JobRunning, Track: spotify.Track{ID: "source-track", Title: "Title"}}
	m.putLocked(job)
	m.mu.Unlock()
	for _, source := range []string{"soulseek", "youtube"} {
		m.handleLog(job.ID, fmt.Sprintf(`{"type":"source","data":{"source":"%s"}}`, source))
		_, jobs := m.SnapshotForAccount(0, "", "account-a")
		if len(jobs) != 1 || jobs[0].Source != source || jobs[0].State != JobRunning {
			t.Fatalf("live source not reported: %+v", jobs)
		}
	}
}

func TestFailedDownloadKeepsItsLastSource(t *testing.T) {
	m, _ := testJobManager(t, t.TempDir())
	m.mu.Lock()
	m.putLocked(&Job{ID: "source-failed", AccountID: "account-a", Source: "youtube", State: JobDownloading, Track: spotify.Track{ID: "source-track", Title: "Title"}})
	m.mu.Unlock()
	m.finish("source-failed", JobFailed, "", "", "peer failed")
	_, jobs := m.SnapshotForAccount(0, "", "account-a")
	if len(jobs) != 1 || jobs[0].Source != "youtube" {
		t.Fatalf("source lost: %+v", jobs)
	}
}
