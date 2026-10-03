package spotify

import (
	"context"
	"fmt"
	"net/url"
)

type apiImage struct {
	URL    string `json:"url"`
	Width  int    `json:"width"`
	Height int    `json:"height"`
}

type apiAlbum struct {
	Name    string      `json:"name"`
	Artists []artistRef `json:"artists"`
	Images  []apiImage  `json:"images"`
}

type apiTrack struct {
	ID         string      `json:"id"`
	Name       string      `json:"name"`
	DurationMS int         `json:"duration_ms"`
	Artists    []artistRef `json:"artists"`
	Album      apiAlbum    `json:"album"`
}

func (t apiTrack) toTrack() Track {
	artist := ""
	if len(t.Artists) > 0 {
		artist = firstArtist(t.Artists[0].Name)
	} else if len(t.Album.Artists) > 0 {
		artist = firstArtist(t.Album.Artists[0].Name)
	}
	return Track{
		ID:          t.ID,
		Artist:      artist,
		Title:       t.Name,
		Album:       t.Album.Name,
		DurationSec: t.DurationMS / 1000,
	}
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

// AI-assisted code: return the stable account ID for local metadata storage.
func (a *Auth) Identity(ctx context.Context) (string, string, error) {
	var me meResponse
	if err := a.get(ctx, apiBase+"/me", &me); err != nil {
		return "", "", err
	}
	if me.ID == "" {
		return "", "", fmt.Errorf("Spotify did not return an account ID")
	}
	name := me.DisplayName
	if name == "" {
		name = me.ID
	}
	return me.ID, name, nil
}

// End AI-assisted code.

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
		// Spotify renamed "tracks" to "items" (February 2026). "items" is
		// absent for playlists the user does not own or collaborate on.
		Items *struct {
			Total int `json:"total"`
		} `json:"items"`
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
			count := p.Tracks.Total
			if p.Items != nil {
				count = p.Items.Total
			}
			out = append(out, LibraryItem{
				ID:       p.ID,
				Name:     p.Name,
				Subtitle: p.Owner.DisplayName,
				Count:    count,
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
	url := fmt.Sprintf("%s/playlists/%s/items?limit=50", apiBase, id)
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

// AlbumTracks returns all tracks of an album. The album name is copied into
// every track because the tracks endpoint does not include it.
func (a *Auth) AlbumTracks(ctx context.Context, id string) ([]Track, error) {
	name, artist, err := a.albumInfo(ctx, id)
	if err != nil {
		return nil, err
	}
	var out []Track
	url := fmt.Sprintf("%s/albums/%s/tracks?limit=50", apiBase, id)
	for url != "" {
		var page albumTracksPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, t := range page.Items {
			if t.Name != "" {
				track := t.toTrack()
				track.Album = name
				if track.Artist == "" {
					track.Artist = artist
				}
				out = append(out, track)
			}
		}
		url = page.Next
	}
	return out, nil
}

type albumInfoResponse struct {
	Name    string      `json:"name"`
	Artists []artistRef `json:"artists"`
}

// AlbumInfo returns the album name and main artist. Errors are ignored; the
// caller gets empty strings.
// AI-assisted code: preserve the GUI wrapper and report errors to full sync reads.
func (a *Auth) AlbumInfo(ctx context.Context, id string) (string, string) {
	name, artist, _ := a.albumInfo(ctx, id)
	return name, artist
}

func (a *Auth) albumInfo(ctx context.Context, id string) (string, string, error) {
	var info albumInfoResponse
	if err := a.get(ctx, fmt.Sprintf("%s/albums/%s?fields=name,artists", apiBase, id), &info); err != nil {
		return "", "", err
	}
	artist := ""
	if len(info.Artists) > 0 {
		artist = firstArtist(info.Artists[0].Name)
	}
	return info.Name, artist, nil
}

// End AI-assisted code.

// Track returns one track by id.
func (a *Auth) Track(ctx context.Context, id string) (Track, error) {
	var t apiTrack
	if err := a.get(ctx, fmt.Sprintf("%s/tracks/%s", apiBase, id), &t); err != nil {
		return Track{}, err
	}
	return t.toTrack(), nil
}

type followedArtistsPage struct {
	Artists struct {
		Items []struct {
			ID     string     `json:"id"`
			Name   string     `json:"name"`
			Images []apiImage `json:"images"`
		} `json:"items"`
		Next string `json:"next"`
	} `json:"artists"`
}

// Artists returns the artists the user follows.
func (a *Auth) Artists(ctx context.Context) ([]LibraryItem, error) {
	var out []LibraryItem
	url := apiBase + "/me/following?type=artist&limit=50"
	for url != "" {
		var page followedArtistsPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, ar := range page.Artists.Items {
			if ar.ID == "" {
				continue
			}
			out = append(out, LibraryItem{
				ID:   ar.ID,
				Name: ar.Name,
				Kind: "artist",
			})
		}
		url = page.Artists.Next
	}
	return out, nil
}

type searchArtistResponse struct {
	Artists struct {
		Items []struct {
			ID   string `json:"id"`
			Name string `json:"name"`
		} `json:"items"`
	} `json:"artists"`
}

// SearchArtist finds the Spotify id of an artist by name. It is used for
// artist names that come from saved albums when the follow scope is missing.
func (a *Auth) SearchArtist(ctx context.Context, name string) (string, error) {
	var resp searchArtistResponse
	url := fmt.Sprintf("%s/search?type=artist&limit=1&q=%s", apiBase, url.QueryEscape(name))
	if err := a.get(ctx, url, &resp); err != nil {
		return "", err
	}
	if len(resp.Artists.Items) == 0 {
		return "", fmt.Errorf("artist %q not found", name)
	}
	return resp.Artists.Items[0].ID, nil
}

type artistAlbumsPage struct {
	Items []struct {
		ID          string      `json:"id"`
		Name        string      `json:"name"`
		TotalTracks int         `json:"total_tracks"`
		ReleaseDate string      `json:"release_date"`
		Artists     []artistRef `json:"artists"`
	} `json:"items"`
	Next string `json:"next"`
}

// ArtistAlbums returns the albums and singles of an artist. Spotify limits
// this endpoint to 10 items per page.
func (a *Auth) ArtistAlbums(ctx context.Context, id string) ([]LibraryItem, error) {
	var out []LibraryItem
	url := fmt.Sprintf("%s/artists/%s/albums?include_groups=album,single&limit=10&market=from_token", apiBase, id)
	for url != "" {
		var page artistAlbumsPage
		if err := a.get(ctx, url, &page); err != nil {
			return nil, err
		}
		for _, al := range page.Items {
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

type topTracksResponse struct {
	Tracks []apiTrack `json:"tracks"`
}

// ArtistTopTracks returns an artist's most popular tracks. Spotify has
// deprecated this endpoint for some apps, so an error is not fatal.
func (a *Auth) ArtistTopTracks(ctx context.Context, id string) ([]Track, error) {
	var resp topTracksResponse
	if err := a.get(ctx, fmt.Sprintf("%s/artists/%s/top-tracks?market=from_token", apiBase, id), &resp); err != nil {
		return nil, err
	}
	var out []Track
	for _, t := range resp.Tracks {
		if t.Name != "" {
			out = append(out, t.toTrack())
		}
	}
	return out, nil
}

// ArtistAllTracks gathers the tracks of an artist's first albums. It is the
// fallback for the deprecated top-tracks endpoint.
func (a *Auth) ArtistAllTracks(ctx context.Context, id string, maxAlbums int) ([]Track, error) {
	albums, err := a.ArtistAlbums(ctx, id)
	if err != nil {
		return nil, err
	}
	seen := make(map[string]bool)
	var out []Track
	for i, al := range albums {
		if maxAlbums > 0 && i >= maxAlbums {
			break
		}
		tracks, err := a.AlbumTracks(ctx, al.ID)
		if err != nil {
			return nil, err
		}
		for _, t := range tracks {
			if t.ID != "" {
				if seen[t.ID] {
					continue
				}
				seen[t.ID] = true
			}
			if t.Album == "" {
				t.Album = al.Name
			}
			out = append(out, t)
		}
	}
	return out, nil
}
