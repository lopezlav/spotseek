// AI-assisted code: typed catalogue search and safe API errors.
package spotify

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"unicode/utf8"
)

type APIError struct {
	Status     int
	RetryAfter int
	Playlist   bool
}

func (e *APIError) Error() string {
	switch e.Status {
	case http.StatusBadRequest:
		return "Spotify could not read this search. Check the name or search filters."
	case http.StatusUnauthorized:
		return "Spotify login has expired. Log in again."
	case http.StatusForbidden:
		if e.Playlist {
			return "Spotify does not allow access to this playlist. Your account must own it or be a collaborator. The saved list is kept."
		}
		return "Spotify does not allow this request for your account or app."
	case http.StatusTooManyRequests:
		return fmt.Sprintf("Spotify is busy. Try again in %d seconds.", e.RetryAfter)
	case http.StatusNotFound:
		return "This item is no longer available on Spotify."
	default:
		return "Spotify could not complete the request. Try again later."
	}
}

type SearchRequest struct {
	Query  string
	Kind   string
	Offset int
	Limit  int
}

func (r SearchRequest) Validate() error {
	if strings.TrimSpace(r.Query) == "" || utf8.RuneCountInString(r.Query) > 500 {
		return fmt.Errorf("Enter a search with 1 to 500 characters.")
	}
	if r.Kind != "track" && r.Kind != "album" && r.Kind != "artist" {
		return fmt.Errorf("Search for songs, albums, or artists.")
	}
	if r.Offset < 0 || r.Offset > 1000 || r.Limit < 1 || r.Limit > 10 {
		return fmt.Errorf("Search page is out of range.")
	}
	return nil
}

type SearchPage struct {
	Tracks     []Track
	Items      []LibraryItem
	NextOffset int
}

func (a *Auth) Search(ctx context.Context, request SearchRequest) (SearchPage, error) {
	if err := request.Validate(); err != nil {
		return SearchPage{}, err
	}
	params := url.Values{"q": {strings.TrimSpace(request.Query)}, "type": {request.Kind}, "limit": {strconv.Itoa(request.Limit)}, "offset": {strconv.Itoa(request.Offset)}}
	type trackPage struct {
		Items  []apiTrack `json:"items"`
		Next   string     `json:"next"`
		Offset int        `json:"offset"`
		Limit  int        `json:"limit"`
	}
	type itemPage struct {
		Items []struct {
			ID          string      `json:"id"`
			Name        string      `json:"name"`
			Artists     []artistRef `json:"artists"`
			TotalTracks int         `json:"total_tracks"`
		} `json:"items"`
		Next   string `json:"next"`
		Offset int    `json:"offset"`
		Limit  int    `json:"limit"`
	}
	var result struct {
		Tracks  *trackPage `json:"tracks"`
		Albums  *itemPage  `json:"albums"`
		Artists *itemPage  `json:"artists"`
	}
	if err := a.get(ctx, apiBase+"/search?"+params.Encode(), &result); err != nil {
		return SearchPage{}, err
	}
	page := SearchPage{Tracks: []Track{}, Items: []LibraryItem{}, NextOffset: -1}
	next := ""
	if request.Kind == "track" {
		if result.Tracks == nil {
			return page, fmt.Errorf("Spotify returned an incomplete search result.")
		}
		next = result.Tracks.Next
		for _, t := range result.Tracks.Items {
			if t.ID != "" {
				page.Tracks = append(page.Tracks, t.toTrack())
			}
		}
	} else {
		items := result.Albums
		if request.Kind == "artist" {
			items = result.Artists
		}
		if items == nil {
			return page, fmt.Errorf("Spotify returned an incomplete search result.")
		}
		next = items.Next
		for _, item := range items.Items {
			if item.ID == "" {
				continue
			}
			subtitle := ""
			if len(item.Artists) > 0 {
				subtitle = item.Artists[0].Name
			}
			page.Items = append(page.Items, LibraryItem{ID: item.ID, Name: item.Name, Subtitle: subtitle, Kind: request.Kind, Count: item.TotalTracks})
		}
	}
	if next != "" {
		u, err := url.Parse(next)
		if err != nil {
			return page, fmt.Errorf("Spotify returned an invalid search page.")
		}
		offset, err := strconv.Atoi(u.Query().Get("offset"))
		if err != nil || offset <= request.Offset {
			return page, fmt.Errorf("Spotify returned an invalid search page.")
		}
		if offset <= 1000 {
			page.NextOffset = offset
		}
	}
	return page, nil
}

// End AI-assisted code.
