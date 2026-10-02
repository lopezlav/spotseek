package spotify

import (
	"context"
	"fmt"
)

type apiTrack struct {
	Name       string      `json:"name"`
	DurationMS int         `json:"duration_ms"`
	Artists    []artistRef `json:"artists"`
}

func (t apiTrack) toTrack() Track {
	artist := ""
	if len(t.Artists) > 0 {
		artist = firstArtist(t.Artists[0].Name)
	}
	return Track{Artist: artist, Title: t.Name, DurationSec: t.DurationMS / 1000}
}

// LibraryItem is a playlist or album shown in the app.
type LibraryItem struct {
	ID       string
	Name     string
	Subtitle string
	Count    int
	Kind     string // "playlist" or "album"
}

type meResponse struct {
	DisplayName string `json:"display_name"`
	ID          string `json:"id"`
}

// Profile returns the signed-in user's display name.
func (a *Auth) Profile(ctx context.Context) (string, error) {
	var me meResponse
	if err := a.get(ctx, apiBase+"/me", &me); err != nil {
		return "", err
	}
	if me.DisplayName == "" {
		return me.ID, nil
	}
	return me.DisplayName, nil
}

type playlistsPage struct {
	Items []struct {
		ID    string `json:"id"`
		Name  string `json:"name"`
		Owner struct {
			DisplayName string `json:"display_name"`
		} `json:"owner"`
		Tracks struct {
			Total int `json:"total"`
		} `json:"tracks"`
	} `json:"items"`
	Next string `json:"next"`
}

// Playlists returns the user's playlists and followed playlists.
func (a *Auth) Playlists(ctx context.Context) ([]LibraryItem, error) {
	var out []LibraryItem
	url := apiBase + "/me/playlists?limit=50"
	for url != "" {
		var page playlistsPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, p := range page.Items {
			if p.ID == "" {
				continue
			}
			out = append(out, LibraryItem{
				ID:       p.ID,
				Name:     p.Name,
				Subtitle: p.Owner.DisplayName,
				Count:    p.Tracks.Total,
				Kind:     "playlist",
			})
		}
		url = page.Next
	}
	return out, nil
}

type albumsPage struct {
	Items []struct {
		Album struct {
			ID          string      `json:"id"`
			Name        string      `json:"name"`
			TotalTracks int         `json:"total_tracks"`
			Artists     []artistRef `json:"artists"`
		} `json:"album"`
	} `json:"items"`
	Next string `json:"next"`
}

// Albums returns the user's saved albums.
func (a *Auth) Albums(ctx context.Context) ([]LibraryItem, error) {
	var out []LibraryItem
	url := apiBase + "/me/albums?limit=50"
	for url != "" {
		var page albumsPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, it := range page.Items {
			al := it.Album
			if al.ID == "" {
				continue
			}
			artist := ""
			if len(al.Artists) > 0 {
				artist = firstArtist(al.Artists[0].Name)
			}
			out = append(out, LibraryItem{
				ID:       al.ID,
				Name:     al.Name,
				Subtitle: artist,
				Count:    al.TotalTracks,
				Kind:     "album",
			})
		}
		url = page.Next
	}
	return out, nil
}

type likedPage struct {
	Items []struct {
		Track apiTrack `json:"track"`
	} `json:"items"`
	Next  string `json:"next"`
	Total int    `json:"total"`
}

// LikedTracks returns the user's liked songs.
func (a *Auth) LikedTracks(ctx context.Context) ([]Track, error) {
	var out []Track
	url := apiBase + "/me/tracks?limit=50"
	for url != "" {
		var page likedPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, it := range page.Items {
			if it.Track.Name != "" {
				out = append(out, it.Track.toTrack())
			}
		}
		url = page.Next
	}
	return out, nil
}

type playlistItemsPage struct {
	Items []struct {
		Item  *apiTrack `json:"item"`
		Track *apiTrack `json:"track"`
	} `json:"items"`
	Next string `json:"next"`
}

// PlaylistTracks returns all tracks of a playlist (works for private playlists).
func (a *Auth) PlaylistTracks(ctx context.Context, id string) ([]Track, error) {
	var out []Track
	url := fmt.Sprintf("%s/playlists/%s/items?limit=100", apiBase, id)
	for url != "" {
		var page playlistItemsPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, it := range page.Items {
			t := it.Item
			if t == nil {
				t = it.Track
			}
			if t != nil && t.Name != "" {
				out = append(out, t.toTrack())
			}
		}
		url = page.Next
	}
	return out, nil
}

type albumTracksPage struct {
	Items []apiTrack `json:"items"`
	Next  string     `json:"next"`
}

// AlbumTracks returns all tracks of an album.
func (a *Auth) AlbumTracks(ctx context.Context, id string) ([]Track, error) {
	var out []Track
	url := fmt.Sprintf("%s/albums/%s/tracks?limit=50", apiBase, id)
	for url != "" {
		var page albumTracksPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, t := range page.Items {
			if t.Name != "" {
				out = append(out, t.toTrack())
			}
		}
		url = page.Next
	}
	return out, nil
}
