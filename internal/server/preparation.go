package server

import (
	"net/http"
	"path/filepath"
	"time"

	"spotseek-app/internal/pipeline"
	"spotseek-app/internal/spotify"
)

type warmTrack struct {
	key         string
	expires     time.Time
	preparation *pipeline.PreparedTrack
}

func (m *Manager) PrepareTrack(track spotify.Track, account string) {
	m.prepareMu.Lock()
	defer m.prepareMu.Unlock()
	if m.index.Lookup(track).Found {
		return
	}
	m.mu.Lock()
	key := accountTrackKey(account, track)
	if m.warm != nil && m.warm.key == key && time.Now().Before(m.warm.expires) {
		m.mu.Unlock()
		return
	}
	old := m.warm
	m.warm = nil
	m.mu.Unlock()
	if old != nil {
		old.preparation.Stop()
	}
	// Publish the search while holding the queue lock so an immediate drop
	// either consumes it or starts first and prevents a competing login.
	m.mu.Lock()
	settings := *m.settings
	searchSoulseek := len(m.active) == 0 && time.Now().After(m.soulseekDownUntil)
	opts := &pipeline.Options{Bitrate: settings.Bitrate, UseFallback: settings.Fallback, SockseekPath: settings.SockseekPath, SoulseekUser: settings.SoulseekUser, SoulseekPass: settings.SoulseekPass,
		WorkDir: filepath.Join(settings.OutputDir, ".spotseek", "prepare", randomID(8))}
	p := pipeline.PrepareTrack(m.ctx, opts, track, searchSoulseek)
	warm := &warmTrack{key: key, expires: time.Now().Add(40 * time.Second), preparation: p}
	m.warm = warm
	m.mu.Unlock()
	time.AfterFunc(40*time.Second, func() {
		m.mu.Lock()
		expired := m.warm == warm
		if m.warm == warm {
			m.warm = nil
		}
		m.mu.Unlock()
		// A consumed preparation belongs to the active job and is stopped by
		// that pipeline. Its own deadline still bounds abandoned drag work.
		if expired {
			p.Stop()
		}
	})
}

func (s *Server) handlePrepareDownload(w http.ResponseWriter, r *http.Request) {
	var body trackDTO
	if err := readJSON(r, &body); err != nil || body.Title == "" {
		writeError(w, http.StatusBadRequest, "track metadata required")
		return
	}
	identity, err := s.ensureIdentity(r.Context(), false)
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	s.manager.PrepareTrack(body.toTrack(), identity.ID)
	writeJSON(w, http.StatusAccepted, map[string]bool{"ok": true})
}
