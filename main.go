package main

import (
	"context"
	"flag"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/app"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/dialog"
	"fyne.io/fyne/v2/storage"
	"fyne.io/fyne/v2/theme"
	"fyne.io/fyne/v2/widget"

	"spotseek-app/internal/pipeline"
	"spotseek-app/internal/spotify"
)

var (
	logMu     sync.Mutex
	logLines  []string
	logEntry  *widget.Entry
	logScroll *container.Scroll

	auth         *spotify.Auth
	spotifyHost  *fyne.Container
	refreshSpotify func()
	outEntry     *widget.Entry
	bitrateSel   *widget.Select
	fallbackChk  *widget.Check
	userEntry    *widget.Entry
	passEntry    *widget.Entry
	runBtn       *widget.Button
	stopBtn      *widget.Button
	progress     *widget.ProgressBar
	statusLabel  *widget.Label
	cancel       context.CancelFunc
	mainWindow   fyne.Window
)

func addLog(s string) {
	logMu.Lock()
	logLines = append(logLines, s)
	if len(logLines) > 3000 {
		logLines = logLines[len(logLines)-3000:]
	}
	logMu.Unlock()
}

func main() {
	ensurePath()
	if len(os.Args) > 1 && os.Args[1] == "--cli" {
		runCLI(os.Args[2:])
		return
	}

	a := app.New()
	a.Settings().SetTheme(theme.DarkTheme())
	w := a.NewWindow("Spotseek")
	mainWindow = w
	w.Resize(fyne.NewSize(820, 720))

	prefs := a.Preferences()

	urlEntry := widget.NewEntry()
	urlEntry.SetPlaceHolder("Paste a Spotify playlist, album, or track link")

	outEntry = widget.NewEntry()
	outEntry.SetText(prefs.StringWithFallback("outputDir", filepath.Join(homeDir(), "Downloads", "Spotseek")))

	bitrateSel = widget.NewSelect([]string{"320", "256", "192", "128"}, nil)
	bitrateSel.SetSelected(prefs.StringWithFallback("bitrate", "320"))

	fallbackChk = widget.NewCheck("Use YouTube Music for tracks Soulseek cannot find", nil)
	fallbackChk.SetChecked(prefs.BoolWithFallback("fallback", true))

	userEntry = widget.NewEntry()
	userEntry.SetPlaceHolder("Soulseek username (optional if configured)")
	passEntry = widget.NewPasswordEntry()
	passEntry.SetPlaceHolder("Soulseek password")
	advanced := widget.NewAccordion(widget.NewAccordionItem("Soulseek account",
		container.NewVBox(widget.NewLabel("Leave empty to use ~/.config/sockseek/sockseek.conf"), userEntry, passEntry)))

	browseBtn := widget.NewButtonWithIcon("", theme.FolderOpenIcon(), func() {
		d := dialog.NewFolderOpen(func(lu fyne.ListableURI, err error) {
			if err == nil && lu != nil {
				outEntry.SetText(lu.Path())
			}
		}, w)
		if lu, err := storage.ListerForURI(storage.NewFileURI(outEntry.Text)); err == nil {
			d.SetLocation(lu)
		}
		d.Resize(fyne.NewSize(600, 450))
		d.Show()
	})

	runBtn = widget.NewButtonWithIcon("Download", theme.DownloadIcon(), func() {
		startRun(strings.TrimSpace(urlEntry.Text), nil)
	})
	stopBtn = widget.NewButtonWithIcon("Stop", theme.CancelIcon(), func() {
		if cancel != nil {
			cancel()
			statusLabel.SetText("Stopping...")
		}
	})
	stopBtn.Disable()
	openBtn := widget.NewButtonWithIcon("Open folder", theme.FolderIcon(), func() {
		openFolder(outEntry.Text)
	})

	linkTab := container.NewVBox(
		widget.NewLabel("Download from a link"),
		urlEntry,
		widget.NewLabel("Export folder"),
		container.NewBorder(nil, nil, nil, browseBtn, outEntry),
		container.NewGridWithColumns(2,
			container.NewVBox(widget.NewLabel("Quality (mp3)"), bitrateSel),
			container.NewVBox(widget.NewLabel(""), fallbackChk),
		),
		advanced,
		container.NewHBox(runBtn, stopBtn, openBtn),
	)

	// My Spotify tab
	spotifyHost = container.NewMax()
	clientEntry := widget.NewEntry()
	clientEntry.SetText(prefs.StringWithFallback("spotifyClientID", ""))
	clientEntry.SetPlaceHolder("Spotify app Client ID")
	clientHelp := widget.NewLabel("Create an app at developer.spotify.com/dashboard.\n" +
		"Add this exact Redirect URI:  " + spotify.RedirectURL() + "\n" +
		"Then paste the Client ID here and click Connect. The app only reads your library.")
	clientHelp.Wrapping = fyne.TextWrapWord
	var connectBtn *widget.Button
	connectBtn = widget.NewButtonWithIcon("Connect Spotify", theme.LoginIcon(), func() {
		cid := strings.TrimSpace(clientEntry.Text)
		if cid == "" {
			dialog.ShowInformation("Client ID needed", "Paste your Spotify app Client ID first.", w)
			return
		}
		prefs.SetString("spotifyClientID", cid)
		auth = spotify.NewAuth(cid)
		connectBtn.Disable()
		statusLabel.SetText("Waiting for Spotify login...")
		go func() {
			err := auth.Login(context.Background(), func(u string) error {
				parsed, perr := url.Parse(u)
				if perr != nil {
					return perr
				}
				fyne.DoAndWait(func() { _ = fyne.CurrentApp().OpenURL(parsed) })
				return nil
			})
			fyne.Do(func() {
				connectBtn.Enable()
				if err != nil {
					addLog("Spotify login failed: " + err.Error())
					dialog.ShowError(err, w)
					statusLabel.SetText("Spotify login failed")
					return
				}
				addLog("Connected to Spotify (read-only).")
				statusLabel.SetText("Connected to Spotify")
				refreshSpotify()
			})
		}()
	})

	// restore a previous session
	if cid := prefs.String("spotifyClientID"); cid != "" {
		auth = spotify.NewAuth(cid)
	}

	refreshSpotify = func() {
		spotifyHost.Objects = nil
		if auth == nil || !auth.LoggedIn() {
			spotifyHost.Add(container.NewVBox(
				widget.NewLabelWithStyle("Connect your Spotify account", fyne.TextAlignLeading, fyne.TextStyle{Bold: true}),
				clientHelp,
				widget.NewLabel("Client ID"),
				clientEntry,
				connectBtn,
			))
		} else {
			spotifyHost.Add(buildLibraryUI(prefs, w))
		}
		spotifyHost.Refresh()
	}
	refreshSpotify()

	spotifyTab := spotifyHost

	tabs := container.NewAppTabs(
		container.NewTabItemWithIcon("Download", theme.DownloadIcon(), linkTab),
		container.NewTabItemWithIcon("My Spotify", theme.AccountIcon(), spotifyTab),
	)

	// Log and progress at the bottom, shared by both tabs.
	logEntry = widget.NewMultiLineEntry()
	logEntry.Wrapping = fyne.TextWrapWord
	logEntry.TextStyle = fyne.TextStyle{Monospace: true}
	logEntry.Disable()
	logScroll = container.NewScroll(logEntry)
	progress = widget.NewProgressBar()
	progress.Hide()
	statusLabel = widget.NewLabel("Ready")

	bottom := container.NewVBox(
		container.NewHBox(progress, statusLabel),
		widget.NewLabel("Log"),
		logScroll,
	)
	logScroll.SetMinSize(fyne.NewSize(800, 220))

	split := container.NewVSplit(tabs, bottom)
	split.SetOffset(0.62)
	w.SetContent(split)
	w.Show()

	go func() {
		ticker := time.NewTicker(200 * time.Millisecond)
		defer ticker.Stop()
		for range ticker.C {
			logMu.Lock()
			text := strings.Join(logLines, "\n")
			logMu.Unlock()
			fyne.Do(func() {
				if logEntry.Text == text {
					return
				}
				logEntry.SetText(text)
				time.AfterFunc(40*time.Millisecond, func() {
					fyne.Do(func() { logScroll.ScrollToBottom() })
				})
			})
		}
	}()

	a.Run()
}

// startRun validates the inputs and runs the pipeline in the background.
func startRun(input string, coll *spotify.Collection) {
	if input == "" && coll == nil {
		dialog.ShowInformation("Missing link", "Paste a Spotify playlist, album, or track link first.", mainWindow)
		return
	}
	dest := strings.TrimSpace(outEntry.Text)
	if dest == "" {
		dialog.ShowInformation("Missing folder", "Choose an export folder.", mainWindow)
		return
	}

	logMu.Lock()
	logLines = nil
	logMu.Unlock()
	logEntry.SetText("")

	ctx, c := context.WithCancel(context.Background())
	cancel = c

	runBtn.Disable()
	stopBtn.Enable()
	progress.Show()
	progress.SetValue(0)
	statusLabel.SetText("Working...")

	opts := &pipeline.Options{
		Input:        input,
		Collection:   coll,
		OutputDir:    dest,
		Bitrate:      bitrateSel.Selected,
		UseFallback:  fallbackChk.Checked,
		SoulseekUser: strings.TrimSpace(userEntry.Text),
		SoulseekPass: passEntry.Text,
		Log:          addLog,
	}

	go func() {
		res, err := pipeline.Run(ctx, opts)
		fyne.Do(func() {
			progress.Hide()
			runBtn.Enable()
			stopBtn.Disable()
			if err != nil {
				statusLabel.SetText("Failed")
				addLog("ERROR: " + err.Error())
				dialog.ShowError(err, mainWindow)
				return
			}
			_ = pipeline.WriteReport(res)
			statusLabel.SetText(fmt.Sprintf("Done: %d files", res.Files))
			dialog.ShowInformation("Done",
				fmt.Sprintf("%d of %d tracks saved to\n%s", res.Files, res.Total, res.OutputDir), mainWindow)
		})
	}()
}

func buildLibraryUI(prefs fyne.Preferences, w fyne.Window) fyne.CanvasObject {
	profileLabel := widget.NewLabel("Loading...")
	disconnect := widget.NewButtonWithIcon("Disconnect", theme.LogoutIcon(), func() {
		if auth != nil {
			auth.Logout()
		}
		addLog("Disconnected from Spotify.")
		refreshSpotifyTab()
	})

	playlistsBox := container.NewVBox(widget.NewLabel("Loading playlists..."))
	albumsBox := container.NewVBox(widget.NewLabel("Loading albums..."))
	likedBox := container.NewVBox(widget.NewLabel("Loading liked songs..."))

	go func() {
		ctx := context.Background()
		name, _ := auth.Profile(ctx)
		playlists, errP := auth.Playlists(ctx)
		albums, errA := auth.Albums(ctx)
		liked, errL := auth.LikedTracks(ctx)
		fyne.Do(func() {
			if name != "" {
				profileLabel.SetText("Signed in as " + name)
			}
			fillItems(playlistsBox, playlists, errP)
			fillItems(albumsBox, albums, errA)
			fillLiked(likedBox, liked, errL)
		})
	}()

	tabs := container.NewAppTabs(
		container.NewTabItem("Playlists", container.NewVScroll(playlistsBox)),
		container.NewTabItem("Albums", container.NewVScroll(albumsBox)),
		container.NewTabItem("Liked Songs", container.NewVScroll(likedBox)),
	)
	header := container.NewBorder(nil, nil, nil, disconnect, profileLabel)
	return container.NewBorder(header, nil, nil, nil, tabs)
}

func fillItems(box *fyne.Container, items []spotify.LibraryItem, err error) {
	box.Objects = nil
	if err != nil {
		box.Add(widget.NewLabel("Could not load: " + err.Error()))
		box.Refresh()
		return
	}
	if len(items) == 0 {
		box.Add(widget.NewLabel("Nothing here"))
		box.Refresh()
		return
	}
	for _, it := range items {
		item := it
		meta := item.Subtitle
		if item.Count > 0 {
			meta = fmt.Sprintf("%s · %d tracks", item.Subtitle, item.Count)
		}
		label := widget.NewLabel(fmt.Sprintf("%s\n%s", item.Name, meta))
		btn := widget.NewButtonWithIcon("Download", theme.DownloadIcon(), func() {
			downloadLibraryItem(item)
		})
		box.Add(container.NewBorder(nil, nil, nil, btn, label))
	}
	box.Refresh()
}

func fillLiked(box *fyne.Container, tracks []spotify.Track, err error) {
	box.Objects = nil
	if err != nil {
		box.Add(widget.NewLabel("Could not load: " + err.Error()))
		box.Refresh()
		return
	}
	label := widget.NewLabel(fmt.Sprintf("Liked Songs\n%d tracks", len(tracks)))
	btn := widget.NewButtonWithIcon("Download all", theme.DownloadIcon(), func() {
		if len(tracks) == 0 {
			return
		}
		startRun("", &spotify.Collection{Kind: "playlist", Name: "Liked Songs", Tracks: tracks})
	})
	box.Add(container.NewBorder(nil, nil, nil, btn, label))
	box.Refresh()
}

// downloadLibraryItem fetches the tracks of a playlist or album, then runs the pipeline.
func downloadLibraryItem(item spotify.LibraryItem) {
	statusLabel.SetText("Loading " + item.Name + "...")
	go func() {
		ctx := context.Background()
		var coll *spotify.Collection
		var err error
		if item.Kind == "album" {
			var tracks []spotify.Track
			tracks, err = auth.AlbumTracks(ctx, item.ID)
			coll = &spotify.Collection{Kind: "album", Name: item.Name, Artist: item.Subtitle, Tracks: tracks}
		} else {
			var tracks []spotify.Track
			tracks, err = auth.PlaylistTracks(ctx, item.ID)
			if err != nil || len(tracks) == 0 {
				// Spotify returns playlist contents only for playlists the user
				// owns or collaborates on. Try the public page for the rest.
				if c, e := spotify.Extract("https://open.spotify.com/playlist/" + item.ID); e == nil {
					tracks = c.Tracks
					err = nil
				}
			}
			coll = &spotify.Collection{Kind: "playlist", Name: item.Name, Tracks: tracks}
		}
		fyne.Do(func() {
			if err != nil {
				statusLabel.SetText("Failed to load " + item.Name)
				dialog.ShowError(err, mainWindow)
				return
			}
			if len(coll.Tracks) == 0 {
				dialog.ShowInformation("Empty", item.Name+" has no tracks.", mainWindow)
				return
			}
			startRun("", coll)
		})
	}()
}

func refreshSpotifyTab() {
	// Rebuild the My Spotify tab after a login or logout.
	if refreshSpotify != nil {
		refreshSpotify()
	}
}

func homeDir() string {
	h, err := os.UserHomeDir()
	if err != nil {
		return "."
	}
	return h
}

// ensurePath adds common install folders to PATH. A macOS .app started from
// Finder, or a Windows app started from Explorer, may not inherit the shell
// PATH, so sockseek, yt-dlp, and ffmpeg would not be found without this.
func ensurePath() {
	home := homeDir()
	var dirs []string
	switch runtime.GOOS {
	case "windows":
		dirs = []string{
			filepath.Join(os.Getenv("LOCALAPPDATA"), "Microsoft", "WinGet", "Links"),
			filepath.Join(os.Getenv("USERPROFILE"), "scoop", "shims"),
			filepath.Join(home, ".local", "bin"),
		}
	case "darwin":
		dirs = []string{
			"/opt/homebrew/bin",
			"/usr/local/bin",
			filepath.Join(home, ".local", "bin"),
			filepath.Join(home, "go", "bin"),
		}
	default:
		dirs = []string{
			"/usr/local/bin",
			filepath.Join(home, ".local", "bin"),
			filepath.Join(home, "go", "bin"),
		}
	}
	path := os.Getenv("PATH")
	for _, dir := range dirs {
		if dir == "" || strings.Contains(path, dir) {
			continue
		}
		path = dir + string(os.PathListSeparator) + path
	}
	_ = os.Setenv("PATH", path)
}

// openFolder opens a folder in the system file manager.
func openFolder(path string) {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", path)
	case "windows":
		cmd = exec.Command("explorer", path)
	default:
		cmd = exec.Command("xdg-open", path)
	}
	_ = cmd.Start()
}

// reorderFlags moves flag arguments before positional ones so the Go flag
// package can parse a command like: --cli <url> -o <dir>
func reorderFlags(args []string) []string {
	valueFlags := map[string]bool{
		"-o": true, "--o": true,
		"-bitrate": true, "--bitrate": true,
		"-limit": true, "--limit": true,
	}
	var flags, positional []string
	for i := 0; i < len(args); i++ {
		a := args[i]
		if strings.HasPrefix(a, "-") {
			flags = append(flags, a)
			if !strings.Contains(a, "=") && valueFlags[a] && i+1 < len(args) {
				i++
				flags = append(flags, args[i])
			}
		} else {
			positional = append(positional, a)
		}
	}
	return append(flags, positional...)
}

func runCLI(args []string) {
	fs := flag.NewFlagSet("spotseek", flag.ExitOnError)
	out := fs.String("o", "", "output folder")
	bitrate := fs.String("bitrate", "320", "mp3 bitrate")
	noFallback := fs.Bool("no-fallback", false, "skip the YouTube fallback")
	limit := fs.Int("limit", 0, "only the first N tracks")
	extractOnly := fs.Bool("extract-only", false, "only print the Spotify track list")
	_ = fs.Parse(reorderFlags(args))
	input := fs.Arg(0)
	if input == "" {
		fmt.Fprintln(os.Stderr, "usage: spotseek-app --cli <spotify-url> -o <dir>")
		os.Exit(2)
	}

	if *extractOnly {
		coll, err := spotify.Extract(input)
		if err != nil {
			fmt.Fprintln(os.Stderr, "error:", err)
			os.Exit(1)
		}
		fmt.Printf("%s (%s, %d tracks)\n", coll.Name, coll.Kind, len(coll.Tracks))
		for i, t := range coll.Tracks {
			fmt.Printf("%3d  %-30s %-40s %ds\n", i+1, t.Artist, t.Title, t.DurationSec)
		}
		return
	}

	if *out == "" {
		fmt.Fprintln(os.Stderr, "-o is required")
		os.Exit(2)
	}
	res, err := pipeline.Run(context.Background(), &pipeline.Options{
		Input:       input,
		OutputDir:   *out,
		Bitrate:     *bitrate,
		UseFallback: !*noFallback,
		Limit:       *limit,
		Log:         func(s string) { fmt.Println(s) },
	})
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
	_ = pipeline.WriteReport(res)
	fmt.Printf("Done: %d files in %s\n", res.Files, res.OutputDir)
}
