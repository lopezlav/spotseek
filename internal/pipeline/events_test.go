package pipeline

import (
	"os"
	"path/filepath"
	"testing"

	"spotseek-app/internal/spotify"
)

func newTestTracker(tracks ...spotify.Track) (*tracker, *[]Event) {
	var got []Event
	o := &Options{Event: func(e Event) { got = append(got, e) }}
	return newTracker(o, tracks), &got
}

func TestTrackStateWaitsForTheTerminalOutcome(t *testing.T) {
	tr, got := newTestTracker(spotify.Track{Artist: "Michael Jackson", Title: "Thriller"})
	// A job that is still queued or downloading has the outcome "None".
	tr.handle(`{"type":"track_state","data":{"artist":"Michael Jackson","title":"Thriller","lifecycleState":"Running","activityPhase":"Downloading","terminalOutcome":"None"}}`)
	tr.handle(`{"type":"track_state","data":{"artist":"Michael Jackson","title":"Thriller","lifecycleState":"Pending","terminalOutcome":"None"}}`)
	if len(*got) != 0 {
		t.Fatalf("a running job made events: %+v", *got)
	}
	tr.handle(`{"type":"track_state","data":{"artist":"Michael Jackson","title":"Thriller","lifecycleState":"Terminal","terminalOutcome":"Succeeded","username":"peer1","size":9000,"bitRate":320,"downloadPath":"/w/soulseek/Thriller.mp3"}}`)
	if len(*got) != 1 || (*got)[0].State != StateDone || (*got)[0].BitRate != 320 || (*got)[0].Peer != "peer1" {
		t.Fatalf("want one done event, got %+v", *got)
	}
	if tr.src[0] != "/w/soulseek/Thriller.mp3" {
		t.Fatalf("download path not kept: %q", tr.src[0])
	}
	// Outside album mode, a terminal failure of another track is a failure.
	tr2, got2 := newTestTracker(spotify.Track{Artist: "Enya", Title: "Only Time"})
	tr2.handle(`{"type":"track_state","data":{"artist":"Enya","title":"Only Time","lifecycleState":"Terminal","terminalOutcome":"Failed"}}`)
	if len(*got2) != 1 || (*got2)[0].State != StateFailed {
		t.Fatalf("a terminal failure is a failure: %+v", *got2)
	}
}

func TestFileIndexMatchesPathsAndNames(t *testing.T) {
	tr, _ := newTestTracker(
		spotify.Track{Artist: "Michael Jackson", Title: "Thriller"},
		spotify.Track{Artist: "Michael Jackson", Title: "Thriller - Instrumental"},
		spotify.Track{Artist: "Michael Jackson", Title: "Beat It - Single Version"},
		spotify.Track{Artist: "Enya", Title: "Only Time"},
	)
	tr.src[3] = "/w/soulseek/x/whatever.mp3"
	used := map[int]bool{}
	cases := map[string]int{
		"/w/soulseek/x/whatever.mp3":                         3,
		"/w/soulseek/album/28 - Thriller - Instrumental.mp3": 1,
		"/w/soulseek/album/04 - Thriller.mp3":                0,
		"/w/soulseek/album/05 - Beat It.mp3":                 2,
		"/w/youtube/Nobody - Nothing.mp3":                    -1,
	}
	for path, want := range cases {
		if got := tr.fileIndex(path, used); got != want {
			t.Errorf("%s: got %d, want %d", path, got, want)
		}
	}
}

func TestAlbumFolderRetryIsNotNotFound(t *testing.T) {
	tr, got := newTestTracker(
		spotify.Track{Artist: "Michael Jackson", Title: "Wanna Be Startin' Somethin'"},
		spotify.Track{Artist: "Michael Jackson", Title: "Baby Be Mine"},
	)
	tr.album = true
	tr.handle(`{"type":"track_state","data":{"artist":"Michael Jackson","title":"01 - Wanna Be Startin' Somethin'","lifecycleState":"Terminal","terminalOutcome":"Succeeded","filename":"x\\01 - Wanna Be Startin' Somethin'.mp3"}}`)
	// The peer stalls: Sockseek fails the rest of the folder and tries another one.
	tr.handle(`{"type":"track_state","data":{"artist":"Michael Jackson","title":"02 - Baby Be Mine","lifecycleState":"Terminal","terminalOutcome":"Failed","failureReason":"AllDownloadsFailed"}}`)
	// A later folder reports the finished track again: it stays done.
	tr.handle(`{"type":"track_state","data":{"artist":"Michael Jackson","title":"Wanna Be Startin' Somethin'","lifecycleState":"Terminal","terminalOutcome":"Failed"}}`)
	for _, e := range *got {
		if e.State == StateFailed {
			t.Fatalf("an album retry showed as failed: %+v", e)
		}
	}
	if tr.state[0] != StateDone || tr.state[1] != StateSearching {
		t.Fatalf("states: %v", tr.state)
	}
}

func TestFindDoesNotGuessBetweenTracks(t *testing.T) {
	tr, _ := newTestTracker(
		spotify.Track{Artist: "Michael Jackson", Title: "Thriller"},
		spotify.Track{Artist: "Vincent Price", Title: "Vincent Price Excerpt From \"Thriller\" Voice-Over Session"},
		spotify.Track{Artist: "Michael Jackson", Title: "Billie Jean"},
		spotify.Track{Artist: "Michael Jackson", Title: "Billie Jean - Underground Mix"},
		spotify.Track{Artist: "Michael Jackson", Title: "Beat It - Single Version"},
	)
	cases := []struct {
		title, file string
		want        int
	}{
		{"Thriller", "", 0},
		{"04 - Thriller", `M\Thriller 25\04 - Thriller.mp3`, 0},
		{"Billie Jean (Underground Mix)", "", 3},
		{"Beat It", `x\05 - Beat It.mp3`, 4},
		// Contains both "thriller" and "billiejean": no guess.
		{"Thriller Billie Jean Medley", "", -1},
	}
	for _, c := range cases {
		if got := tr.findFile("Michael Jackson", c.title, c.file); got != c.want {
			t.Errorf("%q: got %d, want %d", c.title, got, c.want)
		}
	}
}

func TestWalkAudioSkipsTheFailedFolder(t *testing.T) {
	dir := t.TempDir()
	for _, p := range []string{"Album/01 - A.mp3", "failed/Album/02 - B.mp3", "Album/cover.jpg"} {
		full := filepath.Join(dir, p)
		_ = os.MkdirAll(filepath.Dir(full), 0o755)
		_ = os.WriteFile(full, []byte("x"), 0o644)
	}
	if got := walkAudio(dir); len(got) != 1 {
		t.Fatalf("want only the album file, got %v", got)
	}
}
