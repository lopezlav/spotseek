package server

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
)

// Settings holds the headless server configuration. It is stored as JSON in
// the OS config dir so the server does not depend on the GUI preferences.
type Settings struct {
	Addr            string  `json:"addr"`
	OutputDir       string  `json:"output_dir"`
	Bitrate         string  `json:"bitrate"`
	Fallback        bool    `json:"fallback"`
	Concurrency     int     `json:"concurrency"`
	SockseekWaitSec int     `json:"sockseek_wait_sec"`
	MinScore        float64 `json:"min_score"`
	SockseekPath    string  `json:"sockseek_path,omitempty"`
	YtdlpPath       string  `json:"ytdlp_path,omitempty"`
	FfmpegPath      string  `json:"ffmpeg_path,omitempty"`
	FfprobePath     string  `json:"ffprobe_path,omitempty"`
	SoulseekUser    string  `json:"soulseek_user,omitempty"`
	SoulseekPass    string  `json:"soulseek_pass,omitempty"`
	ClientID        string  `json:"client_id,omitempty"`
}

// ConfigDir returns the spotseek config directory.
func ConfigDir() (string, error) {
	base, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(base, "spotseek"), nil
}

// ConfigPath returns the server.json path.
func ConfigPath() (string, error) {
	dir, err := ConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "server.json"), nil
}

func defaultOutputDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return "Spotify"
	}
	return filepath.Join(home, "Music", "Spotify")
}

// DefaultSettings returns the built-in defaults.
func DefaultSettings() *Settings {
	return &Settings{
		Addr:            "127.0.0.1:8765",
		OutputDir:       defaultOutputDir(),
		Bitrate:         "320",
		Fallback:        true,
		Concurrency:     2,
		SockseekWaitSec: 180,
		MinScore:        6,
	}
}

// LoadSettings reads server.json and fills defaults for missing values.
func LoadSettings() (*Settings, error) {
	s := DefaultSettings()
	path, err := ConfigPath()
	if err == nil {
		raw, readErr := os.ReadFile(path)
		if readErr == nil {
			if err := json.Unmarshal(raw, s); err != nil {
				return nil, err
			}
		} else if !os.IsNotExist(readErr) {
			return nil, readErr
		}
	}
	if s.ClientID == "" {
		// Reuse the Spotify app client id from the Spotseek GUI so a user
		// who already logged in there does not have to configure it again.
		s.ClientID = importGUIClientID()
	}
	s.applyDefaults()
	return s, nil
}

// importGUIClientID reads the Fyne preferences file of the GUI app.
func importGUIClientID() string {
	var path string
	switch runtime.GOOS {
	case "darwin":
		home, err := os.UserHomeDir()
		if err != nil {
			return ""
		}
		path = filepath.Join(home, "Library", "Preferences", "fyne", "com.example.Spotseek", "preferences.json")
	case "windows":
		path = filepath.Join(os.Getenv("APPDATA"), "fyne", "com.example.Spotseek", "preferences.json")
	default:
		base := os.Getenv("XDG_CONFIG_HOME")
		if base == "" {
			home, err := os.UserHomeDir()
			if err != nil {
				return ""
			}
			base = filepath.Join(home, ".config")
		}
		path = filepath.Join(base, "fyne", "com.example.Spotseek", "preferences.json")
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	var prefs map[string]any
	if err := json.Unmarshal(raw, &prefs); err != nil {
		return ""
	}
	if v, ok := prefs["spotifyClientID"].(string); ok {
		return v
	}
	return ""
}

func (s *Settings) applyDefaults() {
	d := DefaultSettings()
	if s.Addr == "" {
		s.Addr = d.Addr
	}
	if s.OutputDir == "" {
		s.OutputDir = d.OutputDir
	}
	if s.Bitrate == "" {
		s.Bitrate = d.Bitrate
	}
	if s.Concurrency < 1 {
		s.Concurrency = d.Concurrency
	}
	if s.Concurrency > 8 {
		s.Concurrency = 8
	}
	if s.SockseekWaitSec <= 0 {
		s.SockseekWaitSec = d.SockseekWaitSec
	}
	if s.MinScore == 0 {
		s.MinScore = d.MinScore
	}
}

// Save writes server.json.
func (s *Settings) Save() error {
	path, err := ConfigPath()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	raw, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, raw, 0o600)
}
