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
	if tr.src[0] != filepath.Clean("/w/soulseek/Thriller.mp3") {
		t.Fatalf("download path not kept: %q", tr.src[0])
	}
	// Outside album mode, a terminal failure of another track is a failure.
	tr2, got2 := newTestTracker(spotify.Track{Artist: "Enya", Title: "Only Time"})
	tr2.handle(`{"type":"track_state","data":{"artist":"Enya","title":"Only Time","lifecycleState":"Terminal","terminalOutcome":"Failed"}}`)
	if len(*got2) != 1 || (*got2)[0].State != StateFailed {
		t.Fatalf("a terminal failure is a failure: %+v", *got2)
	}
}

func TestAssignFilesUsesTheBestMatches(t *testing.T) {
	tr, _ := newTestTracker(
		spotify.Track{Artist: "Michael Jackson", Title: "Thriller"},
		spotify.Track{Artist: "Michael Jackson", Title: "P.Y.T. (Pretty Young Thing)"},
		spotify.Track{Artist: "Michael Jackson", Title: "P.Y.T. (Pretty Young Thing) (2008 with will.i.am) (with will.i.am) - Thriller 25th Anniversary Remix"},
		spotify.Track{Artist: "Michael Jackson", Title: "Billie Jean"},
		spotify.Track{Artist: "Michael Jackson", Title: "Billie Jean - Underground Mix"},
		spotify.Track{Artist: "Michael Jackson", Title: "Thriller - Instrumental"},
		spotify.Track{Artist: "Enya", Title: "Only Time"},
	)
	tr.src[6] = filepath.Clean("/w/x/whatever.mp3") // as the tracker stores it
	// The real folder from the Thriller 25 run, in its own order.
	dir := "/w/soulseek/Thriller 25/"
	got := tr.assignFiles([]string{
		dir + "04 - Thriller.mp3",
		dir + "06 - Billie Jean.mp3",
		dir + "08 - P.Y.T. (Pretty Young Thing).mp3",
		dir + "12 - P.Y.T. (Pretty Young Thing) 2008 with will.i.am (Thriller 25th Anniversary Remix feat. willi.i.am).mp3",
		dir + "20 - Billie Jean (Home Demo from 1981).mp3",
		dir + "29 - Billie Jean (Underground Mix).mp3",
		dir + "30 - P.Y.T. (Pretty Young Thing) (2008 with will.i.am) (with will.i.am) - Thriller 25th Anniversary Remix.mp3",
		dir + "30 - Thriller (Instrumental).mp3",
		"/w/x/whatever.mp3",
	})
	want := map[string]int{
		dir + "04 - Thriller.mp3":                      0,
		dir + "06 - Billie Jean.mp3":                   3,
		dir + "08 - P.Y.T. (Pretty Young Thing).mp3":   1,
		dir + "29 - Billie Jean (Underground Mix).mp3": 4,
		dir + "30 - P.Y.T. (Pretty Young Thing) (2008 with will.i.am) (with will.i.am) - Thriller 25th Anniversary Remix.mp3": 2,
		dir + "30 - Thriller (Instrumental).mp3": 5,
		"/w/x/whatever.mp3":                      6,
	}
	for f, i := range want {
		if g, ok := got[f]; !ok || g != i {
			t.Errorf("%s: got %d (%v), want %d", f, g, ok, i)
		}
	}
	if len(got) != len(want) {
		t.Errorf("extra files were given a track: %v", got)
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
