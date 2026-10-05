package pipeline

import (
	"context"
	"testing"

	"spotseek-app/internal/timing"
)

func TestSoulseekTimingSingleTrackAndRetry(t *testing.T) {
	var events []timing.Event
	ctx := timing.WithObserver(context.Background(), func(e timing.Event) { events = append(events, e) })
	s := newSoulseekTiming(ctx)
	s.line(`{"type":"search_start","data":{"artist":"Artist","title":"Song"}}`)
	s.line(`{"type":"download_start","data":{"artist":"Artist","title":"Song"}}`)
	s.line(`{"type":"download_progress","data":{"jobId":"one","bytesTransferred":42}}`)
	s.line(`{"type":"download_progress","data":{"jobId":"one","bytesTransferred":84}}`)
	s.line(`{"type":"download_start","data":{"artist":"Artist","title":"Song"}}`)
	s.line(`{"type":"track_state","data":{"artist":"Artist","title":"Song","terminalOutcome":"Failed","failureReason":"AllDownloadsFailed"}}`)
	s.finish(nil)
	want := []string{"soulseek.startup_to_search", "soulseek.search_and_selection", "soulseek.time_to_first_progress", "soulseek.transfer", "soulseek.transfer"}
	if len(events) != len(want) {
		t.Fatalf("got events: %+v", events)
	}
	for i, stage := range want {
		if events[i].Stage != stage {
			t.Errorf("event %d: %s, want %s", i, events[i].Stage, stage)
		}
		if events[i].DurationMS < 0 {
			t.Errorf("negative duration: %+v", events[i])
		}
	}
	if events[len(events)-1].Error != "AllDownloadsFailed" {
		t.Fatalf("missing terminal failure: %+v", events)
	}
}

func TestSoulseekTimingDoesNotAttributeConcurrentProgress(t *testing.T) {
	var events []timing.Event
	ctx := timing.WithObserver(context.Background(), func(e timing.Event) { events = append(events, e) })
	s := newSoulseekTiming(ctx)
	s.line(`{"type":"download_start","data":{"artist":"A","title":"One"}}`)
	s.line(`{"type":"download_start","data":{"artist":"B","title":"Two"}}`)
	s.line(`{"type":"download_progress","data":{"jobId":"unknown","bytesTransferred":42}}`)
	if len(events) != 0 {
		t.Fatalf("ambiguous progress attributed: %+v", events)
	}
}
