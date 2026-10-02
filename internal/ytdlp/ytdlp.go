package ytdlp

import (
	"context"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"
)

// Candidate is one YouTube search result with a relevance score.
type Candidate struct {
	Title    string
	Artist   string
	Album    string
	Duration int
	ID       string
	URL      string
	Score    float64
}

var badTerms = []string{
	"lyrics", "lyric", "letra", "subtitulad", "subtitulado", "traduccion",
	"reaction", "review", "tutorial", "cover", "karaoke", "instrumental",
	"remix", "sped up", "slowed", "nightcore", "8d", "live", "en vivo",
	"mashup", "type beat", "full album", "backstage",
}

var badVideoRe = regexp.MustCompile(`\b(official (music )?video|videoclip|video oficial)\b`)

var nonAlnum = regexp.MustCompile(`[^a-z0-9 ]+`)
var spaces = regexp.MustCompile(`\s+`)

func norm(s string) string {
	s = strings.Map(func(r rune) rune {
		if unicode.Is(unicode.Mn, r) {
			return -1
		}
		return unicode.ToLower(r)
	}, s)
	s = nonAlnum.ReplaceAllString(s, " ")
	return strings.TrimSpace(spaces.ReplaceAllString(s, " "))
}

func isBadVideo(videoTitle, wantTitle string) bool {
	t := norm(videoTitle)
	want := norm(wantTitle)
	for _, term := range badTerms {
		pat := regexp.MustCompile(`\b` + regexp.QuoteMeta(term) + `\b`)
		if pat.MatchString(t) && !pat.MatchString(want) {
			return true
		}
	}
	return badVideoRe.MatchString(t) && !strings.Contains(want, "official")
}

func score(c Candidate, title, artist string, length int) float64 {
	var s float64
	vt := norm(c.Title)
	va := norm(c.Artist)
	nt := norm(title)
	na := norm(artist)

	if nt != "" {
		if strings.Contains(vt, nt) {
			s += 4
		} else {
			all := true
			for _, w := range strings.Fields(nt) {
				if len([]rune(w)) > 2 && !strings.Contains(vt, w) {
					all = false
					break
				}
			}
			if all && len(strings.Fields(nt)) > 0 {
				s += 2
			}
		}
	}
	if na != "" && strings.Contains(va, na) {
		s += 3
	}
	if c.Album != "" {
		s += 1
	}
	if length > 0 && c.Duration > 0 {
		diff := c.Duration - length
		if diff < 0 {
			diff = -diff
		}
		switch {
		case diff <= 3:
			s += 4
		case diff <= 10:
			s += 2
		case diff > 45:
			s -= 4
		}
	}
	if isBadVideo(c.Title, title) {
		s -= 8
	}
	return s
}

// Binary is the yt-dlp executable. Override it to use a custom path.
var Binary = "yt-dlp"

func runYtDlp(ctx context.Context, timeout time.Duration, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	full := append([]string{"--no-warnings", "--ignore-config"}, args...)
	cmd := exec.CommandContext(ctx, Binary, full...)
	out, err := cmd.Output()
	if ctx.Err() == context.DeadlineExceeded {
		return "", ctx.Err()
	}
	return string(out), err
}

func parseRows(out string) [][]string {
	var rows [][]string
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimRight(line, "\r")
		if line != "" {
			rows = append(rows, strings.Split(line, "\t"))
		}
	}
	return rows
}

// SearchCandidates searches YouTube Music and YouTube, then scores the results.
func SearchCandidates(ctx context.Context, artist, title string, length, limit int) ([]Candidate, error) {
	q := strings.TrimSpace(artist + " " + title)
	seen := map[string]bool{}
	var ids []string

	add := func(out string) {
		for _, row := range parseRows(out) {
			id := strings.TrimSpace(row[0])
			if id != "" && !seen[id] {
				seen[id] = true
				ids = append(ids, id)
			}
		}
	}

	musicURL := "https://music.youtube.com/search?q=" + url.QueryEscape(q)
	out, _ := runYtDlp(ctx, 90*time.Second,
		"--skip-download", "--flat-playlist", "--print", "%(id)s",
		"--playlist-end", strconv.Itoa(limit), musicURL)
	add(out)

	out, _ = runYtDlp(ctx, 90*time.Second,
		"--skip-download", "--flat-playlist", "--print", "%(id)s",
		fmt.Sprintf("ytsearch%d:%s", limit, q))
	add(out)

	if len(ids) == 0 {
		return nil, nil
	}
	shortlist := ids
	if len(shortlist) > limit*2 {
		shortlist = shortlist[:limit*2]
	}
	var urls []string
	for _, id := range shortlist {
		urls = append(urls, "https://www.youtube.com/watch?v="+id)
	}
	args := []string{"--skip-download", "--print",
		"%(title)s\t%(artist)s\t%(album)s\t%(duration)s\t%(id)s\t%(channel)s"}
	args = append(args, urls...)
	out, _ = runYtDlp(ctx, 180*time.Second, args...)

	var cands []Candidate
	for _, row := range parseRows(out) {
		if len(row) < 6 {
			continue
		}
		dur, _ := strconv.Atoi(row[3])
		artist := row[1]
		if artist == "NA" {
			artist = row[5]
		}
		album := row[2]
		if album == "NA" {
			album = ""
		}
		cands = append(cands, Candidate{
			Title:    row[0],
			Artist:   artist,
			Album:    album,
			Duration: dur,
			ID:       row[4],
			URL:      "https://www.youtube.com/watch?v=" + row[4],
		})
	}
	for i := range cands {
		cands[i].Score = score(cands[i], title, artist, length)
	}
	sort.SliceStable(cands, func(i, j int) bool { return cands[i].Score > cands[j].Score })
	return cands, nil
}

// SafeName turns a string into a safe file name.
func SafeName(s string) string {
	re := regexp.MustCompile(`[\\/:*?"<>|]+`)
	s = re.ReplaceAllString(s, " ")
	return strings.TrimSpace(spaces.ReplaceAllString(s, " "))
}

// Download fetches one candidate as an mp3 file.
func Download(ctx context.Context, c Candidate, outdir, filename, quality string) error {
	if err := os.MkdirAll(outdir, 0o755); err != nil {
		return err
	}
	template := filepath.Join(outdir, filename+".%(ext)s")
	attempts := [][]string{
		{},
		{"--extractor-args", "youtube:player_client=default,web_safari"},
		{"--extractor-args", "youtube:player_client=web_safari,tv"},
	}
	var lastErr error
	for _, extra := range attempts {
		args := []string{"--no-warnings", "--ignore-config",
			"-f", "bestaudio/best",
			"-x", "--audio-format", "mp3", "--audio-quality", quality,
			"--embed-metadata", "--embed-thumbnail",
			"-o", template}
		args = append(args, extra...)
		args = append(args, c.URL)
		cmd := exec.CommandContext(ctx, Binary, args...)
		out, err := cmd.CombinedOutput()
		if err == nil {
			return nil
		}
		lastErr = fmt.Errorf("yt-dlp failed: %v: %s", err, string(out))
		if ctx.Err() != nil {
			return ctx.Err()
		}
	}
	return lastErr
}
