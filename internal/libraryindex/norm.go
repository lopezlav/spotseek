package libraryindex

import (
	"regexp"
	"strings"
	"unicode"
)

var featPatterns = []*regexp.Regexp{
	regexp.MustCompile(`(?i)\s*[\(\[][^)\]]*(?:feat|ft|with)\.?[^)\]]*[\)\]]`),
	regexp.MustCompile(`(?i)\s+-\s+(?:feat|ft)\.?\s+.*$`),
}

var remasterPatterns = []*regexp.Regexp{
	regexp.MustCompile(`(?i)\s*[\(\[][^)\]]*(?:remaster|deluxe|anniversary|edition)[^)\]]*[\)\]]`),
	regexp.MustCompile(`(?i)\s+-\s+(?:remaster(?:ed)?|deluxe|anniversary)\b.*$`),
}

var deaccent = strings.NewReplacer(
	"á", "a", "à", "a", "â", "a", "ä", "a", "ã", "a", "å", "a", "ā", "a", "ă", "a", "ą", "a",
	"é", "e", "è", "e", "ê", "e", "ë", "e", "ē", "e", "ĕ", "e", "ė", "e", "ę", "e", "ě", "e",
	"í", "i", "ì", "i", "î", "i", "ï", "i", "ī", "i", "ĭ", "i", "į", "i",
	"ó", "o", "ò", "o", "ô", "o", "ö", "o", "õ", "o", "ø", "o", "ō", "o", "ŏ", "o", "ő", "o",
	"ú", "u", "ù", "u", "û", "u", "ü", "u", "ū", "u", "ŭ", "u", "ů", "u", "ų", "u", "ű", "u",
	"ñ", "n", "ń", "n", "ņ", "n", "ň", "n",
	"ç", "c", "ć", "c", "č", "c", "ĉ", "c",
	"ý", "y", "ÿ", "y", "ŷ", "y",
	"ß", "ss", "æ", "ae", "œ", "oe", "š", "s", "ś", "s", "ş", "s", "ž", "z", "ź", "z", "ż", "z",
	"ğ", "g", "ĝ", "g", "ġ", "g", "ř", "r", "ŕ", "r", "ł", "l", "ľ", "l", "ď", "d", "đ", "d",
	"ť", "t", "ţ", "t", "ð", "d", "þ", "th",
)

// Normalize folds a text for comparison: lowercase, no accents, no
// punctuation, no feat/remaster noise, single spaces.
func Normalize(s string) string {
	s = strings.ToLower(strings.TrimSpace(s))
	s = strings.ReplaceAll(s, "&", " and ")
	s = deaccent.Replace(s)
	for _, re := range featPatterns {
		s = re.ReplaceAllString(s, " ")
	}
	for _, re := range remasterPatterns {
		s = re.ReplaceAllString(s, " ")
	}
	var b strings.Builder
	b.Grow(len(s))
	space := true
	for _, r := range s {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			b.WriteRune(r)
			space = false
			continue
		}
		if !space {
			b.WriteByte(' ')
			space = true
		}
	}
	return strings.TrimSpace(b.String())
}

// TrackKey is the fallback identity of a song when no Spotify id is known.
func TrackKey(artist, title string) string {
	return Normalize(artist) + "|" + Normalize(title)
}

// TitleKey is the identity of a song by title only.
func TitleKey(title string) string {
	return Normalize(title)
}

// ParseFileName splits "Artist - Title" file names. It also accepts an
// en dash and an underscore dash. Without a separator it returns the whole
// base as the title.
func ParseFileName(base string) (artist, title string) {
	base = strings.TrimSpace(base)
	for _, sep := range []string{" - ", " – ", " — "} {
		if i := strings.Index(base, sep); i > 0 {
			return strings.TrimSpace(base[:i]), strings.TrimSpace(base[i+len(sep):])
		}
	}
	return "", base
}

// TitleMatches reports whether two normalized titles refer to the same song.
// It allows one side to contain the other to survive version suffixes like
// "Song (Extended Mix)" in a file name.
func TitleMatches(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	if a == b {
		return true
	}
	if len(a) >= 6 && strings.Contains(b, a) {
		return true
	}
	if len(b) >= 6 && strings.Contains(a, b) {
		return true
	}
	return false
}

// ArtistMatches reports whether two normalized artists refer to the same
// artist. It allows one side to contain the other so "Artist" matches
// "Artist feat. X".
func ArtistMatches(a, b string) bool {
	if a == "" || b == "" {
		return true
	}
	if a == b {
		return true
	}
	if len(a) >= 4 && strings.Contains(b, a) {
		return true
	}
	if len(b) >= 4 && strings.Contains(a, b) {
		return true
	}
	return false
}

// DurationClose reports whether two durations (seconds) are close enough to
// be the same recording. A zero duration means unknown and always matches.
func DurationClose(a, b int) bool {
	if a == 0 || b == 0 {
		return true
	}
	diff := a - b
	if diff < 0 {
		diff = -diff
	}
	return diff <= 5
}
