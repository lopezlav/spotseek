package pipeline

import (
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
	tr.handle(`{"type":"track_state","data":{"artist":"Michael Jackson","title":"Thriller","lifecycleState":"Terminal","terminalOutcome":"Failed"}}`)
	if (*got)[1].State != StateFailed {
		t.Fatalf("a terminal failure is a failure: %+v", (*got)[1])
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
