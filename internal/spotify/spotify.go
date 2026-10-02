package spotify

import (
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"regexp"
	"strings"
	"sync"
	"time"
)

const userAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131 Safari/537.36"

const b62Alphabet = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"

// Track is one song with the data Sockseek needs.
type Track struct {
	Artist      string
	Title       string
	DurationSec int
}

var httpClient = &http.Client{Timeout: 45 * time.Second}

func httpGet(url string, headers map[string]string) (string, error) {
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("User-Agent", userAgent)
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", err
	}
	if resp.StatusCode != 200 {
		return "", fmt.Errorf("%s returned HTTP %d", url, resp.StatusCode)
	}
	return string(body), nil
}

func detect(input string) (kind, id string, err error) {
	re := regexp.MustCompile(`/(playlist|album|track)/([A-Za-z0-9]+)`)
	if m := re.FindStringSubmatch(input); m != nil {
		return m[1], m[2], nil
	}
	if regexp.MustCompile(`^[A-Za-z0-9]+$`).MatchString(input) {
		return "playlist", input, nil
	}
	return "", "", fmt.Errorf("cannot read a Spotify id from %q", input)
}

type nextData struct {
	Props struct {
		PageProps struct {
			State struct {
				Data struct {
					Entity entity `json:"entity"`
				} `json:"data"`
				Settings struct {
					Session struct {
						AccessToken string `json:"accessToken"`
					} `json:"session"`
				} `json:"settings"`
			} `json:"state"`
		} `json:"pageProps"`
	} `json:"props"`
}

type artistRef struct {
	Name string `json:"name"`
}

type trackItem struct {
	URI      string      `json:"uri"`
	Title    string      `json:"title"`
	Name     string      `json:"name"`
	Subtitle string      `json:"subtitle"`
	Duration int         `json:"duration"`
	Artists  []artistRef `json:"artists"`
}

type entity struct {
	Type      string      `json:"type"`
	Name      string      `json:"name"`
	Title     string      `json:"title"`
	URI       string      `json:"uri"`
	Subtitle  string      `json:"subtitle"`
	Duration  int         `json:"duration"`
	TrackList []trackItem `json:"trackList"`
	Artists   []artistRef `json:"artists"`
}

func nextDataOf(html string) (*nextData, error) {
	re := regexp.MustCompile(`(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>`)
	m := re.FindStringSubmatch(html)
	if m == nil {
		return nil, fmt.Errorf("no __NEXT_DATA__ in page")
	}
	var nd nextData
	if err := json.Unmarshal([]byte(m[1]), &nd); err != nil {
		return nil, fmt.Errorf("bad __NEXT_DATA__: %w", err)
	}
	return &nd, nil
}

func embed(kind, id string) (*entity, string, error) {
	html, err := httpGet("https://open.spotify.com/embed/"+kind+"/"+id, nil)
	if err != nil {
		return nil, "", err
	}
	nd, err := nextDataOf(html)
	if err != nil {
		return nil, "", err
	}
	return &nd.Props.PageProps.State.Data.Entity, nd.Props.PageProps.State.Settings.Session.AccessToken, nil
}

func firstArtist(name string) string {
	parts := strings.Split(name, ",")
	return strings.TrimSpace(parts[0])
}

func trackFromItem(it trackItem) Track {
	artist := firstArtist(it.Subtitle)
	if len(it.Artists) > 0 {
		artist = firstArtist(it.Artists[0].Name)
	}
	title := it.Title
	if title == "" {
		title = it.Name
	}
	return Track{Artist: artist, Title: title, DurationSec: it.Duration / 1000}
}

func trackFromEntity(e *entity) Track {
	artist := firstArtist(e.Subtitle)
	if len(e.Artists) > 0 {
		artist = firstArtist(e.Artists[0].Name)
	}
	title := e.Name
	if title == "" {
		title = e.Title
	}
	return Track{Artist: artist, Title: title, DurationSec: e.Duration / 1000}
}

func itemID(uri string) string {
	if strings.HasPrefix(uri, "spotify:track:") {
		return strings.TrimPrefix(uri, "spotify:track:")
	}
	return ""
}

func getTrack(id string) (Track, error) {
	e, _, err := embed("track", id)
	if err != nil {
		return Track{}, err
	}
	return trackFromEntity(e), nil
}

func b62ToHex(s string) (string, error) {
	n := new(big.Int)
	for _, c := range s {
		idx := strings.IndexRune(b62Alphabet, c)
		if idx < 0 {
			return "", fmt.Errorf("invalid base62 character %q", c)
		}
		n.Mul(n, big.NewInt(62))
		n.Add(n, big.NewInt(int64(idx)))
	}
	return fmt.Sprintf("%032x", n), nil
}

func hexToB62(h string) (string, error) {
	n := new(big.Int)
	if _, ok := n.SetString(h, 16); !ok {
		return "", fmt.Errorf("invalid hex %q", h)
	}
	if n.Sign() == 0 {
		return string(b62Alphabet[0]), nil
	}
	base := big.NewInt(62)
	mod := new(big.Int)
	out := ""
	for n.Sign() > 0 {
		n.DivMod(n, base, mod)
		out = string(b62Alphabet[mod.Int64()]) + out
	}
	return out, nil
}

type spPlaylist struct {
	Length   int `json:"length"`
	Contents struct {
		Items []struct {
			URI string `json:"uri"`
		} `json:"items"`
	} `json:"contents"`
}

type albumMeta struct {
	Name string `json:"name"`
	Disc []struct {
		Track []struct {
			GID string `json:"gid"`
		} `json:"track"`
	} `json:"disc"`
}

func fetchTracksParallel(ids []string, workers int) map[string]Track {
	out := make(map[string]Track)
	var mu sync.Mutex
	sem := make(chan struct{}, workers)
	var wg sync.WaitGroup
	for _, id := range ids {
		wg.Add(1)
		go func(id string) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			t, err := getTrack(id)
			if err == nil {
				mu.Lock()
				out[id] = t
				mu.Unlock()
			}
		}(id)
	}
	wg.Wait()
	return out
}

func playlistTracks(id string) (string, []Track, error) {
	e, token, err := embed("playlist", id)
	if err != nil {
		return "", nil, err
	}
	name := e.Name
	byID := map[string]Track{}
	for _, it := range e.TrackList {
		if tid := itemID(it.URI); tid != "" {
			byID[tid] = trackFromItem(it)
		}
	}
	if token == "" {
		return name, ordered(byID, itemOrder(e)), nil
	}
	raw, err := httpGet("https://spclient.wg.spotify.com/playlist/v2/playlist/"+id,
		map[string]string{"Authorization": "Bearer " + token, "Accept": "application/json"})
	if err != nil {
		return name, ordered(byID, itemOrder(e)), nil
	}
	var pl spPlaylist
	if err := json.Unmarshal([]byte(raw), &pl); err != nil {
		return name, ordered(byID, itemOrder(e)), nil
	}
	var ids []string
	for _, it := range pl.Contents.Items {
		if tid := itemID(it.URI); tid != "" {
			ids = append(ids, tid)
		}
	}
	var missing []string
	for _, tid := range ids {
		if _, ok := byID[tid]; !ok {
			missing = append(missing, tid)
		}
	}
	for tid, t := range fetchTracksParallel(missing, 8) {
		byID[tid] = t
	}
	return name, ordered(byID, ids), nil
}

func itemOrder(e *entity) []string {
	var ids []string
	for _, it := range e.TrackList {
		if tid := itemID(it.URI); tid != "" {
			ids = append(ids, tid)
		}
	}
	return ids
}

func ordered(byID map[string]Track, ids []string) []Track {
	var out []Track
	for _, id := range ids {
		if t, ok := byID[id]; ok && t.Title != "" {
			out = append(out, t)
		}
	}
	return out
}

func albumTracks(id string) (string, string, []Track, error) {
	e, token, err := embed("album", id)
	if err != nil {
		return "", "", nil, err
	}
	name := e.Name
	artist := firstArtist(e.Subtitle)
	byID := map[string]Track{}
	for _, it := range e.TrackList {
		if tid := itemID(it.URI); tid != "" {
			byID[tid] = trackFromItem(it)
		}
	}
	if token == "" {
		return name, artist, ordered(byID, itemOrder(e)), nil
	}
	hexgid, err := b62ToHex(id)
	if err != nil {
		return name, artist, ordered(byID, itemOrder(e)), nil
	}
	raw, err := httpGet("https://spclient.wg.spotify.com/metadata/4/album/"+hexgid,
		map[string]string{"Authorization": "Bearer " + token, "Accept": "application/json"})
	if err != nil {
		return name, artist, ordered(byID, itemOrder(e)), nil
	}
	var meta albumMeta
	if err := json.Unmarshal([]byte(raw), &meta); err != nil {
		return name, artist, ordered(byID, itemOrder(e)), nil
	}
	var ids []string
	for _, disc := range meta.Disc {
		for _, t := range disc.Track {
			if b62, err := hexToB62(t.GID); err == nil {
				ids = append(ids, b62)
			}
		}
	}
	var missing []string
	for _, tid := range ids {
		if _, ok := byID[tid]; !ok {
			missing = append(missing, tid)
		}
	}
	for tid, t := range fetchTracksParallel(missing, 8) {
		byID[tid] = t
	}
	return name, artist, ordered(byID, ids), nil
}

// Collection is a resolved Spotify input.
type Collection struct {
	Kind   string // "playlist", "album", or "track"
	Name   string
	Artist string // album artist, used for album-mode search
	Tracks []Track
}

// Extract reads a public Spotify playlist, album, or track.
func Extract(input string) (*Collection, error) {
	kind, id, err := detect(input)
	if err != nil {
		return nil, err
	}
	switch kind {
	case "track":
		t, err := getTrack(id)
		if err != nil {
			return nil, err
		}
		return &Collection{Kind: "track", Name: t.Title, Artist: t.Artist, Tracks: []Track{t}}, nil
	case "album":
		name, artist, tracks, err := albumTracks(id)
		if err != nil {
			return nil, err
		}
		return &Collection{Kind: "album", Name: name, Artist: artist, Tracks: tracks}, nil
	default:
		name, tracks, err := playlistTracks(id)
		if err != nil {
			return nil, err
		}
		return &Collection{Kind: "playlist", Name: name, Tracks: tracks}, nil
	}
}
