package slsk

import (
	"bytes"
	"context"
	"crypto/md5"
	"encoding/hex"
	"net"
	"testing"
)

// fakeServer answers one login. It checks the request the way the real server reads it.
func fakeServer(t *testing.T, answer func(name, pass string) []byte) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		c, err := ln.Accept()
		if err != nil {
			return
		}
		defer c.Close()
		code, body, err := readMessage(c)
		if err != nil || code != codeLogin {
			return
		}
		rd := bytes.NewReader(body)
		name, _ := getString(rd)
		pass, _ := getString(rd)
		var ver [4]byte
		_, _ = rd.Read(ver[:])
		hash, _ := getString(rd)
		sum := md5.Sum([]byte(name + pass))
		if hash != hex.EncodeToString(sum[:]) || ver[0] != majorVersion {
			return
		}
		// Something else first, as the real server may send.
		var other bytes.Buffer
		putU32(&other, 8)
		putU32(&other, 41)
		putU32(&other, 0)
		_, _ = c.Write(other.Bytes())
		_, _ = c.Write(answer(name, pass))
	}()
	return ln.Addr().String()
}

func reply(ok bool, msg, detail string) []byte {
	var p bytes.Buffer
	putU32(&p, codeLogin)
	if ok {
		p.WriteByte(1)
		putString(&p, msg)
		p.Write([]byte{1, 0, 0, 127})
		putString(&p, "hash")
		p.WriteByte(0)
	} else {
		p.WriteByte(0)
		putString(&p, msg)
		if detail != "" {
			putString(&p, detail)
		}
	}
	var out bytes.Buffer
	putU32(&out, uint32(p.Len()))
	out.Write(p.Bytes())
	return out.Bytes()
}

func TestLogin(t *testing.T) {
	addr := fakeServer(t, func(name, pass string) []byte {
		if name == "taken" {
			return reply(false, "INVALIDPASS", "")
		}
		return reply(true, "welcome", "")
	})
	r, err := Login(context.Background(), addr, "newname", "secret")
	if err != nil || !r.OK {
		t.Fatalf("new name: %+v %v", r, err)
	}
	addr = fakeServer(t, func(name, pass string) []byte { return reply(false, "INVALIDPASS", "") })
	r, err = Login(context.Background(), addr, "taken", "x")
	if err != nil || r.OK || r.Reason != "INVALIDPASS" {
		t.Fatalf("taken name: %+v %v", r, err)
	}
}

func TestCheckName(t *testing.T) {
	for _, bad := range []string{"", " lead", "trail ", "ñandú", "a234567890123456789012345678901"} {
		if CheckName(bad) == nil {
			t.Errorf("%q should be refused", bad)
		}
	}
	if CheckName("dj_nico-2005") != nil {
		t.Error("a normal name was refused")
	}
}
