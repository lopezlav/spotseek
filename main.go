package main

import (
	"context"
	"flag"
	"fmt"
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
	logMu    sync.Mutex
	logLines []string
	logEntry *widget.Entry
	logScroll *container.Scroll
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
	w.Resize(fyne.NewSize(760, 680))

	prefs := a.Preferences()

	urlEntry := widget.NewEntry()
	urlEntry.SetPlaceHolder("Paste a Spotify playlist, album, or track link")

	outDir := prefs.StringWithFallback("outputDir", filepath.Join(homeDir(), "Downloads", "Spotseek"))
	outEntry := widget.NewEntry()
	outEntry.SetText(outDir)

	bitrateSel := widget.NewSelect([]string{"320", "256", "192", "128"}, nil)
	bitrateSel.SetSelected(prefs.StringWithFallback("bitrate", "320"))

	fallbackChk := widget.NewCheck("Use YouTube Music for tracks Soulseek cannot find", nil)
	fallbackChk.SetChecked(prefs.BoolWithFallback("fallback", true))

	userEntry := widget.NewEntry()
	userEntry.SetPlaceHolder("Soulseek username (optional if configured)")
	passEntry := widget.NewPasswordEntry()
	passEntry.SetPlaceHolder("Soulseek password")

	advanced := widget.NewAccordion(widget.NewAccordionItem("Soulseek account",
		container.NewVBox(widget.NewLabel("Leave empty to use ~/.config/sockseek/sockseek.conf"), userEntry, passEntry)))

	logEntry = widget.NewMultiLineEntry()
	logEntry.Wrapping = fyne.TextWrapWord
	logEntry.TextStyle = fyne.TextStyle{Monospace: true}
	logEntry.Disable()
	logScroll = container.NewScroll(logEntry)
	logScroll.SetMinSize(fyne.NewSize(720, 300))

	progress := widget.NewProgressBar()
	progress.Hide()

	statusLabel := widget.NewLabel("Ready")

	var cancel context.CancelFunc

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

	folderRow := container.NewBorder(nil, nil, nil, browseBtn, outEntry)

	runBtn := widget.NewButtonWithIcon("Download", theme.DownloadIcon(), nil)
	stopBtn := widget.NewButtonWithIcon("Stop", theme.CancelIcon(), nil)
	stopBtn.Disable()
	openBtn := widget.NewButtonWithIcon("Open folder", theme.FolderIcon(), func() {
		openFolder(outEntry.Text)
	})

	runBtn.OnTapped = func() {
		input := strings.TrimSpace(urlEntry.Text)
		if input == "" {
			dialog.ShowInformation("Missing link", "Paste a Spotify playlist, album, or track link first.", w)
			return
		}
		dest := strings.TrimSpace(outEntry.Text)
		if dest == "" {
			dialog.ShowInformation("Missing folder", "Choose an export folder.", w)
			return
		}
		prefs.SetString("outputDir", dest)
		prefs.SetString("bitrate", bitrateSel.Selected)
		prefs.SetBool("fallback", fallbackChk.Checked)

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
					dialog.ShowError(err, w)
					return
				}
				_ = pipeline.WriteReport(res)
				statusLabel.SetText(fmt.Sprintf("Done: %d/%d tracks, %d files", res.Files, res.Total, res.Files))
				dialog.ShowInformation("Done",
					fmt.Sprintf("%d of %d tracks saved to\n%s", res.Files, res.Total, res.OutputDir), w)
			})
		}()
	}

	stopBtn.OnTapped = func() {
		if cancel != nil {
			cancel()
			statusLabel.SetText("Stopping...")
		}
	}

	top := container.NewVBox(
		widget.NewLabelWithStyle("Spotseek", fyne.TextAlignLeading, fyne.TextStyle{Bold: true}),
		widget.NewLabel("Download a public Spotify playlist, album, or track as mp3."),
		widget.NewSeparator(),
		widget.NewLabel("Spotify link"),
		urlEntry,
		widget.NewLabel("Export folder"),
		folderRow,
		container.NewGridWithColumns(2,
			container.NewVBox(widget.NewLabel("Quality (mp3)"), bitrateSel),
			container.NewVBox(widget.NewLabel(""), fallbackChk),
		),
		advanced,
		container.NewHBox(runBtn, stopBtn, openBtn, statusLabel),
		progress,
		widget.NewLabel("Log"),
	)

	w.SetContent(container.NewBorder(top, nil, nil, nil, logScroll))
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
				// ScrollToBottom uses the laid-out content height, so it must run
				// after the new text has been through a layout pass.
				time.AfterFunc(40*time.Millisecond, func() {
					fyne.Do(func() { logScroll.ScrollToBottom() })
				})
			})
		}
	}()

	a.Run()
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
