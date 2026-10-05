package pipeline

import (
	"context"

	"spotseek-app/internal/ytdlp"
)

type fallbackSearch struct {
	track      missingTrack
	cancel     context.CancelFunc
	done       chan struct{}
	candidates []ytdlp.Candidate
	err        error
}

func startFallbackSearch(ctx context.Context, track missingTrack) *fallbackSearch {
	ctx, cancel := context.WithCancel(ctx)
	s := &fallbackSearch{track: track, cancel: cancel, done: make(chan struct{})}
	go func() {
		defer close(s.done)
		s.candidates, s.err = ytdlp.SearchCandidates(ctx, track.Artist, track.Title, track.Duration, 5)
	}()
	return s
}

func (s *fallbackSearch) get(ctx context.Context) ([]ytdlp.Candidate, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-s.done:
		return s.candidates, s.err
	}
}

func (s *fallbackSearch) stop() {
	s.cancel()
	<-s.done
}
