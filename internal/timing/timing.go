// Package timing records monotonic elapsed times without changing download policy.
package timing

import (
	"context"
	"sync"
	"time"
)

type Event struct {
	Stage      string    `json:"stage"`
	StartedAt  time.Time `json:"started_at"`
	DurationMS float64   `json:"duration_ms"`
	Detail     string    `json:"detail,omitempty"`
	Error      string    `json:"error,omitempty"`
}

type observerKey struct{}

func WithObserver(ctx context.Context, observe func(Event)) context.Context {
	if observe != nil {
		callback := observe
		var mu sync.Mutex
		observe = func(e Event) {
			mu.Lock()
			defer mu.Unlock()
			callback(e)
		}
	}
	return context.WithValue(ctx, observerKey{}, observe)
}

func Enabled(ctx context.Context) bool {
	observe, _ := ctx.Value(observerKey{}).(func(Event))
	return observe != nil
}

func Interval(ctx context.Context, stage, detail string, start time.Time, err error) {
	observe, _ := ctx.Value(observerKey{}).(func(Event))
	if observe == nil {
		return
	}
	e := Event{Stage: stage, Detail: detail, StartedAt: start, DurationMS: float64(time.Since(start)) / float64(time.Millisecond)}
	if err != nil {
		e.Error = err.Error()
	}
	observe(e)
}

func Begin(ctx context.Context, stage, detail string) func(error) {
	start := time.Now()
	return func(err error) { Interval(ctx, stage, detail, start, err) }
}
