package spotify

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/zalando/go-keyring"
)

const (
	authURL     = "https://accounts.spotify.com/authorize"
	tokenURL    = "https://accounts.spotify.com/api/token"
	apiBase     = "https://api.spotify.com/v1"
	redirectURI = "http://127.0.0.1:48721/callback"
	// Read-only scopes. The app never writes to your Spotify account.
	scopes       = "playlist-read-private playlist-read-collaborative user-library-read user-read-private user-follow-read"
	keyringSvc   = "spotseek"
	keyringUser  = "spotify-refresh-token"
	callbackHTML = `<html><body style="font-family:sans-serif;background:#121212;color:#fff;text-align:center;padding-top:80px"><h2>Spotseek is connected</h2><p>You can close this window and return to the app.</p></body></html>`
)

// Auth holds the Spotify OAuth (PKCE) state for one user.
type Auth struct {
	ClientID       string
	RedirectURI    string
	accessToken    string
	refreshToken   string
	expiresAt      time.Time
	mu             sync.Mutex
	sessionVersion uint64
	refreshing     chan struct{}
}

// NewAuth creates an Auth. The client id comes from your Spotify developer app.
func NewAuth(clientID string) *Auth {
	return &Auth{ClientID: strings.TrimSpace(clientID), RedirectURI: redirectURI}
}

// AI-assisted code: use the same lock for client changes and token refresh.
func (a *Auth) SetClientID(clientID string) {
	a.mu.Lock()
	defer a.mu.Unlock()
	next := strings.TrimSpace(clientID)
	if a.ClientID != next {
		a.sessionVersion++
		a.accessToken = ""
	}
	a.ClientID = next
}

func (a *Auth) SessionVersion() uint64 {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.sessionVersion
}

// End AI-assisted code.

// RedirectURL is the loopback redirect URI to register in your Spotify app.
func RedirectURL() string { return redirectURI }

// LoadStoredRefresh returns the saved refresh token, if any.
func LoadStoredRefresh() (string, error) {
	return keyring.Get(keyringSvc, keyringUser)
}

// LoggedIn reports whether a refresh token is stored.
func (a *Auth) LoggedIn() bool {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.refreshToken != "" {
		return true
	}
	t, err := LoadStoredRefresh()
	if err == nil && t != "" {
		a.refreshToken = t
		return true
	}
	return false
}

// AI-assisted code: bind saved account metadata to the current login without saving a token.
func (a *Auth) CacheKey() string {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.ClientID == "" {
		return ""
	}
	if a.refreshToken == "" {
		if token, err := LoadStoredRefresh(); err == nil {
			a.refreshToken = token
		}
	}
	if a.refreshToken == "" {
		return ""
	}
	sum := sha256.Sum256([]byte(a.ClientID + "\x00" + a.refreshToken))
	return fmt.Sprintf("%x", sum)
}

// End AI-assisted code.

// Logout removes the stored refresh token.
func (a *Auth) Logout() {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.logoutLocked()
}

func (a *Auth) logoutLocked() {
	a.sessionVersion++
	a.refreshToken = ""
	a.accessToken = ""
	a.expiresAt = time.Time{}
	_ = keyring.Delete(keyringSvc, keyringUser)
}

func randomURLSafe(n int) (string, error) {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(b), nil
}

func codeChallenge(verifier string) string {
	sum := sha256.Sum256([]byte(verifier))
	return base64.RawURLEncoding.EncodeToString(sum[:])
}

type tokenResponse struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresIn    int    `json:"expires_in"`
	TokenType    string `json:"token_type"`
}

// Login runs the Authorization Code with PKCE flow. It opens the browser,
// waits for the loopback redirect, and stores the refresh token.
func (a *Auth) Login(ctx context.Context, openURL func(string) error) error {
	a.mu.Lock()
	clientID, redirect := a.ClientID, a.RedirectURI
	a.mu.Unlock()
	if clientID == "" {
		return errors.New("a Spotify client id is required")
	}
	verifier, err := randomURLSafe(64)
	if err != nil {
		return err
	}
	state, err := randomURLSafe(16)
	if err != nil {
		return err
	}

	ln, err := net.Listen("tcp", "127.0.0.1:48721")
	if err != nil {
		return fmt.Errorf("cannot listen on 127.0.0.1:48721 (is another login open?): %w", err)
	}
	defer ln.Close()

	codeCh := make(chan string, 1)
	errCh := make(chan error, 1)
	srv := &http.Server{}
	mux := http.NewServeMux()
	mux.HandleFunc("/callback", func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		if e := q.Get("error"); e != "" {
			errCh <- fmt.Errorf("spotify returned: %s", e)
		} else if q.Get("state") != state {
			errCh <- errors.New("state mismatch")
		} else {
			codeCh <- q.Get("code")
		}
		w.Header().Set("Content-Type", "text/html")
		_, _ = io.WriteString(w, callbackHTML)
	})
	srv.Handler = mux
	go func() { _ = srv.Serve(ln) }()
	defer srv.Shutdown(context.Background())

	q := url.Values{}
	q.Set("client_id", clientID)
	q.Set("response_type", "code")
	q.Set("redirect_uri", redirect)
	q.Set("scope", scopes)
	q.Set("state", state)
	q.Set("code_challenge_method", "S256")
	q.Set("code_challenge", codeChallenge(verifier))
	if err := openURL(authURL + "?" + q.Encode()); err != nil {
		return fmt.Errorf("cannot open the browser: %w", err)
	}

	var code string
	select {
	case code = <-codeCh:
	case err := <-errCh:
		return err
	case <-time.After(3 * time.Minute):
		return errors.New("login timed out")
	case <-ctx.Done():
		return ctx.Err()
	}

	form := url.Values{}
	form.Set("grant_type", "authorization_code")
	form.Set("code", code)
	form.Set("redirect_uri", redirect)
	form.Set("client_id", clientID)
	form.Set("code_verifier", verifier)
	tok, err := a.postTokenContext(ctx, form)
	if err != nil {
		return err
	}
	a.mu.Lock()
	a.sessionVersion++
	a.apply(tok)
	a.mu.Unlock()
	if tok.RefreshToken != "" {
		_ = keyring.Set(keyringSvc, keyringUser, tok.RefreshToken)
	}
	return nil
}

func (a *Auth) postToken(form url.Values) (*tokenResponse, error) {
	return a.postTokenContext(context.Background(), form)
}

func (a *Auth) postTokenContext(ctx context.Context, form url.Values) (*tokenResponse, error) {
	req, err := http.NewRequestWithContext(ctx, "POST", tokenURL, strings.NewReader(form.Encode()))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("token request failed: HTTP %d: %s", resp.StatusCode, string(body))
	}
	var tok tokenResponse
	if err := json.Unmarshal(body, &tok); err != nil {
		return nil, err
	}
	return &tok, nil
}

func (a *Auth) apply(tok *tokenResponse) {
	a.accessToken = tok.AccessToken
	if tok.RefreshToken != "" {
		a.refreshToken = tok.RefreshToken
	}
	a.expiresAt = time.Now().Add(time.Duration(tok.ExpiresIn-30) * time.Second)
}

// EnsureToken returns a valid access token, refreshing it when needed.
func (a *Auth) EnsureToken() (string, error) {
	return a.ensureTokenContext(context.Background())
}

// AI-assisted code: a slow token refresh must not block reads of the saved account key.
func (a *Auth) ensureTokenContext(ctx context.Context) (string, error) {
	for {
		a.mu.Lock()
		if a.accessToken != "" && time.Now().Before(a.expiresAt) {
			token := a.accessToken
			a.mu.Unlock()
			return token, nil
		}
		if active := a.refreshing; active != nil {
			a.mu.Unlock()
			select {
			case <-active:
				continue
			case <-ctx.Done():
				return "", ctx.Err()
			}
		}
		flight := make(chan struct{})
		a.refreshing = flight
		if a.refreshToken == "" {
			if token, err := LoadStoredRefresh(); err == nil {
				a.refreshToken = token
			}
		}
		if a.refreshToken == "" {
			close(flight)
			a.refreshing = nil
			a.mu.Unlock()
			return "", errors.New("not connected to Spotify")
		}
		refresh, clientID, version := a.refreshToken, a.ClientID, a.sessionVersion
		a.mu.Unlock()
		form := url.Values{}
		form.Set("grant_type", "refresh_token")
		form.Set("refresh_token", refresh)
		form.Set("client_id", clientID)
		tok, err := a.postTokenContext(ctx, form)
		a.mu.Lock()
		defer a.mu.Unlock()
		defer func() { close(flight); a.refreshing = nil }()
		if version != a.sessionVersion {
			return "", errors.New("Spotify login changed during token refresh")
		}
		if err != nil {
			if strings.Contains(err.Error(), "invalid_grant") {
				a.logoutLocked()
				return "", errors.New("Spotify session expired. Click Disconnect, then Connect again")
			}
			return "", err
		}
		a.apply(tok)
		if tok.RefreshToken != "" {
			_ = keyring.Set(keyringSvc, keyringUser, tok.RefreshToken)
		}
		return a.accessToken, nil
	}
}

// End AI-assisted code.

// get performs an authenticated GET and decodes the JSON body.
func (a *Auth) get(ctx context.Context, url string, out any) error {
	return a.getWithRetry(ctx, url, out, true)
}

// AI-assisted code: retry one rejected access token through the shared refresh.
func (a *Auth) getWithRetry(ctx context.Context, url string, out any, retry bool) error {
	token, err := a.ensureTokenContext(ctx)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	resp, err := httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, readErr := io.ReadAll(io.LimitReader(resp.Body, 16<<20))
	if readErr != nil {
		return readErr
	}
	if resp.StatusCode == 401 {
		a.mu.Lock()
		if a.accessToken == token {
			a.accessToken = ""
		}
		a.mu.Unlock()
		if retry {
			return a.getWithRetry(ctx, url, out, false)
		}
		return &APIError{Status: resp.StatusCode}
	}
	if resp.StatusCode != 200 {
		retry, _ := strconv.Atoi(resp.Header.Get("Retry-After"))
		if retry < 1 {
			retry = 30
		}
		return &APIError{Status: resp.StatusCode, RetryAfter: retry, Playlist: strings.Contains(url, "/playlists/")}
	}
	return json.Unmarshal(body, out)
}

// End AI-assisted code.
