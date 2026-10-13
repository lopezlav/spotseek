package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// The test binary re-executes itself as a fake packaged app, so
// bundledToolDirs sees a real bundle layout instead of the test build folder.
func TestBundledToolDirsFindBundleLayout(t *testing.T) {
	root := t.TempDir()
	exeDir := filepath.Join(root, "app")
	if runtime.GOOS == "darwin" {
		exeDir = filepath.Join(root, "Contents", "MacOS")
	}
	if err := os.MkdirAll(exeDir, 0o755); err != nil {
		t.Fatal(err)
	}
	exe := filepath.Join(exeDir, "spotseek-app")
	testExe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(testExe)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(exe, data, 0o755); err != nil {
		t.Fatal(err)
	}

	want := []string{filepath.Join(exeDir, "bin")}
	if runtime.GOOS == "darwin" {
		want = append(want, filepath.Join(root, "Contents", "Resources", "bin"))
	}
	for _, dir := range want {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}

	cmd := exec.Command(exe, "-test.run=TestBundledToolDirsChild", "--")
	cmd.Env = append(os.Environ(), "SPOTSEEK_TEST_APP=1")
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("child run failed: %v: %s", err, out)
	}
	gotRaw := strings.Split(strings.TrimSpace(string(out)), "\n")
	var got []string
	for _, line := range gotRaw {
		if strings.HasPrefix(line, "DIR ") {
			got = append(got, strings.TrimPrefix(line, "DIR "))
		}
	}
	if len(got) != len(want) {
		t.Fatalf("expected %d dirs, got %d: %v", len(want), len(got), got)
	}
	for i, dir := range want {
		if got[i] != dir {
			t.Fatalf("dir %d: expected %s, got %s", i, dir, got[i])
		}
	}
}

func TestBundledToolDirsChild(t *testing.T) {
	if os.Getenv("SPOTSEEK_TEST_APP") != "1" {
		t.Skip("only runs as a child of TestBundledToolDirsFindBundleLayout")
	}
	for _, dir := range bundledToolDirs() {
		fmt.Println("DIR " + dir)
	}
}
