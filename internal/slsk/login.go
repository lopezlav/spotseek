// Package slsk signs in to the Soulseek server, only to check an account.
//
// Soulseek has no sign-up page: the server creates an account the first time
// someone signs in with a name that is not taken (Nicotine+ and SoulseekQt do
// the same). So "create an account" and "sign in" are one step. The message
// format follows Soulseek.NET, the library Sockseek uses.
package slsk

import (
	"bytes"
	"context"
	"crypto/md5"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"strings"
	"time"
)

// Server is the Soulseek server address.
const Server = "server.slsknet.org:2271"

const (
	codeLogin    = 1
	majorVersion = 170
	minorVersion = 1
)

// Result is what the server said.
type Result struct {
	OK bool
	// Reason is the server's code when OK is false: INVALIDPASS (the name is
	// taken, or the password is wrong), INVALIDUSERNAME, SVRFULL, ...
	Reason string
	Detail string
}

// ErrBadName is returned for a name the server would not take.
var ErrBadName = errors.New("the name must be 1 to 30 letters, numbers or symbols, with no spaces at the start or end")

// CheckName tests the rules of Soulseek names before they go to the server.
func CheckName(name string) error {
	if name == "" || len(name) > 30 || strings.TrimSpace(name) != name {
		return ErrBadName
	}
	for _, r := range name {
		if r < 0x20 || r > 0x7e {
			return ErrBadName
		}
	}
	return nil
}

// Login signs in with name and password, reads the answer, and disconnects.
// A new name creates the account. Signing in with an account that is in use
// elsewhere signs that other session out.
func Login(ctx context.Context, addr, name, password string) (Result, error) {
	if err := CheckName(name); err != nil {
		return Result{}, err
	}
	if password == "" {
		return Result{}, errors.New("the password is empty")
	}
	var d net.Dialer
	cctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	conn, err := d.DialContext(cctx, "tcp", addr)
	if err != nil {
		return Result{}, fmt.Errorf("cannot reach the Soulseek server: %w", err)
	}
	defer conn.Close()
	if dl, ok := cctx.Deadline(); ok {
		_ = conn.SetDeadline(dl)
	}
	if _, err := conn.Write(loginRequest(name, password)); err != nil {
		return Result{}, fmt.Errorf("cannot talk to the Soulseek server: %w", err)
	}
	// The server can send other messages first; read until the login answer.
	for {
		code, body, err := readMessage(conn)
		if err != nil {
			return Result{}, fmt.Errorf("no answer from the Soulseek server: %w", err)
		}
		if code == codeLogin {
			return parseLogin(body)
		}
	}
}

func loginRequest(name, password string) []byte {
	sum := md5.Sum([]byte(name + password))
	var p bytes.Buffer
	putU32(&p, codeLogin)
	putString(&p, name)
	putString(&p, password)
	putU32(&p, majorVersion)
	putString(&p, hex.EncodeToString(sum[:]))
	putU32(&p, minorVersion)
	var out bytes.Buffer
	putU32(&out, uint32(p.Len()))
	out.Write(p.Bytes())
	return out.Bytes()
}

func readMessage(r io.Reader) (uint32, []byte, error) {
	var n uint32
	if err := binary.Read(r, binary.LittleEndian, &n); err != nil {
		return 0, nil, err
	}
	if n < 4 || n > 1<<20 {
		return 0, nil, fmt.Errorf("bad message length %d", n)
	}
	buf := make([]byte, n)
	if _, err := io.ReadFull(r, buf); err != nil {
		return 0, nil, err
	}
	return binary.LittleEndian.Uint32(buf), buf[4:], nil
}

func parseLogin(b []byte) (Result, error) {
	rd := bytes.NewReader(b)
	ok, err := rd.ReadByte()
	if err != nil {
		return Result{}, err
	}
	msg, err := getString(rd)
	if err != nil {
		return Result{}, err
	}
	if ok == 1 {
		return Result{OK: true}, nil
	}
	res := Result{Reason: msg}
	if rd.Len() > 0 {
		res.Detail, _ = getString(rd)
	}
	return res, nil
}

func putU32(b *bytes.Buffer, v uint32) {
	_ = binary.Write(b, binary.LittleEndian, v)
}

func putString(b *bytes.Buffer, s string) {
	putU32(b, uint32(len(s)))
	b.WriteString(s)
}

func getString(r *bytes.Reader) (string, error) {
	var n uint32
	if err := binary.Read(r, binary.LittleEndian, &n); err != nil {
		return "", err
	}
	if int(n) > r.Len() {
		return "", errors.New("bad string length")
	}
	s := make([]byte, n)
	_, err := io.ReadFull(r, s)
	return string(s), err
}
