package pipeline

import (
	"strings"
	"testing"
)

func TestBaseTitleMatchesAlbumFileNames(t *testing.T) {
	cases := map[string]string{
		"Global Warming (feat. Sensato)":                          "01 Global Warming.mp3",
		"Back in Time - featured in \"Men In Black 3\"":           "04 Back in Time.mp3",
		"Rain Over Me (feat. Marc Anthony) - Benny Benassi Remix": "15 Rain Over Me.mp3",
		"I'm Off That":       "12 I'm Off That.mp3",
		"11:59 (feat. Vein)": "16 11_59.mp3",
	}
	for title, file := range cases {
		if !strings.Contains(normTitle(file), normTitle(baseTitle(title))) {
			t.Errorf("%q does not match file %q", title, file)
		}
	}
	if got := baseTitle("(Intro)"); got != "(Intro)" {
		t.Errorf("baseTitle kept nothing: %q", got)
	}
}
