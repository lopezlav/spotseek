package main

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/dialog"
	"fyne.io/fyne/v2/theme"
	"fyne.io/fyne/v2/widget"

	"spotseek-app/internal/ytdlp"
)

var ytSearchBtn *widget.Button

// buildYouTubeTab makes the free-search tab: type anything, pick a YouTube
// Music result, and download it as mp3. No account is needed.
func buildYouTubeTab(w fyne.Window) fyne.CanvasObject {
	queryEntry := widget.NewEntry()
	queryEntry.SetPlaceHolder("Type a song, artist, or a few words from the title")
	resultsBox := container.NewVBox(widget.NewLabel("Search YouTube Music and download without any account."))

	ytSearchBtn = widget.NewButtonWithIcon("Search", theme.SearchIcon(), func() {
		runYouTubeSearch(queryEntry, resultsBox, w)
	})
	queryEntry.OnSubmitted = func(string) {
		runYouTubeSearch(queryEntry, resultsBox, w)
	}

	header := container.NewVBox(
		widget.NewLabelWithStyle("Search YouTube Music", fyne.TextAlignLeading, fyne.TextStyle{Bold: true}),
		queryEntry,
		ytSearchBtn,
	)
	return container.NewBorder(header, nil, nil, nil, container.NewVScroll(resultsBox))
}

// runYouTubeSearch queries YouTube Music and fills the results box.
func runYouTubeSearch(queryEntry *widget.Entry, resultsBox *fyne.Container, w fyne.Window) {
	q := strings.TrimSpace(queryEntry.Text)
	if q == "" {
		dialog.ShowInformation("Missing search", "Type something to search for first.", w)
		return
	}
	ytSearchBtn.Disable()
	resultsBox.Objects = []fyne.CanvasObject{widget.NewLabel("Searching...")}
	resultsBox.Refresh()
	go func() {
		ctx, stop := context.WithTimeout(context.Background(), 2*time.Minute)
		defer stop()
		cands, err := ytdlp.SearchMusic(ctx, q, 20)
		fyne.Do(func() {
			ytSearchBtn.Enable()
			if err != nil {
				addLog("YouTube search failed: " + err.Error())
				resultsBox.Objects = []fyne.CanvasObject{widget.NewLabel("Search failed: " + err.Error())}
				resultsBox.Refresh()
				return
			}
			fillYouTubeResults(resultsBox, cands, w)
		})
	}()
}

func fillYouTubeResults(resultsBox *fyne.Container, cands []ytdlp.Candidate, w fyne.Window) {
	resultsBox.Objects = nil
	for _, c := range cands {
		cand := c
		meta := cand.Artist
		if cand.Duration > 0 {
			meta = fmt.Sprintf("%s · %d:%02d", meta, cand.Duration/60, cand.Duration%60)
		}
		label := widget.NewLabel(fmt.Sprintf("%s\n%s", cand.Title, meta))
		btn := widget.NewButtonWithIcon("Download", theme.DownloadIcon(), func() {
			downloadYouTubeResult(cand, w)
		})
		resultsBox.Add(container.NewBorder(nil, nil, nil, btn, label))
	}
	resultsBox.Refresh()
}

// downloadYouTubeResult saves one search result as an mp3 in the export folder.
func downloadYouTubeResult(c ytdlp.Candidate, w fyne.Window) {
	if jobActive {
		dialog.ShowInformation("Busy", "A download is already running. Wait for it to finish or press Stop.", w)
		return
	}
	dest := strings.TrimSpace(outEntry.Text)
	if dest == "" {
		dialog.ShowInformation("Missing folder", "Choose an export folder on the Download tab first.", w)
		return
	}
	name := ytdlp.SafeName(c.Artist + " - " + c.Title)
	ctx, cfn := context.WithCancel(context.Background())
	cancel = cfn
	jobActive = true
	runBtn.Disable()
	stopBtn.Enable()
	statusLabel.SetText("Downloading " + c.Title + "...")
	addLog("YouTube download: " + c.Title)
	go func() {
		err := ytdlp.Download(ctx, c, dest, name, bitrateSel.Selected+"K")
		fyne.Do(func() {
			jobActive = false
			runBtn.Enable()
			stopBtn.Disable()
			cancel = nil
			if errors.Is(err, context.Canceled) {
				statusLabel.SetText("Stopped")
				return
			}
			if err != nil {
				statusLabel.SetText("Failed")
				addLog("ERROR: " + err.Error())
				dialog.ShowError(err, w)
				return
			}
			statusLabel.SetText("Done: " + c.Title)
			addLog("Saved: " + name + ".mp3")
		})
	}()
}
