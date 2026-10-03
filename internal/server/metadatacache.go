// AI-assisted code: persistent account metadata and shared background refreshes.
package server

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

const metadataMaxAge = 5 * time.Minute

type accountIdentity struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type metadataEntry struct {
	Data     json.RawMessage `json:"data"`
	Revision string          `json:"revision"`
	LastSync time.Time       `json:"last_sync"`
}

type metadataDisk struct {
	Version  int                        `json:"version"`
	Sessions map[string]accountIdentity `json:"sessions"`
	Entries  map[string]metadataEntry   `json:"entries"`
}

type metadataFlight struct {
	done  chan struct{}
	entry metadataEntry
	err   error
}

type metadataCache struct {
	mu        sync.Mutex
	wg        sync.WaitGroup
	path      string
	disk      metadataDisk
	flights   map[string]*metadataFlight
	attempted map[string]time.Time
}

func newMetadataCache(path string) (*metadataCache, error) {
	c := &metadataCache{path: path, disk: metadataDisk{Version: 1, Sessions: map[string]accountIdentity{}, Entries: map[string]metadataEntry{}}, flights: map[string]*metadataFlight{}, attempted: map[string]time.Time{}}
	raw, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return c, nil
	}
	if err != nil {
		return c, err
	}
	var saved metadataDisk
	if err := json.Unmarshal(raw, &saved); err != nil {
		return c, err
	}
	if saved.Version != 1 {
		return c, fmt.Errorf("unsupported metadata cache version %d", saved.Version)
	}
	if saved.Sessions != nil {
		c.disk.Sessions = saved.Sessions
	}
	if saved.Entries != nil {
		c.disk.Entries = saved.Entries
	}
	return c, nil
}

func (c *metadataCache) saveLocked() error {
	raw, err := json.Marshal(c.disk)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(c.path), 0700); err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(c.path), ".metadata-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if err := f.Chmod(0600); err != nil {
		f.Close()
		return err
	}
	if _, err := f.Write(raw); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), c.path)
}

func (c *metadataCache) identity(session string) accountIdentity {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.disk.Sessions[session]
}

func (c *metadataCache) saveIdentity(session string, identity accountIdentity) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if session == "" || identity.ID == "" {
		return fmt.Errorf("cannot save an empty account identity")
	}
	previous, existed := c.disk.Sessions[session]
	c.disk.Sessions[session] = identity
	if err := c.saveLocked(); err != nil {
		if existed {
			c.disk.Sessions[session] = previous
		} else {
			delete(c.disk.Sessions, session)
		}
		return err
	}
	return nil
}

func (c *metadataCache) setPath(path string) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	previous := c.path
	c.path = path
	if err := c.saveLocked(); err != nil {
		c.path = previous
		return err
	}
	return nil
}

func metadataKey(account, kind, id string) string {
	parts, _ := json.Marshal([]string{account, kind, id})
	return string(parts)
}

// fetch shares one bounded read for all clients, including explicit refreshes.
func (c *metadataCache) fetch(ctx context.Context, key string, loader func(context.Context) (any, error)) *metadataFlight {
	c.mu.Lock()
	defer c.mu.Unlock()
	if f := c.flights[key]; f != nil {
		return f
	}
	f := &metadataFlight{done: make(chan struct{})}
	c.flights[key] = f
	c.attempted[key] = time.Now()
	c.wg.Add(1)
	go func() {
		defer c.wg.Done()
		loadCtx, cancel := context.WithTimeout(ctx, 90*time.Second)
		defer cancel()
		data, err := loader(loadCtx)
		var entry metadataEntry
		if err == nil {
			var raw []byte
			raw, err = json.Marshal(data)
			if err == nil {
				entry = metadataEntry{Data: raw, Revision: fmt.Sprintf("%x", sha256.Sum256(raw)), LastSync: time.Now().UTC()}
			}
		}
		c.mu.Lock()
		defer c.mu.Unlock()
		if err == nil {
			old, existed := c.disk.Entries[key]
			c.disk.Entries[key] = entry
			err = c.saveLocked()
			if err != nil {
				if existed {
					c.disk.Entries[key] = old
				} else {
					delete(c.disk.Entries, key)
				}
			}
		}
		f.entry, f.err = entry, err
		delete(c.flights, key)
		close(f.done)
	}()
	return f
}

func (c *metadataCache) get(ctx context.Context, key string, force bool, loader func(context.Context) (any, error), requestCtx context.Context) (metadataEntry, bool, error) {
	c.mu.Lock()
	entry, found := c.disk.Entries[key]
	attempted := c.attempted[key]
	c.mu.Unlock()
	if found && !force {
		if time.Since(entry.LastSync) > metadataMaxAge && time.Since(attempted) > metadataMaxAge {
			c.fetch(ctx, key, loader)
		}
		return entry, true, nil
	}
	f := c.fetch(ctx, key, loader)
	select {
	case <-f.done:
		return f.entry, false, f.err
	case <-requestCtx.Done():
		return metadataEntry{}, false, requestCtx.Err()
	}
}

// End AI-assisted code.
