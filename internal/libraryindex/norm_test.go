package libraryindex

import "testing"

func TestNormalize(t *testing.T) {
	cases := map[string]string{
		"Café Tacvba":             "cafe tacvba",
		"Song (feat. Someone)":    "song",
		"Song - Remastered 2011":  "song",
		"AC/DC":                   "ac dc",
		"Björk & Friends":         "bjork and friends",
		"Título (Deluxe Edition)": "titulo",
		"  Multiple   Spaces  ":   "multiple spaces",
		"Song (Live at Wembley)":  "song live at wembley",
		"Şebnem Ferah":            "sebnem ferah",
	}
	for in, want := range cases {
		if got := Normalize(in); got != want {
			t.Errorf("Normalize(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestParseFileName(t *testing.T) {
	artist, title := ParseFileName("Daft Punk - Around the World")
	if artist != "Daft Punk" || title != "Around the World" {
		t.Errorf("got %q / %q", artist, title)
	}
	artist, title = ParseFileName("NoSeparator")
	if artist != "" || title != "NoSeparator" {
		t.Errorf("got %q / %q", artist, title)
	}
}

func TestTitleMatches(t *testing.T) {
	if !TitleMatches(Normalize("Around the World"), Normalize("Around the World (Extended Mix)")) {
		t.Error("expected version suffix to match")
	}
	if TitleMatches(Normalize("Song A"), Normalize("Song B")) {
		t.Error("different titles must not match")
	}
}
